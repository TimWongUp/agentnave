//! Optimistic file replacement, not an atomic compare-and-swap with other applications.
use super::{
    CONFIG_BACKUP, backup_bytes,
    config::{Document, Format},
};
use serde_json::Value;
use std::{
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::Path,
};

pub(crate) fn safe_path(path: &Path, home: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("write_failed:配置路径必须是无父目录跳转的绝对路径".into());
    }
    for ancestor in path.ancestors() {
        if ancestor != home && ancestor.is_symlink() {
            return Err("symlink_path:配置路径经过符号链接，请手动处理".into());
        }
    }
    Ok(())
}
fn read(path: &Path, home: &Path) -> Result<Option<Vec<u8>>, String> {
    safe_path(path, home)?;
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("write_failed:无法读取配置".into()),
    }
}
fn hash(bytes: &Option<Vec<u8>>) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    hash.finish()
}
#[derive(Clone, Copy)]
pub(super) enum Phase {
    Prepared,
    Written,
}

pub(super) struct Target<'a> {
    pub path: &'a Path,
    pub home: &'a Path,
    pub app: &'a Path,
    pub id: &'a str,
    pub format: Format,
    pub key: &'static str,
}

pub(super) fn edit(
    target: Target<'_>,
    mutate: impl Fn(&mut Option<Value>) -> Result<(), String>,
) -> Result<bool, String> {
    edit_observed(target, mutate, |_, _| {})
}

/// The observer is a deterministic seam for concurrent-editor regressions; production uses no-op.
pub(super) fn edit_observed(
    target: Target<'_>,
    mutate: impl Fn(&mut Option<Value>) -> Result<(), String>,
    mut observe: impl FnMut(Phase, &Path),
) -> Result<bool, String> {
    let Target {
        path,
        home,
        app,
        id,
        format,
        key,
    } = target;
    safe_path(&app.join("backups"), home)?;
    for _ in 0..3 {
        let original = read(path, home)?;
        let raw = std::str::from_utf8(original.as_deref().unwrap_or_default())
            .map_err(|_| "parse_failed:配置编码无效")?;
        let before = Document::parse(raw, format, key).map_err(|e| format!("parse_failed:{e}"))?;
        let mut desired = before.entry.clone();
        mutate(&mut desired)?;
        if desired == before.entry {
            return Ok(false);
        }
        let expected = before
            .render(desired.as_ref())
            .map_err(|e| format!("write_failed:{e}"))?;
        before
            .verify(&expected, desired.as_ref())
            .map_err(|e| format!("write_failed:{e}"))?;
        let parent = path.parent().ok_or("write_failed:配置路径无父目录")?;
        safe_path(path, home)?;
        fs::create_dir_all(parent).map_err(|_| "write_failed:无法创建配置目录")?;
        let mut tmp =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "write_failed:无法暂存配置")?;
        if let Ok(meta) = fs::metadata(path) {
            tmp.as_file()
                .set_permissions(meta.permissions())
                .map_err(|_| "write_failed:无法保留文件权限")?;
        }
        tmp.write_all(expected.as_bytes())
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|_| "write_failed:无法暂存配置")?;
        observe(Phase::Prepared, path);
        let current = read(path, home)?;
        if hash(&current) != hash(&original) || current != original {
            continue;
        }
        if let Some(bytes) = original.as_ref() {
            backup_bytes(app, CONFIG_BACKUP, id, path, bytes, home)?;
        }
        // Recheck after making the backup, immediately before rename. Another application's
        // write between this check and rename remains an unavoidable, very short race window.
        if read(path, home)? != original {
            continue;
        }
        tmp.persist(path)
            .map_err(|_| "write_failed:无法原子替换配置")?;
        observe(Phase::Written, path);
        let written = read(path, home)?;
        if written.as_deref() != Some(expected.as_bytes()) {
            return Err("conflict:配置被其他进程更新，未回滚或覆盖其改动".into());
        }
        if before
            .verify(
                std::str::from_utf8(written.as_deref().unwrap()).unwrap_or(""),
                desired.as_ref(),
            )
            .is_err()
        {
            rollback_if_ours(path, home, expected.as_bytes(), original.as_deref())?;
            return Err("rolled_back:写后校验失败，已恢复原文件".into());
        }
        return Ok(true);
    }
    Err("conflict:配置连续发生并发变化，请稍后重试".into())
}

pub(super) fn rollback_if_ours(
    path: &Path,
    home: &Path,
    ours: &[u8],
    original: Option<&[u8]>,
) -> Result<(), String> {
    if read(path, home)?.as_deref() != Some(ours) {
        return Err("conflict:正式文件已有新改动，未回滚".into());
    }
    // This is also optimistic: there is a short check-to-replace window, not a file-system CAS.
    if let Some(original) = original {
        safe_path(path, home)?;
        let parent = path.parent().ok_or("write_failed:配置路径无父目录")?;
        let mut tmp =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "write_failed:无法暂存回滚配置")?;
        let permissions = fs::metadata(path)
            .map_err(|_| "conflict:正式文件已变化")?
            .permissions();
        tmp.as_file()
            .set_permissions(permissions)
            .and_then(|_| tmp.write_all(original))
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|_| "write_failed:无法暂存回滚配置")?;
        if read(path, home)?.as_deref() != Some(ours) {
            return Err("conflict:正式文件已有新改动，未回滚".into());
        }
        tmp.persist(path)
            .map_err(|_| "write_failed:无法恢复原配置")?;
        Ok(())
    } else {
        safe_path(path, home)?;
        fs::remove_file(path).map_err(|_| "write_failed:无法回滚新配置".into())
    }
}

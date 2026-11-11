use super::*;
use std::io::{Read, Write};

const WEBP_LIMIT: u64 = 2 * 1024 * 1024;
pub(super) const CLIP_LIMIT: u64 = 25 * 1024 * 1024;

/// One atom, with checked lengths for ordinary, extended and remainder sizes.
fn atom(bytes: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let size = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?);
    let kind = bytes.get(4..8)?;
    let (header, size) = match size {
        0 => (8, bytes.len()),
        1 => (
            16,
            usize::try_from(u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?)).ok()?,
        ),
        size => (8, size as usize),
    };
    if size < header {
        return None;
    }
    Some((kind, bytes.get(header..size)?, bytes.get(size..)?))
}

fn track_size(payload: &[u8]) -> Option<(u32, u32)> {
    let offset = match payload.first()? {
        0 => 76,
        1 => 88,
        _ => return None,
    };
    let w = u32::from_be_bytes(payload.get(offset..offset + 4)?.try_into().ok()?);
    let h = u32::from_be_bytes(payload.get(offset + 4..offset + 8)?.try_into().ok()?);
    (w > 0 && h > 0).then_some((w >> 16, h >> 16))
}

fn dimensions(mut bytes: &[u8], level: u8) -> Option<(u32, u32)> {
    let mut found = None;
    while !bytes.is_empty() {
        let (kind, payload, rest) = atom(bytes)?;
        let size = match (level, kind) {
            (0, b"moov") | (1, b"trak") => dimensions(payload, level + 1),
            (2, b"tkhd") => track_size(payload),
            _ => None,
        };
        if found.is_none() {
            found = size;
        }
        bytes = rest;
    }
    found
}

pub(super) fn mov_size(bytes: &[u8]) -> Option<(u32, u32)> {
    dimensions(bytes, 0)
}

fn content_root(chosen: &Path) -> Result<PathBuf, String> {
    if chosen.join(CHARACTERS[0]).is_dir() {
        return Ok(chosen.to_owned());
    }
    let mut dirs = fs::read_dir(chosen)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir());
    let first = dirs.next().ok_or("所选文件夹中没有视频包")?.path();
    if dirs.next().is_none() && first.join(CHARACTERS[0]).is_dir() {
        Ok(first)
    } else {
        Err("请选择包含 13 个角色文件夹的视频包目录".into())
    }
}

fn checked_file(path: &Path, tier: &str) -> Result<Vec<u8>, String> {
    let ext = path.extension().and_then(|e| e.to_str());
    let poster = ext == Some("webp");
    let limit = if poster { WEBP_LIMIT } else { CLIP_LIMIT };
    let mut file = fs::File::open(path).map_err(|e| format!("{}：{e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("视频包只允许普通文件".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} 超过文件大小限制", path.display()));
    }
    let size = match ext {
        Some("webp") => crate::characters::webp_size(&bytes),
        Some("mov") => mov_size(&bytes),
        Some("webm") => webm::webm_size(&bytes),
        _ => None,
    };
    let expected = if poster || tier == "544" { 544 } else { 272 };
    if size != Some((expected, expected)) {
        if !poster && let Some((w, h)) = size {
            return Err(format!(
                "所选文件夹中的视频尺寸为 {w}×{h}，请导入 {expected}×{expected} 的视频包"
            ));
        }
        return Err(format!(
            "{} 损坏或尺寸错误，应为 {expected}×{expected}",
            path.display()
        ));
    }
    Ok(bytes)
}

pub(super) fn stage(base: &Path, tier: &str, chosen: &Path) -> Result<tempfile::TempDir, String> {
    let source = content_root(chosen)?;
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let staged = tempfile::tempdir_in(base).map_err(|e| e.to_string())?;
    for file in files() {
        let bytes = checked_file(&source.join(&file), tier)?;
        let destination = staged.path().join(file);
        fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(destination, bytes).map_err(|e| e.to_string())?;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis()
        .to_string();
    fs::write(staged.path().join("stamp"), stamp).map_err(|e| e.to_string())?;
    Ok(staged)
}

pub(super) fn swap(base: &Path, tier: &str, staged: tempfile::TempDir) -> Result<(), String> {
    let old = tempfile::tempdir_in(base).map_err(|e| e.to_string())?;
    let target = base.join(tier);
    let backup = old.path().join("pack");
    if target.exists() {
        fs::rename(&target, &backup).map_err(|e| e.to_string())?;
    }
    let staged = staged.keep();
    if let Err(error) = fs::rename(&staged, &target) {
        let _ = fs::remove_dir_all(staged);
        if backup.exists()
            && let Err(restore) = fs::rename(&backup, &target)
        {
            // Preserve the previous pack if restoration itself fails.
            let recovery = old.keep();
            return Err(format!(
                "{error}；恢复失败：{restore}；旧包保存在 {}",
                recovery.display()
            ));
        }
        return Err(error.to_string());
    }
    Ok(())
}

pub(super) fn extract(
    archive: &Path,
    destination: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut count = 0;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        download::check_cancel(cancel)?;
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if !path.components().all(|p| {
            matches!(
                p,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err("压缩包包含不安全的路径".into());
        }
        if entry.header().entry_type().is_dir() {
            continue;
        }
        count += 1;
        if count > 221 || !entry.header().entry_type().is_file() || entry.size() > CLIP_LIMIT {
            return Err("压缩包包含过多文件、超大文件或非普通文件".into());
        }
        let output = destination.join(path);
        fs::create_dir_all(output.parent().ok_or("无效压缩包路径")?).map_err(|e| e.to_string())?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .map_err(|e| e.to_string())?;
        let mut buffer = [0; 65536];
        loop {
            download::check_cancel(cancel)?;
            let n = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

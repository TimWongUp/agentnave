//! Only intact App copies are writable. Version marker alone never proves ownership.
use super::{VERSION, backup};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/manager_skill.rs"));
const MARKER: &str = ".agentnave-app";
const MANIFEST: &str = ".agentnave-app-manifest.json";

pub(super) fn hash(bytes: &[u8]) -> String {
    // A deterministic content fingerprint; ownership is not an adversarial trust boundary.
    let value = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{value:016x}")
}
fn files(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<(), String> {
    for item in fs::read_dir(dir).map_err(|_| "无法读取 Skill")? {
        let path = item.map_err(|_| "无法读取 Skill")?.path();
        if path.is_symlink() {
            return Err("Skill 包含符号链接".into());
        }
        if path.is_dir() {
            files(root, &path, out)?;
        } else if path.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if name != MARKER && name != MANIFEST {
                out.insert(name, hash(&fs::read(path).map_err(|_| "无法读取 Skill")?));
            }
        } else {
            return Err("Skill 包含特殊文件".into());
        }
    }
    Ok(())
}
pub(super) fn fingerprint(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut found = BTreeMap::new();
    files(path, path, &mut found)?;
    Ok(found)
}
pub(super) fn state(path: &Path, home: &Path) -> Value {
    if super::transaction::safe_path(path, home).is_err() {
        return json!({"state":"external", "path":path, "version":null});
    }
    let marker = path.join(MARKER);
    let version = super::transaction::safe_path(&marker, home)
        .ok()
        .and_then(|_| fs::read_to_string(marker).ok());
    let state = if !path.exists() {
        "missing"
    } else if path.is_dir() && version.is_some() && owned(path, version.as_deref().unwrap()) {
        "app"
    } else {
        "external"
    };
    json!({"state": state, "path": path, "version": version.map(|v| v.trim().to_owned())})
}
fn owned(path: &Path, version: &str) -> bool {
    let Ok(found) = fingerprint(path) else {
        return false;
    };
    let Ok(bytes) = fs::read(path.join(MANIFEST)) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    if manifest["version"] != version.trim() {
        return false;
    }
    let Ok(expected) =
        serde_json::from_value::<BTreeMap<String, String>>(manifest["files"].clone())
    else {
        return false;
    };
    if found != expected {
        return false;
    }
    // Current-version copies also have to match this binary's embedded contents.
    version.trim() != VERSION
        || found
            == FILES
                .iter()
                .map(|(name, bytes)| (name.to_string(), hash(bytes)))
                .collect()
}
fn backup_tree(path: &Path, app: &Path, id: &str, home: &Path) -> Result<(), String> {
    for name in fingerprint(path)?
        .keys()
        .chain([MARKER.to_owned(), MANIFEST.to_owned()].iter())
    {
        backup(app, id, &path.join(name), home)?;
    }
    Ok(())
}
pub(super) fn install(path: &Path, app: &Path, id: &str, home: &Path) -> Result<(), String> {
    let current = state(path, home);
    if current["state"] == "external"
        || (current["state"] == "app" && current["version"] == VERSION)
    {
        return Ok(());
    }
    super::transaction::safe_path(path, home)?;
    super::transaction::safe_path(&app.join("backups"), home)?;
    let parent = path.parent().ok_or("Skill 路径无效")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建 Skill 目录")?;
    let stage = tempfile::tempdir_in(parent).map_err(|_| "无法暂存 Skill")?;
    let next = stage.path().join("next");
    fs::create_dir(&next).map_err(|_| "无法暂存 Skill")?;
    let mut manifest = BTreeMap::new();
    for (name, bytes) in FILES {
        let target = next.join(name);
        fs::create_dir_all(target.parent().unwrap()).map_err(|_| "无法创建 Skill 子目录")?;
        super::atomic(&target, bytes)?;
        manifest.insert(*name, hash(bytes));
    }
    super::atomic(&next.join(MARKER), VERSION.as_bytes())?;
    super::atomic(
        &next.join(MANIFEST),
        &serde_json::to_vec(&json!({"version": VERSION, "files": manifest})).unwrap(),
    )?;
    if state(&next, home)["state"] != "app" {
        return Err("Skill 写后校验失败".into());
    }
    let previous = stage.path().join("previous");
    if path.exists() {
        if state(path, home)["state"] != "app" {
            return Err("Skill 已被外部修改".into());
        }
        super::transaction::safe_path(path, home)?;
        backup_tree(path, app, id, home)?;
        super::transaction::safe_path(path, home)?;
        if state(path, home)["state"] != "app" {
            return Err("conflict:Skill 在备份期间被其他方式修改，已保留".into());
        }
        fs::rename(path, &previous).map_err(|_| "无法暂存旧 Skill")?;
    }
    if fs::rename(&next, path).is_err() {
        if previous.exists() && !path.exists() {
            super::transaction::safe_path(path, home)?;
            fs::rename(&previous, path).map_err(|_| "Skill 更新失败且回滚失败")?;
        }
        return Err("Skill 更新失败，原目录已保留".into());
    }
    Ok(())
}
pub(super) fn remove(path: &Path, app: &Path, id: &str, home: &Path) -> Result<(), String> {
    if state(path, home)["state"] != "app" {
        return Ok(());
    }
    super::transaction::safe_path(path, home)?;
    super::transaction::safe_path(&app.join("backups"), home)?;
    backup_tree(path, app, id, home)?;
    super::transaction::safe_path(path, home)?;
    if state(path, home)["state"] != "app" {
        return Err("conflict:Skill 在备份期间被其他方式修改，未移除".into());
    }
    // A rename makes the managed directory disappear as one operation.
    let stage = tempfile::tempdir_in(path.parent().unwrap()).map_err(|_| "无法暂存 Skill")?;
    let moved: PathBuf = stage.path().join("removed");
    fs::rename(path, moved).map_err(|_| "无法移除 Skill")?;
    Ok(())
}

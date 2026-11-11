//! The App is the only manager of this service's Skills: any other copy at a Skill path, a link
//! included, is moved whole into the App's backups and replaced. A version marker alone never proves an App copy; only a
//! Skill folder reached through a linked parent is left alone, since writing there lands elsewhere.
use super::{VERSION, backup};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

include!(concat!(env!("OUT_DIR"), "/manager_skill.rs"));
/// One embedded Skill tree: the always-installed manager or the optional review gate.
pub(super) type Files = &'static [(&'static str, &'static [u8])];
pub(super) const MANAGER: Files = FILES;
/// Installed with the manager, beside it: nobody switches these on before they are needed.
pub(super) const REQUIRED: &[(&str, Files)] = &[("agentnave-handover", HANDOVER_FILES)];
/// Skills the user installs per host from the 技能 page; add a tree here and in build.rs.
pub(super) const OPTIONAL: &[(&str, Files)] = &[("agentnave-review", REVIEW_FILES)];

/// `GET /app/skills`: each optional Skill's name, its SKILL.md title and description, and the
/// full SKILL.md so the page can show how it works before the user installs it.
pub fn catalog() -> Value {
    let items: Vec<Value> = OPTIONAL
        .iter()
        .map(|(name, embedded)| {
            let text = embedded
                .iter()
                .find(|(file, _)| *file == "SKILL.md")
                .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_default();
            // Front matter is the block between the first two `---` lines.
            let description = text
                .lines()
                .skip(1)
                .take_while(|line| line.trim() != "---")
                .find_map(|line| line.strip_prefix("description:"))
                .unwrap_or("")
                .trim();
            let title = text
                .lines()
                .find_map(|line| line.strip_prefix("# "))
                .unwrap_or(name)
                .trim();
            json!({"name": name, "title": title, "description": description, "content": text})
        })
        .collect();
    json!({"skills": items})
}
const MARKER: &str = ".agentnave-app";
const MANIFEST: &str = ".agentnave-app-manifest.json";

type FileStamp = (String, u64, SystemTime);
type CachedFingerprint = (Vec<FileStamp>, BTreeMap<String, String>);

static FINGERPRINTS: LazyLock<Mutex<HashMap<PathBuf, CachedFingerprint>>> =
    LazyLock::new(Mutex::default);

pub(super) fn hash(bytes: &[u8]) -> String {
    // A deterministic content fingerprint; ownership is not an adversarial trust boundary.
    let value = bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{value:016x}")
}
fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, (PathBuf, u64, SystemTime)>,
) -> Result<(), String> {
    for item in fs::read_dir(dir).map_err(|_| "无法读取 Skill")? {
        let entry = item.map_err(|_| "无法读取 Skill")?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|_| "无法读取 Skill")?;
        if kind.is_symlink() {
            return Err("Skill 包含符号链接".into());
        }
        if kind.is_dir() {
            collect_files(root, &path, out)?;
        } else if kind.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if name != MARKER && name != MANIFEST {
                let meta = entry.metadata().map_err(|_| "无法读取 Skill")?;
                let modified = meta.modified().unwrap_or(UNIX_EPOCH);
                out.insert(name, (path, meta.len(), modified));
            }
        } else {
            return Err("Skill 包含特殊文件".into());
        }
    }
    Ok(())
}
pub(super) fn fingerprint(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut entries = BTreeMap::new();
    collect_files(path, path, &mut entries)?;
    let stamps: Vec<FileStamp> = entries
        .iter()
        .map(|(name, (_, len, modified))| (name.clone(), *len, *modified))
        .collect();
    {
        let cache = FINGERPRINTS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_stamps, cached_hashes)) = cache.get(path)
            && *cached_stamps == stamps
        {
            return Ok(cached_hashes.clone());
        }
    }
    let mut found = BTreeMap::new();
    for (name, (file_path, _, _)) in entries {
        let bytes = fs::read(file_path).map_err(|_| "无法读取 Skill")?;
        found.insert(name, hash(&bytes));
    }
    FINGERPRINTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(path.to_path_buf(), (stamps, found.clone()));
    Ok(found)
}
/// The folder holding a Skill: it must not be reached through a link; the Skill itself may be one.
fn safe_parent(path: &Path, home: &Path) -> Result<(), String> {
    crate::config_paths::safe_path(path.parent().ok_or("Skill 路径无效")?, home)
}
/// `missing`, `app` (an intact App copy), `external` (anything else, replaced by the next
/// install) or `blocked` (reached through a linked folder; never written).
pub(super) fn state(path: &Path, home: &Path, embedded: Files) -> Value {
    if safe_parent(path, home).is_err() {
        return json!({"state":"blocked", "path":path, "version":null});
    }
    if path.is_symlink() {
        return json!({"state":"external", "path":path, "version":null});
    }
    let marker = path.join(MARKER);
    let version = crate::config_paths::safe_path(&marker, home)
        .ok()
        .and_then(|_| fs::read_to_string(marker).ok());
    let state = if !path.exists() {
        "missing"
    } else if path.is_dir()
        && version.is_some()
        && owned(path, version.as_deref().unwrap(), embedded)
    {
        "app"
    } else {
        "external"
    };
    json!({"state": state, "path": path, "version": version.map(|v| v.trim().to_owned())})
}
fn owned(path: &Path, version: &str, embedded: Files) -> bool {
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
            == embedded
                .iter()
                .map(|(name, bytes)| (name.to_string(), hash(bytes)))
                .collect()
}
/// Backs up an App copy file by file; its files are the App's own, so the shared budget of
/// Skill backups may later drop them.
fn backup_tree(path: &Path, app: &Path, id: &str, home: &Path) -> Result<(), String> {
    for name in fingerprint(path)?
        .keys()
        .chain([MARKER.to_owned(), MANIFEST.to_owned()].iter())
    {
        backup(app, id, &path.join(name), home)?;
    }
    Ok(())
}
/// Moves anything else at a Skill path, a link included, into a folder of its own in the App's
/// backups: nothing is copied, so a change made meanwhile is kept too; a folder is never one of
/// the backup files evicted past the budget, and a new one never lands on an older one. Returns
/// where it went.
fn move_to_backups(path: &Path, app: &Path, id: &str, home: &Path) -> Result<PathBuf, String> {
    let dir = app.join("backups");
    crate::config_paths::safe_path(&dir, home)?;
    fs::create_dir_all(&dir).map_err(|_| "无法创建备份目录")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = path.file_name().unwrap_or_default();
    let folder = dir.join(format!("{stamp:020}-skill-{id}-{}", name.to_string_lossy()));
    fs::create_dir(&folder).map_err(|_| "无法创建备份目录")?;
    let target = folder.join(name);
    safe_parent(path, home)?;
    if fs::rename(path, &target).is_err() {
        let _ = fs::remove_dir(&folder);
        return Err("无法把原有 Skill 移入备份目录，已保留".to_owned());
    }
    Ok(target)
}
/// Whether anything, a dangling link included, sits at a path.
fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
pub(super) fn install(
    path: &Path,
    app: &Path,
    id: &str,
    home: &Path,
    embedded: Files,
) -> Result<(), String> {
    let current = state(path, home, embedded);
    if current["state"] == "blocked" {
        return Err("symlink_path:Skill 目录经过符号链接，请手动处理".into());
    }
    if current["state"] == "app" && current["version"] == VERSION {
        return Ok(());
    }
    safe_parent(path, home)?;
    crate::config_paths::safe_path(&app.join("backups"), home)?;
    let parent = path.parent().ok_or("Skill 路径无效")?;
    fs::create_dir_all(parent).map_err(|_| "无法创建 Skill 目录")?;
    let stage = tempfile::tempdir_in(parent).map_err(|_| "无法暂存 Skill")?;
    let next = stage.path().join("next");
    fs::create_dir(&next).map_err(|_| "无法暂存 Skill")?;
    let mut manifest = BTreeMap::new();
    for (name, bytes) in embedded {
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
    if state(&next, home, embedded)["state"] != "app" {
        return Err("Skill 写后校验失败".into());
    }
    let mut previous = stage.path().join("previous");
    if current["state"] == "app" {
        backup_tree(path, app, id, home)?;
        safe_parent(path, home)?;
        if state(path, home, embedded) != current {
            return Err("conflict:Skill 在备份期间被其他方式修改，已保留".into());
        }
        fs::rename(path, &previous).map_err(|_| "无法暂存旧 Skill")?;
    } else if present(path) {
        previous = move_to_backups(path, app, id, home)?;
    }
    if fs::rename(&next, path).is_err() {
        if present(&previous) && !present(path) {
            safe_parent(path, home)?;
            fs::rename(&previous, path).map_err(|_| "Skill 更新失败且回滚失败")?;
        }
        return Err("Skill 更新失败，原目录已保留".into());
    }
    Ok(())
}
pub(super) fn remove(
    path: &Path,
    app: &Path,
    id: &str,
    home: &Path,
    embedded: Files,
) -> Result<(), String> {
    let current = state(path, home, embedded);
    if current["state"] == "missing" || current["state"] == "blocked" {
        return Ok(());
    }
    if current["state"] != "app" {
        return move_to_backups(path, app, id, home).map(|_| ());
    }
    crate::config_paths::safe_path(&app.join("backups"), home)?;
    backup_tree(path, app, id, home)?;
    safe_parent(path, home)?;
    if state(path, home, embedded) != current {
        return Err("conflict:Skill 在备份期间被其他方式修改，未移除".into());
    }
    // A rename makes the directory disappear as one operation.
    let stage = tempfile::tempdir_in(path.parent().unwrap()).map_err(|_| "无法暂存 Skill")?;
    let moved: PathBuf = stage.path().join("removed");
    fs::rename(path, moved).map_err(|_| "无法移除 Skill")?;
    Ok(())
}

//! Folders that could be linked as accounts, and the checks a folder passes first.

use super::{Account, managed_root, real};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// A folder that holds something only a Claude folder holds.
fn is_claude_folder(dir: &Path) -> bool {
    [".claude.json", ".credentials.json", "settings.json"]
        .iter()
        .any(|name| dir.join(name).is_file())
        || dir.join("projects").is_dir()
}

/// The folder an existing account cannot be: the default account's, a registered one's, or one
/// that holds or sits inside either (their chats would be shared).
fn taken(dir: &Path, default_dir: &Path, accounts: &[Account]) -> bool {
    let dir = real(dir);
    std::iter::once(default_dir)
        .chain(accounts.iter().map(|account| account.dir.as_path()))
        .map(real)
        .any(|other| dir.starts_with(&other) || other.starts_with(&dir))
}

/// Home's folders named like a Claude folder that are not the default account's or registered.
pub(super) fn candidates(home: &Path, default_dir: &Path, accounts: &[Account]) -> Vec<Value> {
    let mut found: Vec<PathBuf> = fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let name = name.to_lowercase();
            name.strip_prefix('.')
                .unwrap_or(&name)
                .starts_with("claude")
                && path.is_dir()
                && !path.is_symlink()
                && is_claude_folder(path)
                && !path.starts_with(managed_root(home))
        })
        .filter(|path| !taken(path, default_dir, accounts))
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            json!({"dir": path.display().to_string(), "name": name})
        })
        .collect()
}

/// Why `dir` cannot be linked, or the folder to register.
pub(super) fn check(
    dir: &Path,
    default_dir: &Path,
    accounts: &[Account],
) -> Result<PathBuf, String> {
    if !dir.is_absolute() || !dir.is_dir() {
        return Err("文件夹不存在".into());
    }
    if taken(dir, default_dir, accounts) {
        return Err("这个文件夹已经是默认账号或已登记的账号".into());
    }
    Ok(dir.to_owned())
}

/// The chat ids a Claude folder keeps: `projects/<project>/<id>.jsonl`.
fn session_ids(dir: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    let projects = fs::read_dir(dir.join("projects"))
        .into_iter()
        .flatten()
        .flatten();
    for project in projects {
        for file in fs::read_dir(project.path()).into_iter().flatten().flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".jsonl") {
                ids.insert(id.to_owned());
            }
        }
    }
    ids
}

/// How many chat ids `dir` shares with the default account or a registered one: such a chat
/// would name two accounts at once.
pub(super) fn shared_sessions(dir: &Path, default_dir: &Path, accounts: &[Account]) -> usize {
    let own = session_ids(dir);
    if own.is_empty() {
        return 0;
    }
    let mut shared = HashSet::new();
    for other in std::iter::once(default_dir).chain(accounts.iter().map(|a| a.dir.as_path())) {
        shared.extend(session_ids(other).intersection(&own).cloned());
    }
    shared.len()
}

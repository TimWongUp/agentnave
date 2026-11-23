//! Linking a folder that already holds a Claude account (ADR-0118): found by a scan of the home
//! folder or named by a path. Such an account is registered as `managed: false`, so removing it
//! only unbinds it. The sign-in states of the scan go to the page that asked and nowhere else.

use super::{
    Account, absolute, auth, clean_label, managed_root, next_id, normalized, read, real,
    remember_id, save, writing,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The folder a path field names: `~` stands for the home folder, anything else must be absolute.
/// A trailing separator is dropped, because the CLI keys its stored sign-in by the exact text.
pub(super) fn expand(home: &Path, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let expanded = match text.strip_prefix('~') {
        Some("") => home.to_owned(),
        Some(rest) if rest.starts_with(separators) => {
            home.join(rest.trim_start_matches(separators))
        }
        _ if absolute(text) => PathBuf::from(text),
        _ => return Err("请填写绝对路径，或以 ~ 开头".into()),
    };
    let shown = expanded.display().to_string();
    let trimmed = shown.trim_end_matches(separators);
    Ok(if trimmed.is_empty() || trimmed.ends_with(':') {
        expanded
    } else {
        PathBuf::from(trimmed)
    })
}

fn same(left: &Path, right: &Path) -> bool {
    real(left) == real(right)
        || normalized(&left.display().to_string()) == normalized(&right.display().to_string())
}

/// Why `dir` cannot be linked, if it cannot: it must exist and be neither the default account's
/// folder nor one already registered.
fn check(dir: &Path, default: &Path, registered: &[Account]) -> Result<(), String> {
    if !dir.exists() {
        return Err("目录不存在".into());
    }
    if !dir.is_dir() {
        return Err("这个路径不是文件夹".into());
    }
    if same(dir, default) {
        return Err("这是默认账号的目录".into());
    }
    match registered.iter().find(|account| same(dir, &account.dir)) {
        Some(account) => Err(format!("这个目录已关联为“{}”", account.label)),
        None => Ok(()),
    }
}

/// The folders in `home` named `.claude…` that hold a `.claude.json`, minus the default account's
/// and the registered ones, in name order.
pub(super) fn discover(home: &Path, default: &Path, registered: &[Account]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".claude"))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(".claude.json").is_file())
        .filter(|path| check(path, default, registered).is_ok())
        .collect();
    found.sort();
    found
}

/// The scan's answer: each folder with what `claude auth status` says about it, read side by side.
pub(super) fn candidates(dirs: &[PathBuf], program: Option<&Path>) -> Vec<Value> {
    std::thread::scope(|scope| {
        let reads: Vec<_> = dirs
            .iter()
            .map(|dir| {
                scope.spawn(move || match program {
                    Some(program) => auth::status_with(program, Some(dir)),
                    None => json!({"error": "这个 CLI 没有安装"}),
                })
            })
            .collect();
        dirs.iter()
            .zip(reads)
            .map(|(dir, read)| {
                let state = read
                    .join()
                    .unwrap_or_else(|_| json!({"error": "无法读取登录状态"}));
                let name = dir.file_name().unwrap_or_default().to_string_lossy();
                json!({"dir": dir.display().to_string(), "name": name, "state": state})
            })
            .collect()
    })
}

/// Registers the folder `dir` (as typed or picked) as an external account named `label`.
pub(super) fn link(
    root: &Path,
    home: &Path,
    default: &Path,
    label: &str,
    dir: &str,
) -> Result<Account, String> {
    let label = clean_label(label)?;
    let dir = expand(home, dir)?;
    let _writing = writing();
    let mut accounts = read(root)?;
    check(&dir, default, &accounts)?;
    let managed = managed_root(home);
    fs::create_dir_all(&managed).map_err(|error| format!("无法保存账号：{error}"))?;
    let account = Account {
        id: next_id(&accounts, &managed),
        label,
        dir,
        managed: false,
    };
    accounts.push(account.clone());
    save(root, &accounts)?;
    remember_id(&managed, &account.id);
    Ok(account)
}

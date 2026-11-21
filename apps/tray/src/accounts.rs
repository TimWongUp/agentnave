//! The named accounts of a CLI (ADR-0107): `accounts.json` in the data root, which the App writes
//! and the service only reads. A named account is one folder the CLI keeps its sign-in, settings
//! and chats in (`CLAUDE_CONFIG_DIR` for Claude); the default account is no row here. The shared
//! samples in `tests/fixtures/app-files/accounts` pin what a valid file is.

mod auth;
mod remove;
mod request;
#[cfg(test)]
mod tests;

use crate::data_files::write_atomic;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub(crate) use auth::CONFIG_VARIABLE;
pub(crate) use request::{Context, request};

pub(crate) const FILE: &str = "accounts.json";
/// The one CLI that has accounts so far.
pub(crate) const PROVIDER: &str = "claude";
const FIELDS: [&str; 5] = ["id", "provider", "label", "dir", "managed"];
const LABEL_MAX: usize = 40;
const ID_MAX: usize = 32;
/// Where the App makes the folders of the accounts it manages, outside the data root so that
/// moving or clearing the data root never takes a sign-in with it.
const MANAGED: &str = ".agentnave-accounts";
/// The highest number an automatic id ever took, so a deleted account's id is not given again.
const LAST_ID: &str = ".last-id";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Account {
    pub id: String,
    pub label: String,
    pub dir: PathBuf,
    pub managed: bool,
}

impl Account {
    pub(crate) fn value(&self) -> Value {
        json!({"id": self.id, "provider": PROVIDER, "label": self.label,
            "dir": self.dir.display().to_string(), "managed": self.managed})
    }
}

/// Edits to the file wait for each other; the service never writes it.
static WRITING: Mutex<()> = Mutex::new(());

fn writing() -> MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The folder of the accounts the App manages for `provider`'s CLI.
pub(crate) fn managed_root(home: &Path) -> PathBuf {
    home.join(MANAGED).join(PROVIDER)
}

pub(crate) fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= ID_MAX
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-')
}

/// The name as saved: 1 to 40 characters once trimmed, none of them a control character.
pub(crate) fn clean_label(text: &str) -> Result<String, String> {
    let label = text.trim();
    let count = label.chars().count();
    if count == 0 || count > LABEL_MAX || label.chars().any(char::is_control) {
        return Err(format!("账号名称需为 1–{LABEL_MAX} 个字符"));
    }
    Ok(label.to_owned())
}

/// Absolute on any platform: the shared samples hold POSIX paths that must pass everywhere.
fn absolute(dir: &str) -> bool {
    let bytes = dir.as_bytes();
    dir.starts_with('/')
        || dir.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

/// The folder as compared for duplicates: separators and `.`/`..` settled, no case on Windows.
fn normalized(dir: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split(separators) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if cfg!(windows) {
        joined.to_lowercase()
    } else {
        joined
    }
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} 必须为对象"))
}

fn validate_row(row: &Value, what: &str) -> Result<(), String> {
    let fields = object(row, what)?;
    if let Some(key) = fields.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(format!("{what} 含未知字段 {key}"));
    }
    if let Some(key) = FIELDS.iter().find(|key| !fields.contains_key(**key)) {
        return Err(format!("{what} 缺少 {key}"));
    }
    if !fields["id"].as_str().is_some_and(valid_id) {
        return Err(format!("{what} 的 id 无效"));
    }
    if fields["provider"] != PROVIDER {
        return Err(format!("{what} 的 CLI 不受支持"));
    }
    if !fields["label"]
        .as_str()
        .is_some_and(|label| clean_label(label).is_ok())
    {
        return Err(format!("{what} 的名称无效"));
    }
    if !fields["dir"].as_str().is_some_and(absolute) {
        return Err(format!("{what} 的目录必须是绝对路径"));
    }
    if !fields["managed"].is_boolean() {
        return Err(format!("{what} 的 managed 必须为布尔值"));
    }
    Ok(())
}

/// accounts.json: `{"version": 1, "accounts": [{id, provider, label, dir, managed}]}`. No file
/// means no named account. Structure only: the default account's folder is known at run time.
pub fn validate(document: &Value) -> Result<(), String> {
    let top = object(document, FILE)?;
    if let Some(key) = top
        .keys()
        .find(|key| !["version", "accounts"].contains(&key.as_str()))
    {
        return Err(format!("{FILE} 含未知字段 {key}"));
    }
    if document["version"].as_u64() != Some(1) {
        return Err(format!("{FILE} 的 version 必须为 1"));
    }
    let rows = document["accounts"]
        .as_array()
        .ok_or("accounts 必须为数组")?;
    let (mut ids, mut dirs) = (HashSet::new(), HashSet::new());
    for (index, row) in rows.iter().enumerate() {
        let what = format!("accounts[{index}]");
        validate_row(row, &what)?;
        if !ids.insert(row["id"].as_str().unwrap_or_default()) {
            return Err(format!("{what} 的 id 重复"));
        }
        if !dirs.insert(normalized(row["dir"].as_str().unwrap_or_default())) {
            return Err(format!("{what} 的目录重复"));
        }
    }
    Ok(())
}

fn document(accounts: &[Account]) -> Value {
    json!({"version": 1, "accounts": accounts.iter().map(Account::value).collect::<Vec<_>>()})
}

/// The registered accounts; none while the file is absent.
pub(crate) fn read(root: &Path) -> Result<Vec<Account>, String> {
    let content = match fs::read(root.join(FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取 {FILE}：{error}")),
    };
    let document: Value =
        serde_json::from_slice(&content).map_err(|error| format!("{FILE} 损坏：{error}"))?;
    validate(&document).map_err(|error| format!("{FILE} 格式错误：{error}"))?;
    Ok(document["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| Account {
            id: row["id"].as_str().unwrap_or_default().to_owned(),
            label: row["label"].as_str().unwrap_or_default().trim().to_owned(),
            dir: PathBuf::from(row["dir"].as_str().unwrap_or_default()),
            managed: row["managed"] == true,
        })
        .collect())
}

fn save(root: &Path, accounts: &[Account]) -> Result<(), String> {
    let document = document(accounts);
    validate(&document)?;
    write_atomic(root, FILE, &document)
}

/// The accounts for the pages, or why the file cannot be used.
pub(crate) fn state(root: &Path) -> Value {
    match read(root) {
        Ok(accounts) => json!({
            "accounts": accounts.iter().map(Account::value).collect::<Vec<_>>(),
            "error": null,
        }),
        Err(error) => json!({"accounts": [], "error": error}),
    }
}

/// A path as the file system names it, so two names for one folder compare equal. Windows paths
/// stay as written: their canonical form carries a `\\?\` prefix the CLIs do not take.
fn real(path: &Path) -> PathBuf {
    if cfg!(windows) {
        return path.to_owned();
    }
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

fn id_number(id: &str) -> Option<u64> {
    id.strip_prefix('a')?.parse().ok()
}

/// The next `a<N>`: above every number a registered account, a folder left in the managed root or
/// an earlier creation used.
fn next_id(accounts: &[Account], managed: &Path) -> String {
    let named = fs::read_dir(managed)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| id_number(&entry.file_name().to_string_lossy()));
    let remembered = fs::read_to_string(managed.join(LAST_ID))
        .ok()
        .and_then(|text| text.trim().parse().ok());
    let highest = accounts
        .iter()
        .filter_map(|account| id_number(&account.id))
        .chain(named)
        .chain(remembered)
        .max()
        .unwrap_or(0);
    format!("a{}", highest + 1)
}

#[cfg(unix)]
fn private(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn private(_dir: &Path) {}

/// Registers a new managed account: its id, its folder in the managed root and its name.
pub(crate) fn create(root: &Path, home: &Path, label: &str) -> Result<Account, String> {
    let label = clean_label(label)?;
    let _writing = writing();
    let mut accounts = read(root)?;
    let managed = managed_root(home);
    fs::create_dir_all(&managed).map_err(|error| format!("无法创建账号目录：{error}"))?;
    let id = next_id(&accounts, &managed);
    let dir = managed.join(&id);
    fs::create_dir(&dir).map_err(|error| format!("无法创建账号目录：{error}"))?;
    private(&dir);
    let account = Account {
        id: id.clone(),
        label,
        dir: real(&dir),
        managed: true,
    };
    accounts.push(account.clone());
    if let Err(error) = save(root, &accounts) {
        let _ = fs::remove_dir(&dir);
        return Err(error);
    }
    let _ = fs::write(managed.join(LAST_ID), id.trim_start_matches('a'));
    Ok(account)
}

pub(crate) fn rename(root: &Path, id: &str, label: &str) -> Result<Account, String> {
    let label = clean_label(label)?;
    let _writing = writing();
    let mut accounts = read(root)?;
    let account = accounts
        .iter_mut()
        .find(|account| account.id == id)
        .ok_or("找不到这个账号")?;
    account.label = label;
    let renamed = account.clone();
    save(root, &accounts)?;
    Ok(renamed)
}

/// Takes the account out of the file; its folder stays as it is.
pub(crate) fn unbind(root: &Path, id: &str) -> Result<Account, String> {
    let _writing = writing();
    let mut accounts = read(root)?;
    let at = accounts
        .iter()
        .position(|account| account.id == id)
        .ok_or("找不到这个账号")?;
    let account = accounts.remove(at);
    save(root, &accounts)?;
    Ok(account)
}

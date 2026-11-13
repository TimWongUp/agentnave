//! decision-keys.json: `{"version": 1, "keys": {"gemini": key}}`, readable by the current user
//! only. The App writes it and the service reads it; nothing here or in a response ever holds a
//! key's text beyond the moment it is written.

use serde_json::{Value, json};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Mutex;

const KEYS_FILE: &str = "decision-keys.json";
const KEY_NAME: &str = "gemini";
const KEY_MAX_BYTES: usize = 4096;
static WRITE: Mutex<()> = Mutex::new(());

/// The `keys` object of the file, empty while it is missing.
fn read_keys(root: &Path) -> Result<serde_json::Map<String, Value>, String> {
    let broken = || format!("{KEYS_FILE} 格式错误，请先修复或删除这个文件");
    let content = match fs::read(root.join(KEYS_FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Default::default()),
        Err(_) => return Err(format!("无法读取 {KEYS_FILE}")),
    };
    let document: Value = serde_json::from_slice(&content).map_err(|_| broken())?;
    let keys = document["keys"].as_object().filter(|keys| {
        document["version"].as_u64() == Some(1) && keys.values().all(Value::is_string)
    });
    keys.cloned().ok_or_else(broken)
}

/// Whether a key is set; the key itself never leaves this module.
pub(crate) fn key_state(root: &Path) -> Result<bool, String> {
    let keys = read_keys(root)?;
    let key = keys.get(KEY_NAME).and_then(Value::as_str);
    Ok(key.is_some_and(|key| !key.trim().is_empty()))
}

/// Saves the decision model's key, or clears it when `key` is empty.
pub(crate) fn save_key(root: &Path, payload: &Value) -> Result<bool, String> {
    let key = payload["key"].as_str().ok_or("密钥必须为字符串")?.trim();
    // Visible ASCII only, as the service requires of a key that goes into an HTTP header.
    if key.len() > KEY_MAX_BYTES || !key.chars().all(|c| c.is_ascii_graphic()) {
        return Err("密钥过长或包含无效字符".into());
    }
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = read_keys(root)?;
    if key.is_empty() {
        keys.remove(KEY_NAME);
    } else {
        keys.insert(KEY_NAME.into(), key.into());
    }
    let document = json!({"version": 1, "keys": keys});
    crate::voice::write_private_json(root, KEYS_FILE, &document)?;
    Ok(!key.is_empty())
}

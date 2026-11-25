//! decision-keys.json: `{"version": 1, "keys": {"gemini": key, "openai": key, ...}}`, one key for
//! each source that takes one, readable by the current user only. The App writes it and the service
//! reads it; nothing here or in a response ever holds a key's text beyond the moment it is written
//! or sent as a request header; a response shows a saved key only as `key_previews`' masked text.

use super::models::{SOURCES, source};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Mutex;

const KEYS_FILE: &str = "decision-keys.json";
const KEY_MAX_BYTES: usize = 4096;
static WRITE: Mutex<()> = Mutex::new(());

/// The `keys` object of the file, empty while it is missing.
fn read_keys(root: &Path) -> Result<Map<String, Value>, String> {
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

fn text(keys: &Map<String, Value>, provider: &str) -> Option<String> {
    keys.get(provider)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

fn states(keys: &Map<String, Value>) -> Value {
    Value::Object(
        SOURCES
            .iter()
            .filter(|source| source.has_key())
            .map(|source| (source.id.to_owned(), json!(text(keys, source.id).is_some())))
            .collect(),
    )
}

/// Whether each source that takes a key has one; the keys themselves never leave this module.
pub(crate) fn keys_state(root: &Path) -> Result<Value, String> {
    Ok(states(&read_keys(root)?))
}

/// Each saved key as `mask_key` shows it, for the sources that have one; empty while the file
/// cannot be read.
pub(crate) fn key_previews(root: &Path) -> Value {
    let Ok(keys) = read_keys(root) else {
        return json!({});
    };
    Value::Object(
        SOURCES
            .iter()
            .filter(|source| source.has_key())
            .filter_map(|source| {
                let key = text(&keys, source.id)?;
                Some((source.id.to_owned(), json!(crate::voice::mask_key(&key))))
            })
            .collect(),
    )
}

/// What the page is told while the file cannot be read.
pub(crate) fn no_keys() -> Value {
    states(&Map::new())
}

/// The saved key of a source, for the header of a request to it; None while there is none or the
/// file cannot be read.
pub(super) fn key_of(root: &Path, provider: &str) -> Option<String> {
    text(&read_keys(root).ok()?, provider)
}

/// Saves the key of `payload["provider"]` (the default source when it names none), or clears it
/// when `key` is empty; answers the source it was for.
pub(crate) fn save_key(root: &Path, payload: &Value) -> Result<&'static str, String> {
    let provider = match &payload["provider"] {
        Value::Null => super::models::DEFAULT_SOURCE,
        Value::String(provider) => provider,
        _ => return Err("来源必须为字符串".into()),
    };
    let source = source(provider)
        .filter(|source| source.has_key())
        .ok_or("这个来源不使用密钥")?;
    let key = payload["key"].as_str().ok_or("密钥必须为字符串")?.trim();
    // Visible ASCII only, as the service requires of a key that goes into an HTTP header.
    if key.len() > KEY_MAX_BYTES || !key.chars().all(|c| c.is_ascii_graphic()) {
        return Err("密钥过长或包含无效字符".into());
    }
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = read_keys(root)?;
    if key.is_empty() {
        keys.remove(source.id);
    } else {
        keys.insert(source.id.into(), key.into());
    }
    let document = json!({"version": 1, "keys": keys});
    crate::voice::write_private_json(root, KEYS_FILE, &document)?;
    Ok(source.id)
}

//! App settings shared with the service: `settings.json` in the data root.
//!
//! The service reads `max_running` before each start and retention before each cleanup pass.
//! The App also caps how many desks an office draws. Only the App writes the file.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::{Value, json};

const FILE: &str = "settings.json";
pub const DEFAULT_MAX_RUNNING: u64 = 8;
const MAX_RUNNING: std::ops::RangeInclusive<u64> = 1..=32;

pub fn defaults() -> Value {
    json!({"version": 1, "max_running": DEFAULT_MAX_RUNNING, "history_retention_days": null, "claude_interactive": false})
}

/// The normalized known settings; the same rules as the service:
/// unknown keys are ignored and a missing `max_running` means the default.
pub fn validate(document: &Value) -> Result<Value, String> {
    let object = document.as_object().ok_or("设置必须为 JSON 对象")?;
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("version 必须为 1".into());
    }
    let max_running = match object.get("max_running") {
        None => DEFAULT_MAX_RUNNING,
        Some(value) => value
            .as_u64()
            .filter(|value| MAX_RUNNING.contains(value))
            .ok_or("max_running 必须为 1–32 的整数")?,
    };
    let retention = match object.get("history_retention_days") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|days| matches!(days, 30 | 90 | 180))
                .ok_or("history_retention_days 必须为 null、30、90 或 180")?,
        ),
    };
    let claude_interactive = match object.get("claude_interactive") {
        None => false,
        Some(value) => value.as_bool().ok_or("claude_interactive 必须为布尔值")?,
    };
    Ok(
        json!({"version": 1, "max_running": max_running, "history_retention_days": retention, "claude_interactive": claude_interactive}),
    )
}

/// The saved document; `None` when there is none yet.
pub fn read(root: &Path) -> Result<Option<Value>, String> {
    let content = match fs::read(root.join(FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 {FILE}：{error}")),
    };
    let document: Value =
        serde_json::from_slice(&content).map_err(|error| format!("{FILE} 损坏：{error}"))?;
    validate(&document)
        .map(Some)
        .map_err(|error| format!("{FILE} 格式错误：{error}"))
}

/// The effective limit: the saved one, or the default when the file is missing or broken.
pub fn max_running(root: &Path) -> u64 {
    read(root)
        .ok()
        .flatten()
        .and_then(|document| document["max_running"].as_u64())
        .unwrap_or(DEFAULT_MAX_RUNNING)
}

/// What the settings page shows; a broken file reports why and offers the defaults to save.
pub fn state(root: &Path) -> Value {
    match read(root) {
        Ok(Some(document)) => json!({"settings": document, "saved": true, "error": null}),
        Ok(None) => json!({"settings": defaults(), "saved": false, "error": null}),
        Err(error) => json!({"settings": defaults(), "saved": false, "error": error}),
    }
}

/// Save over the file on disk: the given keys win and keys this build does not know stay,
/// so a page that saves one setting keeps the others, the service's and newer versions'.
pub fn save(root: &Path, document: &Value) -> Result<(), String> {
    let given = document.as_object().ok_or("设置必须为 JSON 对象")?;
    let mut merged = fs::read(root.join(FILE))
        .ok()
        .and_then(|content| serde_json::from_slice::<Value>(&content).ok())
        .and_then(|saved| saved.as_object().cloned())
        .unwrap_or_default();
    // A broken saved limit must not block saving other keys; it falls back to the default.
    let saved_limit = merged.get("max_running").and_then(Value::as_u64);
    if !saved_limit.is_some_and(|limit| MAX_RUNNING.contains(&limit)) {
        merged.remove("max_running");
    }
    merged.extend(given.clone());
    let known = validate(&Value::Object(merged.clone()))?;
    merged.extend(known.as_object().cloned().unwrap_or_default());
    crate::members::write_atomic(root, FILE, &Value::Object(merged))
}

#[cfg(test)]
mod tests;

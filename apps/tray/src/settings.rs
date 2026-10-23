//! App settings shared with the service: `settings.json` in the data root.
//!
//! The service reads `max_running` before each start and ignores other keys; the App also caps
//! how many desks an office draws with it. Only the App writes the file.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde_json::{Value, json};

const FILE: &str = "settings.json";
pub const DEFAULT_MAX_RUNNING: u64 = 8;
const MAX_RUNNING: std::ops::RangeInclusive<u64> = 1..=32;

pub fn defaults() -> Value {
    json!({"version": 1, "max_running": DEFAULT_MAX_RUNNING})
}

/// The normalized document (only `version` and `max_running`); the same rules as the service:
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
    Ok(json!({"version": 1, "max_running": max_running}))
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

pub fn save(root: &Path, document: &Value) -> Result<(), String> {
    crate::members::write_atomic(root, FILE, &validate(document)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_validate_and_save_atomically() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
        assert_eq!(state(root.path())["saved"], false);
        for bad in [
            json!({"version": 1, "max_running": 0}),
            json!({"version": 1, "max_running": 33}),
            json!({"version": 1, "max_running": 2.5}),
            json!({"version": 1, "max_running": true}),
            json!({"version": 2, "max_running": 4}),
            json!({"version": 1.0, "max_running": 4}),
            json!({"version": true, "max_running": 4}),
            json!([]),
        ] {
            assert!(save(root.path(), &bad).is_err(), "{bad}");
        }
        assert!(!root.path().join(FILE).exists());
        save(root.path(), &json!({"version": 1, "max_running": 3})).unwrap();
        assert_eq!(max_running(root.path()), 3);
        assert_eq!(state(root.path())["settings"]["max_running"], 3);
        // Like the service: unknown keys are ignored, and saving keeps only the known two.
        fs::write(
            root.path().join(FILE),
            br#"{"version":1,"max_running":2,"future":true}"#,
        )
        .unwrap();
        assert_eq!(max_running(root.path()), 2);
        assert_eq!(
            state(root.path())["settings"],
            json!({"version": 1, "max_running": 2})
        );
        save(
            root.path(),
            &json!({"version": 1, "max_running": 5, "x": 1}),
        )
        .unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(root.path().join(FILE)).unwrap()).unwrap();
        assert_eq!(saved, json!({"version": 1, "max_running": 5}));
        fs::write(root.path().join(FILE), br#"{"version":1}"#).unwrap();
        assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
        assert_eq!(state(root.path())["saved"], true);
        // A broken file keeps the default limit and tells the page why.
        fs::write(root.path().join(FILE), b"{oops").unwrap();
        assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
        let broken = state(root.path());
        assert!(broken["error"].as_str().unwrap().contains("损坏"));
        assert_eq!(broken["settings"]["max_running"], DEFAULT_MAX_RUNNING);
    }
}

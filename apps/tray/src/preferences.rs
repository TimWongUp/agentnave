//! The App's own settings, apart from the data root the service shares. They live in the
//! App's config folder, so a preview build keeps its own and never changes the release's.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

const FILE: &str = "preferences.json";

/// `app_config_dir` of this build: the config folder named by the bundle identifier.
pub fn dir() -> PathBuf {
    let id = if crate::PREVIEW {
        "io.github.timwongup.agentnave.preview"
    } else {
        "io.github.timwongup.agentnave"
    };
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(id)
}

/// The saved document, or an empty one when there is none yet.
pub fn read(dir: &Path) -> Result<Map<String, Value>, String> {
    let content = match fs::read(dir.join(FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("无法读取 {FILE}：{error}")),
    };
    match serde_json::from_slice(&content) {
        Ok(Value::Object(document)) => Ok(document),
        _ => Err(format!("{FILE} 损坏")),
    }
}

/// What the pages read; a broken file reports why and reads as empty.
pub fn state(dir: &Path) -> Value {
    match read(dir) {
        Ok(document) => json!({"preferences": document, "error": null}),
        Err(error) => json!({"preferences": {}, "error": error}),
    }
}

/// Each section checks the fields it knows; a new section adds one line here.
fn validate_field(section: &str, field: &str, value: &Value) -> Result<(), String> {
    match section {
        "general" => general(field, value),
        _ => Err(format!("未知的设置分区：{section}")),
    }
}

fn general(field: &str, value: &Value) -> Result<(), String> {
    match field {
        "language" => one_of(value, &["system", "zh", "en"], "general.language"),
        _ => Err(format!("未知的设置项：general.{field}")),
    }
}

fn one_of(value: &Value, allowed: &[&str], name: &str) -> Result<(), String> {
    match value.as_str() {
        Some(v) if allowed.contains(&v) => Ok(()),
        _ => Err(format!("{name} 必须为 {}", allowed.join("、"))),
    }
}

/// Merge `{section: {field: value}}` into the saved document. Only the given fields change;
/// fields and sections this build does not know stay as they were.
pub fn update(dir: &Path, changes: &Value) -> Result<Value, String> {
    let changes = changes.as_object().ok_or("设置必须为 JSON 对象")?;
    for (section, fields) in changes {
        let fields = fields
            .as_object()
            .ok_or_else(|| format!("{section} 必须为 JSON 对象"))?;
        for (field, value) in fields {
            validate_field(section, field, value)?;
        }
    }
    // A broken file is replaced rather than blocking every later change.
    let mut document = read(dir).unwrap_or_default();
    for (section, fields) in changes {
        let target = document
            .entry(section.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if !target.is_object() {
            *target = Value::Object(Map::new());
        }
        let target = target.as_object_mut().expect("just made an object");
        for (field, value) in fields.as_object().into_iter().flatten() {
            target.insert(field.clone(), value.clone());
        }
    }
    fs::create_dir_all(dir).map_err(|error| format!("无法创建设置目录：{error}"))?;
    let document = Value::Object(document);
    crate::members::write_atomic(dir, FILE, &document)?;
    Ok(document)
}

/// The interface language a page opens in: `zh`, `en`, or `system` for the page to decide.
pub fn language(dir: &Path) -> String {
    read(dir)
        .ok()
        .and_then(|document| {
            document
                .get("general")?
                .get("language")?
                .as_str()
                .map(str::to_owned)
        })
        .filter(|language| ["zh", "en"].contains(&language.as_str()))
        .unwrap_or_else(|| "system".into())
}

/// The page with its `<html lang>` set, so text is right from the first paint; `system`
/// leaves the choice to common.js, which reads the system language.
pub fn localize(html: &[u8], language: &str) -> Vec<u8> {
    let opening = br#"<html lang="zh-CN">"#;
    let lang = if language == "en" { "en" } else { "zh-CN" };
    let replacement = format!(r#"<html lang="{lang}" data-lang="{language}">"#);
    match html.windows(opening.len()).position(|w| w == opening) {
        Some(at) => [
            &html[..at],
            replacement.as_bytes(),
            &html[at + opening.len()..],
        ]
        .concat(),
        None => html.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_merge_fields_and_keep_unknown_ones() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("config");
        assert_eq!(state(&folder)["preferences"], json!({}));
        assert_eq!(language(&folder), "system");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join(FILE),
            br#"{"general":{"future":1},"later":{"x":true}}"#,
        )
        .unwrap();
        update(&folder, &json!({"general": {"language": "en"}})).unwrap();
        assert_eq!(
            read(&folder).unwrap(),
            *json!({"general": {"future": 1, "language": "en"}, "later": {"x": true}})
                .as_object()
                .unwrap()
        );
        assert_eq!(language(&folder), "en");
        for bad in [
            json!({"general": {"language": "fr"}}),
            json!({"general": {"typo": 1}}),
            json!({"nowhere": {"language": "en"}}),
            json!({"general": "en"}),
            json!([]),
        ] {
            assert!(update(&folder, &bad).is_err(), "{bad}");
        }
        assert_eq!(language(&folder), "en");
    }

    #[test]
    fn a_broken_file_reads_empty_and_is_replaced_by_the_next_change() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE), b"{oops").unwrap();
        assert!(state(dir.path())["error"].is_string());
        assert_eq!(language(dir.path()), "system");
        update(dir.path(), &json!({"general": {"language": "zh"}})).unwrap();
        assert_eq!(state(dir.path())["error"], Value::Null);
        assert_eq!(language(dir.path()), "zh");
    }

    #[test]
    fn pages_open_in_the_chosen_language() {
        let html = b"<!doctype html>\n<html lang=\"zh-CN\">\n<head>";
        let page = |language| String::from_utf8(localize(html, language)).unwrap();
        assert!(page("en").contains(r#"<html lang="en" data-lang="en">"#));
        assert!(page("zh").contains(r#"<html lang="zh-CN" data-lang="zh">"#));
        assert!(page("system").contains(r#"<html lang="zh-CN" data-lang="system">"#));
        assert_eq!(localize(b"<html>", "en"), b"<html>");
    }
}

//! The App's own settings, apart from the data root the service shares. They live in the
//! App's config folder, so a preview build keeps its own and never changes the release's.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub mod autostart;
pub mod general;
mod keep_awake;
pub mod runtime;

const FILE: &str = "preferences.json";

mod alerts;

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
        "general" => general::validate(field, value),
        "alerts" => alerts::validate(field, value),
        "updates" => updates(field, value),
        _ => Err(format!("未知的设置分区：{section}")),
    }
}

fn updates(field: &str, value: &Value) -> Result<(), String> {
    match field {
        "auto_check" => match value.as_bool() {
            Some(_) => Ok(()),
            None => Err("updates.auto_check 必须为 true 或 false".into()),
        },
        "check_hours" => match value.as_u64() {
            Some(4 | 12 | 24) => Ok(()),
            _ => Err("updates.check_hours 必须为 4、12 或 24".into()),
        },
        _ => Err(format!("未知的设置项：updates.{field}")),
    }
}

fn one_of(value: &Value, allowed: &[&str], name: &str) -> Result<(), String> {
    match value.as_str() {
        Some(v) if allowed.contains(&v) => Ok(()),
        _ => Err(format!("{name} 必须为 {}", allowed.join("、"))),
    }
}

fn validate_changes(changes: &Value) -> Result<(), String> {
    general::reject_autostart(crate::PREVIEW, changes)?;
    let changes = changes.as_object().ok_or("设置必须为 JSON 对象")?;
    for (section, fields) in changes {
        let fields = fields
            .as_object()
            .ok_or_else(|| format!("{section} 必须为 JSON 对象"))?;
        for (field, value) in fields {
            validate_field(section, field, value)?;
        }
    }
    Ok(())
}

/// Merge `{section: {field: value}}` into the saved document. Only the given fields change;
/// fields and sections this build does not know stay as they were.
pub fn update(dir: &Path, changes: &Value) -> Result<Value, String> {
    validate_changes(changes)?;
    let changes = changes.as_object().expect("validated object");
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
    crate::data_files::write_atomic(dir, FILE, &document)?;
    Ok(document)
}

/// The saved language choice; native menus and pages resolve `system` with the OS locale.
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

/// Attributes are validated before insertion; system theme is settled by CSS before paint.
/// Attributes a page writes after `lang` (such as `data-idle-blur`) are kept.
pub fn localize(html: &[u8], general: &Value) -> Vec<u8> {
    let opening = br#"<html lang="zh-CN""#;
    let language = general["language"].as_str().unwrap_or("system");
    let locale = sys_locale::get_locale().unwrap_or_default();
    let lang = match general::resolved_language(language, &locale) {
        "en" => "en",
        _ => "zh-CN",
    };
    let theme = general["theme"].as_str().unwrap_or("system");
    let motion = general["motion"].as_str().unwrap_or("system");
    let character_style = general["character_style"].as_str().unwrap_or("static");
    let battery_still = general["battery_still"].as_bool().unwrap_or(true);
    let pause_unfocused = general["pause_unfocused"].as_bool().unwrap_or(true);
    let on_battery = if crate::power::on_battery() {
        r#" data-on-battery="true""#
    } else {
        ""
    };
    let replacement = format!(
        r#"<html lang="{lang}" data-lang="{language}" data-theme="{theme}" data-motion="{motion}" data-character-style="{character_style}" data-battery-still="{battery_still}" data-pause-unfocused="{pause_unfocused}"{on_battery}"#
    );
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
mod tests;

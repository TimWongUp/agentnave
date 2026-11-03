//! Validation and defaults shared by the pages and native App integrations.
use serde_json::{Value, json};

pub fn validate(field: &str, value: &Value) -> Result<(), String> {
    let name = format!("general.{field}");
    match field {
        "language" => super::one_of(value, &["system", "zh", "en"], &name),
        "theme" => super::one_of(value, &["system", "light", "dark"], &name),
        "motion" => super::one_of(value, &["system", "reduce", "full"], &name),
        "tray_click" => super::one_of(value, &["panel", "office"], &name),
        "tray_font" => super::one_of(
            value,
            &["futura", "baskerville", "typewriter", "marker"],
            &name,
        ),
        "scale" if matches!(value.as_u64(), Some(90 | 100 | 115)) => Ok(()),
        "scale" => Err(format!("{name} 必须为 90、100 或 115")),
        "autostart" | "open_on_start" | "tray_count" | "keep_awake" if value.is_boolean() => Ok(()),
        "autostart" | "open_on_start" | "tray_count" | "keep_awake" => {
            Err(format!("{name} 必须为布尔值"))
        }
        _ => Err(format!("未知的设置项：{name}")),
    }
}

pub fn effective(document: &Value, autostart: bool) -> Value {
    let mut result = json!({
        "language": "system", "theme": "system", "motion": "system", "scale": 100,
        "autostart": autostart, "open_on_start": true, "tray_click": "panel",
        "tray_count": true, "tray_font": "futura", "keep_awake": false,
    });
    if let Some(saved) = document["general"].as_object() {
        for (field, value) in saved {
            if field != "autostart" && validate(field, value).is_ok() {
                result[field] = value.clone();
            }
        }
    }
    result
}

pub fn current() -> Value {
    let document = Value::Object(super::read(&super::dir()).unwrap_or_default());
    effective(&document, false)
}

pub fn reject_autostart(preview: bool, changes: &Value) -> Result<(), String> {
    if preview && changes["general"].get("autostart").is_some() {
        Err("预览版不可修改开机启动".into())
    } else {
        Ok(())
    }
}

pub fn resolved_language(language: &str, locale: &str) -> &'static str {
    match language {
        "en" => "en",
        "zh" => "zh",
        _ if locale.to_ascii_lowercase().starts_with("en") => "en",
        _ => "zh",
    }
}

pub fn text<'a>(zh: &'a str, en: &'a str) -> &'a str {
    let language = super::language(&super::dir());
    if resolved_language(&language, &sys_locale::get_locale().unwrap_or_default()) == "en" {
        en
    } else {
        zh
    }
}

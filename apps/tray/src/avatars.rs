//! Appearance only: CLI marks saved by earlier versions. Model logos are fixed official icons;
//! older family and model entries stay readable but are no longer shown.
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

const LIMIT: usize = 4 * 1024 * 1024;

pub fn read(root: &Path) -> Result<Value, String> {
    let path = root.join("avatars.json");
    match fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e.to_string()),
        Ok(meta) if meta.len() > LIMIT as u64 => return Err("头像配置过大".into()),
        _ => {}
    }
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("头像配置损坏：{e}"))?;
    let Some(entries) = value.as_object() else {
        return Err("头像配置必须为对象".into());
    };
    for (key, image) in entries {
        validate(key, image.as_str().ok_or("头像必须为字符串")?)?;
    }
    Ok(value)
}

fn validate(key: &str, image: &str) -> Result<(), String> {
    let parts: Vec<String> = serde_json::from_str(key).map_err(|_| "头像标识无效")?;
    if parts.len() != 3
        || !["family", "host", "model"].contains(&parts[0].as_str())
        || parts[1].is_empty()
        || key.len() > 1024
    {
        return Err("头像标识无效".into());
    }
    let encoded = image
        .strip_prefix("data:image/png;base64,")
        .ok_or("头像必须为 PNG 图片")?;
    if encoded.len() > 100_000
        || !encoded.starts_with("iVBORw0KGgo")
        || !encoded
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"+/=".contains(&ch))
    {
        return Err("头像格式无效或超过大小限制".into());
    }
    Ok(())
}

pub fn asset(name: &str) -> Option<(&'static str, &'static [u8])> {
    macro_rules! asset {
        ($name:literal) => {
            include_bytes!(concat!("../../../src/agentnave/dashboard_assets/", $name))
        };
    }
    Some(match name {
        "avatars.js" => ("text/javascript; charset=utf-8", asset!("avatars.js")),
        "avatar-claude.svg" => ("image/svg+xml", asset!("avatar-claude.svg")),
        "avatar-gpt.svg" => ("image/svg+xml", asset!("avatar-gpt.svg")),
        "avatar-gemini.svg" => ("image/svg+xml", asset!("avatar-gemini.svg")),
        "avatar-grok.svg" => ("image/svg+xml", asset!("avatar-grok.svg")),
        "avatar-deepseek.svg" => ("image/svg+xml", asset!("avatar-deepseek.svg")),
        "avatar-glm.svg" => ("image/svg+xml", asset!("avatar-glm.svg")),
        "avatar-qwen.svg" => ("image/svg+xml", asset!("avatar-qwen.svg")),
        "avatar-kimi.svg" => ("image/svg+xml", asset!("avatar-kimi.svg")),
        "avatar-minimax.svg" => ("image/svg+xml", asset!("avatar-minimax.svg")),
        _ => return None,
    })
}

//! Appearance only: the tray writes, both Studio views read the shared data directory.
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use crate::data::Response;
use serde_json::{Value, json};

static WRITE: Mutex<()> = Mutex::new(());
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

pub fn request(root: &Path, method: &str, action: Option<&str>, body: &[u8]) -> Response {
    if method != "POST" {
        return Response::error(405, "此操作只接受 POST");
    }
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    if body.len() > 110_000 {
        return Response::error(400, "头像超过大小限制");
    }
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return Response::error(400, "请求必须为 JSON");
    };
    let Some(key) = payload["key"].as_str() else {
        return Response::error(400, "缺少头像标识");
    };
    let image = match payload.get("image") {
        Some(Value::Null) => None,
        Some(Value::String(image)) => Some(image.as_str()),
        _ => return Response::error(400, "头像必须为图片或 null"),
    };
    // Validate reset keys as well, before creating any directories.
    if let Err(error) = validate(key, image.unwrap_or("data:image/png;base64,iVBORw0KGgo=")) {
        return Response::error(400, &error);
    }
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut document = match read(root) {
        Ok(value) => value,
        Err(error) => return Response::error(500, &error),
    };
    let entries = document.as_object_mut().expect("read validated object");
    if let Some(image) = image {
        entries.insert(key.into(), image.into());
    } else {
        entries.remove(key);
    }
    let bytes = match serde_json::to_vec(&document) {
        Ok(bytes) if bytes.len() <= LIMIT => bytes,
        _ => return Response::error(400, "头像配置过大"),
    };
    let save = || -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root)?;
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(root.join("avatars.json"))?;
        Ok(())
    };
    match save() {
        Ok(()) => Response::json(&document),
        Err(error) => Response::error(500, &format!("无法保存头像：{error}")),
    }
}

pub fn asset(name: &str) -> Option<(&'static str, &'static [u8])> {
    macro_rules! asset {
        ($name:literal) => {
            include_bytes!(concat!("../../../src/agentnave/dashboard_assets/", $name))
        };
    }
    Some(match name {
        "avatars.js" => ("text/javascript; charset=utf-8", asset!("avatars.js")),
        "avatar-claude.png" => ("image/png", asset!("avatar-claude.png")),
        "avatar-gpt.svg" => ("image/svg+xml", asset!("avatar-gpt.svg")),
        "avatar-gemini.png" => ("image/png", asset!("avatar-gemini.png")),
        "avatar-grok.png" => ("image/png", asset!("avatar-grok.png")),
        "avatar-deepseek.ico" => ("image/x-icon", asset!("avatar-deepseek.ico")),
        "avatar-glm.svg" => ("image/svg+xml", asset!("avatar-glm.svg")),
        "avatar-qwen.png" => ("image/png", asset!("avatar-qwen.png")),
        "avatar-kimi.png" => ("image/png", asset!("avatar-kimi.png")),
        "avatar-minimax.ico" => ("image/x-icon", asset!("avatar-minimax.ico")),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn avatars_require_explicit_write_and_preserve_other_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        let reader = crate::data::Reader::new(root.clone());
        let key = r#"["model","pi","gpt-6"]"#;
        let payload =
            json!({"key": key, "image": "data:image/png;base64,iVBORw0KGgo="}).to_string();
        assert_eq!(
            reader
                .request("GET", "/app/avatars", Some("1"), payload.as_bytes())
                .status,
            405
        );
        assert_eq!(
            reader
                .request("POST", "/app/avatars", None, payload.as_bytes())
                .status,
            403
        );
        let bad = json!({"key":key,"image":"data:image/svg+xml,<svg/>"}).to_string();
        assert_eq!(
            reader
                .request("POST", "/app/avatars", Some("1"), bad.as_bytes())
                .status,
            400
        );
        assert!(!root.exists());
        assert_eq!(
            reader
                .request("POST", "/app/avatars", Some("1"), payload.as_bytes())
                .status,
            200
        );
        let other =
            json!({"key": r#"["host","codex",""]"#, "image":"data:image/png;base64,iVBORw0KGgo="})
                .to_string();
        assert_eq!(
            request(&root, "POST", Some("1"), other.as_bytes()).status,
            200
        );
        let reset = json!({"key":key,"image":null}).to_string();
        assert_eq!(
            request(&root, "POST", Some("1"), reset.as_bytes()).status,
            200
        );
        assert_eq!(read(&root).unwrap().as_object().unwrap().len(), 1);
        fs::write(root.join("avatars.json"), "broken").unwrap();
        assert_eq!(
            request(&root, "POST", Some("1"), payload.as_bytes()).status,
            500
        );
        assert_eq!(
            fs::read_to_string(root.join("avatars.json")).unwrap(),
            "broken"
        );
    }
}

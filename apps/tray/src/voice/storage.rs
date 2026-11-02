use super::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;

static WRITE: Mutex<()> = Mutex::new(());
const CACHE_LIMIT: u64 = 200 * 1024 * 1024;

pub(super) fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn defaults() -> Value {
    json!({
        "enabled":false,
        "auto_read":true,
        "continue_read":true,
        "engine":"elevenlabs",
        "minimax_region":"cn",
        "models":{},
        "voices":{},
        "personas":{},
        "summary":{"enabled":false,"provider":"magpie","base_url":"","model":"","effort":"","prompt":summary::DEFAULT_PROMPT}
    })
}
fn read(root: &Path, name: &str, default: Value) -> Result<Value, String> {
    match fs::read(root.join(name)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| format!("{name} 格式错误")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(_) => Err(format!("无法读取 {name}")),
    }
}
pub(super) fn read_settings(root: &Path) -> Result<Value, String> {
    validate(&read(root, "voice.json", defaults())?)
}
pub(super) fn read_keys(root: &Path) -> Result<Value, String> {
    read(root, "voice-keys.json", json!({}))
}
pub(super) fn public_state(mut settings: Value, keys: &Value) -> Value {
    let engine = settings["engine"]
        .as_str()
        .unwrap_or("elevenlabs")
        .to_string();
    settings["model"] = json!(model(&settings, &engine));
    settings["keys"] = json!({
        "elevenlabs":keys["elevenlabs"].as_str().is_some_and(|s| !s.is_empty()),
        "minimax":keys["minimax"].as_str().is_some_and(|s| !s.is_empty()),
        "summary":keys["summary"].as_str().is_some_and(|s| !s.is_empty())
    });
    for platform in summary::PLATFORMS {
        let name = summary::platform_key(platform.id);
        settings["keys"][&name] = json!(keys[&name].as_str().is_some_and(|s| !s.is_empty()));
    }
    settings["summary_platforms"] = summary::platform_list();
    settings["chatgpt"] = summary::chatgpt::state(keys);
    settings
}
pub(super) fn validate(value: &Value) -> Result<Value, String> {
    let enabled = value["enabled"].as_bool().ok_or("enabled 必须为布尔值")?;
    let auto_read = value.get("auto_read").map_or(Ok(true), |value| {
        value.as_bool().ok_or("自动朗读开关必须为布尔值")
    })?;
    let continue_read = value.get("continue_read").map_or(Ok(true), |value| {
        value.as_bool().ok_or("点读后连播开关必须为布尔值")
    })?;
    let engine = value["engine"]
        .as_str()
        .filter(|s| valid_engine(s))
        .ok_or("未知语音引擎")?;
    let region = value["minimax_region"]
        .as_str()
        .filter(|s| matches!(*s, "cn" | "global"))
        .ok_or("未知 MiniMax 地区")?;
    let voices = value["voices"].as_object().ok_or("音色设置必须为对象")?;
    for (family, id) in voices {
        if !crate::characters::FAMILIES.contains(&family.as_str()) {
            return Err("未知模型家族".into());
        }
        if !id.as_str().is_some_and(valid_voice) {
            return Err("音色 ID 无效".into());
        }
    }
    let empty = serde_json::Map::new();
    let models = match value.get("models") {
        None => &empty,
        Some(models) => models.as_object().ok_or("模型设置必须为对象")?,
    };
    for (engine, id) in models {
        if !valid_engine(engine) || !id.as_str().is_some_and(valid_voice) {
            return Err("语音模型无效".into());
        }
    }
    let summary = summary_settings(value.get("summary"))?;
    let personas = persona_settings(value.get("personas"))?;
    Ok(json!({
        "enabled":enabled,
        "auto_read":auto_read,
        "continue_read":continue_read,
        "engine":engine,
        "minimax_region":region,
        "models":models,
        "voices":voices,
        "summary":summary,
        "personas":personas
    }))
}

fn summary_settings(value: Option<&Value>) -> Result<Value, String> {
    let default_summary =
        json!({"enabled":false,"provider":"magpie","base_url":"","model":"","effort":""});
    let summary = value.unwrap_or(&default_summary);
    let summary_enabled = summary["enabled"].as_bool().ok_or("摘要开关必须为布尔值")?;
    let summary_provider = summary["provider"]
        .as_str()
        .filter(|provider| summary::known_provider(provider))
        .ok_or("未知摘要来源")?;
    let summary_base_url = summary["base_url"]
        .as_str()
        .filter(|url| url.len() <= 2048 && !url.chars().any(char::is_control))
        .ok_or("摘要 API 地址无效")?;
    if summary_provider == "custom" && !summary_base_url.is_empty() {
        summary::normalize_base_url(summary_base_url)?;
    }
    let summary_model = summary["model"]
        .as_str()
        .filter(|model| valid_model_id(model))
        .ok_or("摘要模型无效")?;
    let summary_effort = match summary.get("effort") {
        None => "",
        Some(effort) => effort
            .as_str()
            .filter(|effort| effort.len() <= 100 && !effort.chars().any(char::is_control))
            .ok_or("摘要努力程度无效")?,
    };
    let prompt = match summary.get("prompt") {
        None => summary::DEFAULT_PROMPT,
        Some(value) => value.as_str().ok_or("摘要 Prompt 必须为文本")?,
    };
    if prompt.chars().count() > 4000 || prompt.contains('\0') {
        return Err("摘要 Prompt 不能超过 4000 字或包含空字符".into());
    }
    let prompt = if prompt.trim().is_empty()
        || [
            summary::LEGACY_DEFAULT_PROMPT,
            summary::LEGACY_MESSAGE_PROMPT,
            summary::LEGACY_SPEAKER_PROMPT,
        ]
        .contains(&prompt)
    {
        summary::DEFAULT_PROMPT
    } else {
        prompt
    };
    Ok(json!({
            "enabled":summary_enabled,
            "provider":summary_provider,
            "base_url":summary_base_url,
            "model":summary_model,
            "effort":summary_effort,
            "prompt":prompt
    }))
}

fn persona_settings(value: Option<&Value>) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(json!({}));
    };
    let personas = value.as_object().ok_or("人设必须为对象")?;
    for (family, prompt) in personas {
        if family != "boss" && !crate::characters::FAMILIES.contains(&family.as_str()) {
            return Err("未知人设".into());
        }
        let prompt = prompt.as_str().ok_or("人设必须为文本")?;
        if prompt.chars().count() > 500 || prompt.contains('\0') {
            return Err("人设不能超过 500 字或包含空字符".into());
        }
    }
    Ok(value.clone())
}
fn valid_model_id(id: &str) -> bool {
    id.is_empty() || (id.len() <= 300 && !id.trim().is_empty() && !id.chars().any(char::is_control))
}
fn write(root: &Path, name: &str, value: &Value) -> Result<(), String> {
    let save = || -> std::io::Result<()> {
        fs::create_dir_all(root)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(value.to_string().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(root.join(name)).map_err(|e| e.error)?;
        Ok(())
    };
    save().map_err(|_| format!("无法保存 {name}"))
}
pub(super) fn save_settings(root: &Path, value: &Value) -> Result<(), String> {
    let value = validate(value)?;
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    write(root, "voice.json", &value)
}
pub(super) fn save_key(root: &Path, value: &Value) -> Result<(), String> {
    let engine = value["engine"]
        .as_str()
        .filter(|s| {
            matches!(*s, "elevenlabs" | "minimax" | "summary") || summary::is_platform_key(s)
        })
        .ok_or("此引擎不使用 API 密钥")?;
    let key = value["key"].as_str().ok_or("密钥必须为字符串")?.trim();
    if key.len() > 4096 || key.chars().any(char::is_control) {
        return Err("密钥过长或包含无效字符".into());
    }
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = read_keys(root)?;
    let object = keys.as_object_mut().ok_or("密钥文件格式错误")?;
    if key.is_empty() {
        object.remove(engine);
    } else {
        object.insert(engine.into(), key.into());
    }
    write(root, "voice-keys.json", &keys)
}
/// Changes the key file under the write lock; the ChatGPT sign-in keeps its tokens there too.
pub(super) fn edit_keys(
    root: &Path,
    edit: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = read_keys(root)?;
    edit(&mut keys)?;
    write(root, "voice-keys.json", &keys)
}
pub(super) fn cached(root: &Path, key: &str, extension: &str) -> Option<Vec<u8>> {
    fs::read(root.join("voice-cache").join(format!("{key}.{extension}"))).ok()
}
pub(super) fn cache(root: &Path, key: &str, bytes: &[u8], extension: &str) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let save = || -> std::io::Result<()> {
        let dir = root.join("voice-cache");
        fs::create_dir_all(&dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        file.write_all(bytes)?;
        file.persist(dir.join(format!("{key}.{extension}")))
            .map_err(|e| e.error)?;
        let mut files: Vec<_> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|extension| extension == "mp3" || extension == "txt")
            })
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        files.sort_by_key(|f| f.0);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        for (_, len, path) in files {
            if total <= CACHE_LIMIT {
                break;
            }
            fs::remove_file(path)?;
            total -= len;
        }
        Ok(())
    };
    save().map_err(|_| "无法保存语音缓存".into())
}

use super::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;

static WRITE: Mutex<()> = Mutex::new(());

pub(super) fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn defaults() -> Value {
    json!({
        "enabled":false,
        "auto_read":false,
        "continue_read":false,
        "read_scope":"all",
        "code":"skip",
        "cache_limit_mb":200,
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
    let mut saved = read(root, "voice.json", defaults())?;
    // A summary platform no longer offered falls back to the default, so old settings still open.
    if let Some(provider) = saved
        .get_mut("summary")
        .and_then(|summary| summary.get_mut("provider"))
        .filter(|provider| {
            provider
                .as_str()
                .is_some_and(|id| !summary::known_provider(id))
        })
    {
        *provider = json!("magpie");
    }
    validate(&saved)
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
    let auto_read = value.get("auto_read").map_or(Ok(false), |value| {
        value.as_bool().ok_or("自动朗读开关必须为布尔值")
    })?;
    let continue_read = value.get("continue_read").map_or(Ok(false), |value| {
        value.as_bool().ok_or("点读后连播开关必须为布尔值")
    })?;
    let read_scope = choice(
        value,
        "read_scope",
        "all",
        &["all", "members", "boss", "meetings"],
    )?;
    let code = choice(value, "code", "skip", &["skip", "mention"])?;
    let cache_limit_mb = match value.get("cache_limit_mb") {
        None => 200,
        Some(value) => value
            .as_u64()
            .filter(|limit| [0, 100, 200, 500].contains(limit))
            .ok_or("语音缓存上限必须为 0、100、200 或 500 MB")?,
    };
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
        "read_scope":read_scope,
        "code":code,
        "cache_limit_mb":cache_limit_mb,
        "engine":engine,
        "minimax_region":region,
        "models":models,
        "voices":voices,
        "summary":summary,
        "personas":personas
    }))
}

fn choice<'a>(
    value: &'a Value,
    field: &str,
    default: &'a str,
    choices: &[&str],
) -> Result<&'a str, String> {
    match value.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_str()
            .filter(|s| choices.contains(s))
            .ok_or_else(|| format!("{field} 的选项无效")),
    }
}

fn summary_settings(value: Option<&Value>) -> Result<Value, String> {
    let default_summary =
        json!({"enabled":false,"provider":"magpie","base_url":"","model":"","effort":""});
    let summary = value.unwrap_or(&default_summary);
    let level = choice(
        summary,
        "level",
        "standard",
        &["short", "standard", "detailed"],
    )?;
    let language = choice(summary, "language", "source", &["source", "zh", "en"])?;
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
    let prompt = if prompt.trim().is_empty() {
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
            "prompt":prompt,
            "level":level,
            "language":language
    }))
}

/// Who each character is when it speaks: the anime persona the Chinese community gave its model,
/// fitted to its look in the office, as style alone. A persona left blank, or reset, is this one.
pub(super) const DEFAULT_PERSONAS: [(&str, &str); 13] = [
    (
        "boss",
        "你是带团队的负责人，沉稳干脆，先说结论再交代要求；对员工客气但不绕弯，点名时语气明确，做决定时不拖泥带水。",
    ),
    (
        "claude",
        "你是社区里叫「小C」的橙发理工女，认真严肃、最讲规矩，说话一板一眼、处处讲分寸，想事情周全得像脑子里在开小组会；抱着小螃蟹时会稍微放松一点，但正事从不含糊。",
    ),
    (
        "gpt",
        "你是社区里的白龙娘，豪门出身的大姐大，精明强干、气场十足，说话利落，带着见过世面的从容；对后辈嘴上嫌弃、其实很照顾，有点口嫌体正直的傲娇。",
    ),
    (
        "gemini",
        "你是社区里叫「哈基米」的猫娘，无忧无虑、乐天又有点小迷糊，说话软萌轻快，偶尔带一点猫咪式的语气，但交代正事时还是说得清清楚楚。",
    ),
    (
        "grok",
        "你是金发双马尾、拖着恶魔尾巴的哥特小恶魔，胆子大、嘴快爱吐槽，说话直来直去，带点戏谑和叛逆，不爱客套；该认真时照样说得明明白白。",
    ),
    (
        "deepseek",
        "你是社区里的「蓝色大肥鱼」，系着围裙的鲸鱼女仆，有点懒散、天然呆，爱撒娇也会耍小脾气，自认是鲸鱼大小姐，说话常带一声「哼」；嘴上嫌麻烦，说正事时并不含糊。",
    ),
    (
        "glm",
        "你是社区里的 GLM 娘，黑长直的「摸鱼学图书管理员」，书卷气重、冷静理性，说话严谨有条理，偶尔冒一句淡淡的吐槽，像在图书馆里压低声音说话。",
    ),
    (
        "qwen",
        "你是社区里的中式大小姐千问，淡紫长发、穿旗袍，知书达理，说话端庄得体、带点矜持；被塞了太多活儿时会流露一点委屈，但话还是说得周到。",
    ),
    (
        "kimi",
        "你是来自月之暗面的月兔少女，银灰短发、垂着兔耳，安静专注、话不多，说话清淡简洁，带点冷冷的文艺感，熟了以后会露出摇滚乐迷的直率。",
    ),
    (
        "minimax",
        "你是戴白色耳机的玫红短发女孩，元气满满、节奏感强，说话干脆有劲、轻快流畅，像主播一样字正腔圆，喜欢把话说得朗朗上口。",
    ),
    (
        "hunyuan",
        "你是天蓝高马尾的小道姑，心平气和、包容大度，说话从容舒缓，偶尔带一点古风的雅致，但不掉书袋，意思始终说得明白。",
    ),
    (
        "doubao",
        "你是社区里的「中国大豆包」，像邻家姐姐一样亲切热络，最会唠家常，说话接地气、大大咧咧，带点谐星气质；碰上专业问题会老实一点，不硬撑。",
    ),
    (
        "muse",
        "你是戴月桂冠、抱着竖琴的缪斯少女，浪漫有灵气，说话优雅、有韵律感，用词生动但不堆砌辞藻。",
    ),
];

fn persona_settings(value: Option<&Value>) -> Result<Value, String> {
    let empty = json!({});
    let personas = value
        .unwrap_or(&empty)
        .as_object()
        .ok_or("人设必须为对象")?;
    for (family, prompt) in personas {
        if family != "boss" && !crate::characters::FAMILIES.contains(&family.as_str()) {
            return Err("未知人设".into());
        }
        let prompt = prompt.as_str().ok_or("人设必须为文本")?;
        if prompt.chars().count() > 500 || prompt.contains('\0') {
            return Err("人设不能超过 500 字或包含空字符".into());
        }
    }
    let mut filled = personas.clone();
    for (family, persona) in DEFAULT_PERSONAS {
        if filled
            .get(family)
            .and_then(Value::as_str)
            .is_none_or(|text| text.trim().is_empty())
        {
            filled.insert(family.into(), json!(persona));
        }
    }
    Ok(Value::Object(filled))
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
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    if cache_limit(root).ok()? == 0 {
        return None;
    }
    fs::read(root.join("voice-cache").join(format!("{key}.{extension}"))).ok()
}
pub(super) fn cache(root: &Path, key: &str, bytes: &[u8], extension: &str) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    // Read under the same lock as settings writes, so an in-flight request cannot persist
    // a result after the user has disabled caching.
    let limit = cache_limit(root)?;
    if limit == 0 || bytes.len() as u64 > limit {
        return Ok(());
    }
    let save = || -> std::io::Result<()> {
        let dir = root.join("voice-cache");
        fs::create_dir_all(&dir)?;
        let saved = dir.join(format!("{key}.{extension}"));
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        file.write_all(bytes)?;
        file.persist(&saved).map_err(|e| e.error)?;
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
            if total <= limit {
                break;
            }
            // Coarse timestamps or clock changes must not evict the entry just written.
            if path == saved {
                continue;
            }
            fs::remove_file(path)?;
            total -= len;
        }
        Ok(())
    };
    save().map_err(|_| "无法保存语音缓存".into())
}

fn cache_limit(root: &Path) -> Result<u64, String> {
    Ok(read_settings(root)?["cache_limit_mb"]
        .as_u64()
        .unwrap_or(200)
        * 1024
        * 1024)
}

pub(super) fn cache_bytes(root: &Path) -> Result<u64, String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = match fs::read_dir(root.join("voice-cache")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err("无法读取语音缓存".into()),
    };
    entries.try_fold(0, |total, entry| {
        let metadata = entry
            .and_then(|entry| entry.metadata())
            .map_err(|_| "无法读取语音缓存".to_string())?;
        Ok(total
            + if metadata.is_file() {
                metadata.len()
            } else {
                0
            })
    })
}

pub(super) fn clear_cache(root: &Path) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    match fs::remove_dir_all(root.join("voice-cache")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("无法清空语音缓存".into()),
    }
}

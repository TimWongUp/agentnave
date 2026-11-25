//! Meeting speech is opt-in. Only Rust owns credentials and calls cloud synthesis APIs.
mod elevenlabs;
#[cfg(test)]
mod live;
mod minimax;
pub mod player;
mod scene;
mod storage;
mod summary;
#[cfg(test)]
mod tests;
pub mod text;

use crate::response::Response;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// A summary hears at most this much of a very long message.
const SUMMARY_INPUT_CHARS: usize = 6_000;

#[derive(Clone)]
struct Voice {
    id: String,
    name: String,
}
static CATALOGS: LazyLock<Mutex<HashMap<String, Vec<Voice>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn root(shared: &Path) -> PathBuf {
    if crate::PREVIEW {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("com.agentnave.preview")
            .join("voice")
    } else {
        shared.to_path_buf()
    }
}

// Cargo enables both ring (updater) and aws-lc (reqwest); keep HTTP clients on the
// updater's explicit provider instead of relying on ambiguous automatic selection.
fn initialize_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

static HTTP_CLIENT: LazyLock<Result<reqwest::blocking::Client, String>> = LazyLock::new(|| {
    initialize_tls();
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法创建语音网络连接".into())
});

/// One shared client, so repeated speech keeps reusing the same connections.
fn client() -> Result<reqwest::blocking::Client, String> {
    HTTP_CLIENT.clone()
}

fn http(request: reqwest::blocking::RequestBuilder, key: &str) -> Result<Vec<u8>, String> {
    let response = request
        .send()
        .map_err(|_| "语音请求失败，请检查网络或代理".to_string())?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|_| "无法读取语音响应".to_string())?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes);
        let safe = if key.is_empty() {
            detail.to_string()
        } else {
            detail.replace(key, "[密钥]")
        };
        return Err(format!(
            "语音服务返回 {status}：{}",
            safe.chars().take(500).collect::<String>()
        ));
    }
    Ok(bytes.to_vec())
}

fn http_json(
    request: reqwest::blocking::RequestBuilder,
    body: &Value,
    key: &str,
) -> Result<Value, String> {
    let bytes = http(
        request
            .header("Content-Type", "application/json")
            .body(body.to_string()),
        key,
    )?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "语音服务返回了无效 JSON".to_string())?;
    Ok(value)
}

fn valid_engine(engine: &str) -> bool {
    matches!(engine, "elevenlabs" | "minimax")
}
fn valid_voice(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// The model a synthesis uses: the user's pick, else the engine's default.
fn model<'a>(settings: &'a Value, engine: &str) -> &'a str {
    settings["models"][engine]
        .as_str()
        .unwrap_or(if engine == "minimax" {
            minimax::DEFAULT_MODEL
        } else {
            elevenlabs::DEFAULT_MODEL
        })
}

fn catalog(settings: &Value, keys: &Value, engine: &str, kind: &str) -> Result<Vec<Voice>, String> {
    let key = keys[engine]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("请先保存所选引擎的 API 密钥")?;
    let cache_key =
        storage::hash(&json!([engine, kind, settings["minimax_region"], key]).to_string());
    let cached = {
        let cache = CATALOGS.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(&cache_key).cloned()
    };
    if let Some(voices) = cached {
        return Ok(voices);
    }
    let voices = match (engine, kind) {
        ("elevenlabs", "models") => elevenlabs::models(key)?,
        ("elevenlabs", _) => elevenlabs::voices(key)?,
        ("minimax", _) => {
            minimax::voices(key, settings["minimax_region"].as_str().unwrap_or("cn"))?
        }
        _ => return Err("未知语音引擎".into()),
    };
    if voices.is_empty() {
        return Err("语音服务没有返回可用音色".into());
    }
    CATALOGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cache_key, voices.clone());
    Ok(voices)
}

fn listed(items: &[Voice]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|v| json!({"id":v.id,"name":v.name}))
            .collect(),
    )
}

pub fn get(shared: &Path, path: &str) -> Response {
    if path == "/app/voice/summary/models" {
        return Response::error(405, "此操作只接受 POST");
    }
    let root = root(shared);
    let result: Result<Value, String> = (|| {
        if path == "/app/voice/cache" {
            return Ok(json!({"bytes": storage::cache_bytes(&root)?}));
        }
        let settings = storage::read_settings(&root)?;
        let keys = storage::read_keys(&root)?;
        if path == "/app/voice" {
            return Ok(storage::public_state(settings, &keys));
        }
        let (kind, engine) = path
            .strip_prefix("/app/voice/voices/")
            .map(|engine| ("voices", engine))
            .or_else(|| {
                path.strip_prefix("/app/voice/models/")
                    .map(|engine| ("models", engine))
            })
            .filter(|(_, engine)| valid_engine(engine))
            .ok_or("未知语音引擎")?;
        if (kind, engine) == ("models", "minimax") {
            return Ok(listed(&minimax::models()));
        }
        // Catalogs never contact the network while voice is disabled.
        if settings["enabled"] != true {
            return Err("请先开启会议语音再读取音色".into());
        }
        Ok(listed(&catalog(&settings, &keys, engine, kind)?))
    })();
    match result {
        Ok(value) => Response::json(&value),
        Err(error) => Response::error(400, &error),
    }
}

pub fn request(
    shared: &Path,
    path: &str,
    method: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if method == "GET" {
        return get(shared, path);
    }
    if method != "POST" {
        return Response::error(405, "此操作只接受 POST");
    }
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    if body.len() > 200_000 {
        return Response::error(400, "语音请求不能超过 200000 字节");
    }
    let payload: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) if path == "/app/voice/cache/clear" && body.is_empty() => json!({}),
        Err(_) => return Response::error(400, "语音请求必须为 JSON 对象"),
    };
    let root = root(shared);
    let result = match path {
        "/app/voice" => storage::save_settings(&root, &payload).map(|()| get(shared, "/app/voice")),
        "/app/voice/key" => storage::save_key(&root, &payload).map(|()| {
            CATALOGS.lock().unwrap_or_else(|e| e.into_inner()).clear();
            get(shared, "/app/voice")
        }),
        "/app/voice/speak" => speak(&root, &payload),
        "/app/voice/cache/clear" => {
            storage::clear_cache(&root).map(|()| Response::json(&json!({"cleared": true})))
        }
        "/app/voice/summary/models" => summary_models(&root),
        "/app/voice/chatgpt/start" => {
            summary::chatgpt::start(&root).map(|()| get(shared, "/app/voice"))
        }
        "/app/voice/chatgpt/sign-out" => {
            summary::chatgpt::sign_out(&root).map(|()| get(shared, "/app/voice"))
        }
        _ => return Response::error(404, "没有此语音路由"),
    };
    match result {
        Ok(response) => response,
        Err(error) => Response::error(400, &error),
    }
}

fn summary_models(root: &Path) -> Result<Response, String> {
    let settings = storage::read_settings(root)?;
    if settings["enabled"] != true {
        return Err("请先开启会议语音再读取摘要模型".into());
    }
    let keys = storage::read_keys(root)?;
    let key = summary::key_of(&keys, &settings["summary"]);
    let models = summary::models(root, &settings["summary"], key)?;
    Ok(Response::json(&summary::listed(&models)))
}

fn speak(root: &Path, payload: &Value) -> Result<Response, String> {
    let settings = storage::read_settings(root)?;
    if settings["enabled"] != true {
        return Err("语音播报尚未开启".into());
    }
    let family = match payload.get("family") {
        None | Some(Value::Null) => "default",
        Some(Value::String(s))
            if s == "default" || crate::characters::FAMILIES.contains(&s.as_str()) =>
        {
            s.as_str()
        }
        _ => return Err("未知模型家族".into()),
    };
    let boss = match payload.get("boss") {
        None => false,
        Some(Value::Bool(boss)) => *boss,
        _ => return Err("boss 必须为布尔值".into()),
    };
    let (text, to_summarize) = source_text(payload, &settings)?;
    if text.is_empty() && to_summarize.is_none() {
        return Ok(no_audio());
    }
    let engine = settings["engine"].as_str().unwrap_or("elevenlabs");
    let keys = storage::read_keys(root)?;
    let key = keys[engine]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or("请先保存所选引擎的 API 密钥")?;
    let scene = scene::Scene::of(payload, boss);
    let text = match to_summarize {
        Some(full_text) => {
            let summary_key = summary::key_of(&keys, &settings["summary"]);
            let mut options = settings["summary"].clone();
            options["persona"] = json!(persona_for(&settings, family, boss));
            options["identity"] = json!(scene.identity());
            options["code"] = settings["code"].clone();
            // A failed summary stops the speech: speaking the opening instead would be a
            // different message from the one the user asked to hear.
            summary::summarize(root, &full_text, &options, summary_key)
                .map_err(|error| format!("摘要失败：{error}"))?
        }
        None => text,
    };
    let voice = select_voice(payload, family, &settings, &keys, engine)?;
    let model = model(&settings, engine);
    let cache_key = audio_key(&settings, engine, &voice, &text);
    if let Some(audio) = storage::cached(root, &cache_key, "mp3") {
        return Ok(Response::ok("audio/mpeg", audio).with_notice(None));
    }
    let audio = match engine {
        "elevenlabs" => elevenlabs::synthesize(key, model, &voice, &text),
        "minimax" => minimax::synthesize(
            key,
            model,
            &voice,
            &text,
            settings["minimax_region"].as_str().unwrap_or("cn"),
        ),
        _ => Err("未知语音引擎".into()),
    }?;
    if audio.is_empty() {
        return Err("语音服务未返回音频".into());
    }
    storage::cache(root, &cache_key, &audio, "mp3")?;
    Ok(Response::ok("audio/mpeg", audio).with_notice(None))
}

fn persona_for(settings: &Value, family: &str, boss: bool) -> String {
    let character = settings["personas"][family]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let boss_role = boss
        .then(|| settings["personas"]["boss"].as_str())
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (character, boss_role) {
        (Some(character), Some(role)) => format!(
            "{character}\n作为 boss 时的角色设定补充（只补充当前作为 boss 的定位，不覆盖上面的角色人设与口吻）：{role}"
        ),
        (Some(character), None) => character.to_string(),
        (None, Some(role)) => role.to_string(),
        (None, None) => String::new(),
    }
}

fn audio_key(settings: &Value, engine: &str, voice: &str, text: &str) -> String {
    storage::hash(
        &json!([
            engine,
            model(settings, engine),
            voice,
            settings["minimax_region"],
            text
        ])
        .to_string(),
    )
}

fn source_text(payload: &Value, settings: &Value) -> Result<(String, Option<String>), String> {
    let raw = payload["text"].as_str().ok_or("缺少发言文本")?;
    let full = payload["full"].as_bool().ok_or("full 必须为布尔值")?;
    let summary_enabled = settings["summary"]["enabled"] == true;
    let code = settings["code"].as_str().unwrap_or("skip");
    let language = if summary_enabled && !full {
        settings["summary"]["language"].as_str().unwrap_or("source")
    } else {
        "source"
    };
    // With summaries on, every message is spoken through the summary model, which knows who
    // speaks to whom; only a full read, like a voice sample, is read as written.
    if !summary_enabled || full {
        return Ok((text::clean_with_code(raw, full, code, language), None));
    }
    let cleaned = text::clean_with_code(raw, true, code, language);
    if cleaned.is_empty() {
        return Ok((cleaned, None));
    }
    Ok((
        String::new(),
        Some(cleaned.chars().take(SUMMARY_INPUT_CHARS).collect()),
    ))
}

fn no_audio() -> Response {
    let mut response = Response::ok("audio/mpeg", vec![]);
    response.status = 204;
    response
}

fn select_voice(
    payload: &Value,
    family: &str,
    settings: &Value,
    keys: &Value,
    engine: &str,
) -> Result<String, String> {
    let selected = payload
        .get("voice")
        .or_else(|| settings["voices"].get(family));
    if let Some(value) = selected {
        return value
            .as_str()
            .filter(|id| valid_voice(id))
            .map(str::to_string)
            .ok_or("音色 ID 无效".into());
    }
    let voices = catalog(settings, keys, engine, "voices")?;
    let index = crate::characters::FAMILIES
        .iter()
        .position(|item| *item == family)
        .unwrap_or(crate::characters::FAMILIES.len());
    Ok(voices[index % voices.len()].id.clone())
}

//! The decision sources (ADR-0123) and their model catalogs. The table is what the settings page and
//! the file checks share; the catalogs are read on demand, one GET at a time, and nothing here
//! writes a file. A key goes into a request header and nowhere else.

use crate::endpoint::is_loopback_url;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

pub(super) const DEFAULT_SOURCE: &str = "gemini";
const OLLAMA_URL: &str = "http://127.0.0.1:11434";
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const USER_AGENT: &str = concat!("AgentNave-Tray/", env!("CARGO_PKG_VERSION"));

pub(super) enum Key {
    Required,
    Optional,
    None,
}

pub(super) enum Model {
    /// The one model the source offers.
    Fixed(&'static str),
    /// Any model id; this one when the file names none.
    Default(&'static str),
    /// Any model id, which the file must name.
    Free,
}

pub(super) enum Address {
    None,
    /// May be left out for the default.
    Optional(&'static str),
    Required,
}

pub(super) struct Source {
    pub id: &'static str,
    name_zh: &'static str,
    name_en: &'static str,
    pub key: Key,
    pub model: Model,
    pub address: Address,
    /// Whether the source can list its models on request.
    catalog: bool,
    recommended: &'static [&'static str],
}

pub(super) const SOURCES: [Source; 6] = [
    Source {
        id: "gemini",
        name_zh: "Google Gemini",
        name_en: "Google Gemini",
        key: Key::Required,
        model: Model::Fixed("gemini-decision-1"),
        address: Address::None,
        catalog: false,
        recommended: &[],
    },
    Source {
        id: "openai",
        name_zh: "OpenAI",
        name_en: "OpenAI",
        key: Key::Required,
        model: Model::Fixed("gpt-6-luna"),
        address: Address::None,
        catalog: false,
        recommended: &[],
    },
    Source {
        id: "typesafe",
        name_zh: "TypeSafe AI",
        name_en: "TypeSafe AI",
        key: Key::Required,
        model: Model::Default("jev-latest"),
        address: Address::None,
        catalog: true,
        recommended: &["jev-latest", "jev-1.14.0"],
    },
    Source {
        id: "openrouter",
        name_zh: "OpenRouter",
        name_en: "OpenRouter",
        key: Key::Required,
        model: Model::Default("liquid/d1.2-ultra"),
        address: Address::None,
        catalog: true,
        recommended: &[
            "liquid/d1.2-ultra",
            "liquid/d1.2",
            "microsoft/microsoft-decision-1.1",
            "typesafe/jev-1.14",
            "perplexity/pplx-decider-v1.3:free",
            "parallel/decisions-fast",
            "google/gemini-decision-1",
            "openai/gpt-6-luna-decisions",
        ],
    },
    Source {
        id: "ollama",
        name_zh: "本机 Ollama",
        name_en: "Local Ollama",
        key: Key::None,
        model: Model::Free,
        address: Address::Optional(OLLAMA_URL),
        catalog: true,
        recommended: &[],
    },
    Source {
        id: "custom",
        name_zh: "自定义端点",
        name_en: "Custom endpoint",
        key: Key::Optional,
        model: Model::Free,
        address: Address::Required,
        catalog: true,
        recommended: &[],
    },
];

pub(super) fn source(id: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|source| source.id == id)
}

impl Source {
    pub(super) fn default_model(&self) -> Option<&'static str> {
        match self.model {
            Model::Fixed(model) | Model::Default(model) => Some(model),
            Model::Free => None,
        }
    }

    pub(super) fn default_address(&self) -> Option<&'static str> {
        match self.address {
            Address::Optional(address) => Some(address),
            _ => None,
        }
    }

    pub(super) fn takes_address(&self) -> bool {
        !matches!(self.address, Address::None)
    }

    pub(super) fn has_key(&self) -> bool {
        !matches!(self.key, Key::None)
    }
}

/// What the settings page needs to draw the sources, in order.
pub(crate) fn providers() -> Value {
    Value::Array(
        SOURCES
            .iter()
            .map(|source| {
                json!({
                    "id": source.id,
                    "name_zh": source.name_zh,
                    "name_en": source.name_en,
                    "key": match source.key {
                        Key::Required => "required",
                        Key::Optional => "optional",
                        Key::None => "none",
                    },
                    "fixed_model": matches!(source.model, Model::Fixed(_)),
                    "default_model": source.default_model(),
                    "models": source.recommended.iter()
                        .map(|id| json!({"id": id, "name": id}))
                        .collect::<Vec<_>>(),
                    "catalog": source.catalog,
                    "custom_url": source.takes_address(),
                    "url_required": matches!(source.address, Address::Required),
                    "default_base_url": source.default_address(),
                })
            })
            .collect(),
    )
}

/// The status the route answers with, and why.
pub(crate) type Failure = (u16, String);

static CACHE: LazyLock<Mutex<HashMap<String, Vec<Value>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The models a source lists now, from this session's memory unless `refresh` asks again.
pub(crate) fn fetch_models(
    root: &Path,
    provider: &str,
    base_url: Option<&str>,
    refresh: bool,
) -> Result<Vec<Value>, Failure> {
    fetch_catalog(root, provider, base_url, refresh, crate::PREVIEW)
}

pub(super) fn fetch_catalog(
    root: &Path,
    provider: &str,
    base_url: Option<&str>,
    refresh: bool,
    preview: bool,
) -> Result<Vec<Value>, Failure> {
    let source = source(provider)
        .filter(|source| source.catalog)
        .ok_or((400, "这个来源没有可读取的模型目录".to_string()))?;
    let base = catalog_base(root, source, base_url)?;
    let key = request_key(root, source, &base, preview)?;
    let cache_key = cache_key(root, source, &base, key.as_deref());
    if !refresh && let Some(models) = lock_cache().get(&cache_key) {
        return Ok(models.clone());
    }
    let models = match source.id {
        "openrouter" => fetch(
            "https://openrouter.ai/api/v1/models?output_modalities=decisions",
            key.as_deref(),
            parse_openrouter,
        ),
        "typesafe" => {
            let key = key
                .as_deref()
                .ok_or((400, "请先保存 TypeSafe 密钥".into()))?;
            fetch(
                "https://api.typesafe.ai/v1/models",
                Some(key),
                parse_typesafe,
            )
        }
        "ollama" => fetch(
            &format!("{}/api/tags", ollama_root(&base)),
            None,
            parse_ollama,
        ),
        _ => fetch(&custom_models_url(&base), key.as_deref(), parse_listed),
    }?;
    lock_cache().insert(cache_key, models.clone());
    Ok(models)
}

/// The key a catalog request carries, if any. The preview never reads the installed App's keys, and
/// a custom endpoint gets its saved key only at the very address the key was saved with, so a key
/// cannot be sent to an address the user has not paired it with.
pub(super) fn request_key(
    root: &Path,
    source: &Source,
    base: &str,
    preview: bool,
) -> Result<Option<String>, Failure> {
    match source.id {
        "typesafe" if preview => Err((403, "预览版不使用正式版密钥读取目录".into())),
        "openrouter" | "custom" if preview => Ok(None),
        "ollama" => Ok(None),
        "custom" if saved_address(root, source).as_deref() != Some(base) => Ok(None),
        _ => Ok(super::keys::key_of(root, source.id)),
    }
}

/// Where this session's answers are kept apart: the data root, the source and its address, and
/// the key that was sent (as a hash), so a changed key never reads an answer given to another.
fn cache_key(root: &Path, source: &Source, base: &str, key: Option<&str>) -> String {
    let key = key.map_or_else(String::new, |key| {
        format!("{:x}", Sha256::digest(key.as_bytes()))
    });
    format!("{}|{}|{}|{}", root.to_string_lossy(), source.id, base, key)
}

/// Forgets every list read so far; a saved or cleared key changes what a source answers.
pub(super) fn clear_cache() {
    lock_cache().clear();
}

fn lock_cache() -> std::sync::MutexGuard<'static, HashMap<String, Vec<Value>>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The address the saved file gives the source, as saved, when the file's source is this one.
fn saved_address_raw(root: &Path, source: &Source) -> Option<String> {
    super::read(root)
        .ok()
        .flatten()
        .filter(|document| document["decision_provider"].as_str() == Some(source.id))
        .and_then(|document| document["decision_base_url"].as_str().map(str::to_owned))
}

/// That address as the App calls it, or None while it is not a good one.
fn saved_address(root: &Path, source: &Source) -> Option<String> {
    super::normalize_address(&saved_address_raw(root, source)?).ok()
}

/// The address a catalog is read from: the page's own, else the saved file's for the same source,
/// else the source's default; empty for the fixed-address sources.
fn catalog_base(root: &Path, source: &Source, given: Option<&str>) -> Result<String, Failure> {
    if !source.takes_address() {
        return Ok(String::new());
    }
    let saved = saved_address_raw(root, source);
    let chosen = given
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
        .or(saved)
        .or_else(|| source.default_address().map(str::to_owned))
        .unwrap_or_default();
    super::normalize_address(&chosen).map_err(|error| (400, error))
}

/// `<root>` of an Ollama address: the server itself, not its OpenAI-style route.
pub(super) fn ollama_root(base: &str) -> &str {
    let base = base.strip_suffix("/v1/systemone").unwrap_or(base);
    base.strip_suffix("/v1").unwrap_or(base)
}

pub(super) fn custom_models_url(base: &str) -> String {
    let base = base.strip_suffix("/systemone").unwrap_or(base);
    if base.ends_with("/models") {
        base.to_owned()
    } else if base.ends_with("/v1") || base.ends_with("/v1beta") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

fn fetch(
    url: &str,
    key: Option<&str>,
    parse: fn(&Value) -> Vec<Value>,
) -> Result<Vec<Value>, Failure> {
    let unreachable = || (502, "无法连接模型目录，请检查地址、网络或代理".to_string());
    let target = reqwest::Url::parse(url).map_err(|_| (400, "地址无效".to_string()))?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none());
    // A server on this machine is never behind the system proxy.
    if is_loopback_url(&target) {
        client = client.no_proxy();
    }
    let client = client.build().map_err(|_| unreachable())?;
    let mut request = client
        .get(target)
        .header(reqwest::header::USER_AGENT, USER_AGENT);
    if let Some(key) = key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().map_err(|_| unreachable())?;
    let status = response.status();
    if !status.is_success() {
        let message = match status.as_u16() {
            401 | 403 => "平台拒绝了密钥，请检查已保存的密钥".to_string(),
            code => format!("模型目录返回 {code}"),
        };
        return Err((502, message));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unreachable())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err((502, "模型目录过大".into()));
    }
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| (502, "模型目录返回了无效 JSON".to_string()))?;
    Ok(parse(&document))
}

fn entry(id: &str, name: Option<&str>) -> Value {
    json!({"id": id, "name": name.filter(|name| !name.is_empty()).unwrap_or(id)})
}

/// An id the file accepts: visible ASCII, at most 200 characters.
pub(super) fn valid_model_id(id: &str) -> bool {
    (1..=200).contains(&id.chars().count()) && id.chars().all(|c| c.is_ascii_graphic())
}

fn sorted(mut models: Vec<Value>) -> Vec<Value> {
    models.retain(|model| model["id"].as_str().is_some_and(valid_model_id));
    models.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    models.dedup_by(|a, b| a["id"] == b["id"]);
    models
}

pub(super) fn parse_openrouter(document: &Value) -> Vec<Value> {
    sorted(
        document["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let id = model["id"].as_str()?;
                let mut found = entry(id, model["name"].as_str());
                if let Some(price) = model["pricing"]["prompt"].as_str() {
                    found["price"] = json!(price);
                }
                if let Some(context) = model["context_length"].as_u64() {
                    found["context_length"] = json!(context);
                }
                Some(found)
            })
            .collect(),
    )
}

pub(super) fn parse_typesafe(document: &Value) -> Vec<Value> {
    let named = document["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["name"].as_str());
    let listed = document["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str());
    sorted(named.chain(listed).map(|id| entry(id, None)).collect())
}

/// Ollama's `/api/tags`: only what says it can decide and runs on that machine; a model with a
/// `remote_host` is a name that Ollama forwards to a cloud service.
pub(super) fn parse_ollama(document: &Value) -> Vec<Value> {
    sorted(
        document["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| {
                model["capabilities"]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|one| one == "decision"))
                    && model["remote_host"]
                        .as_str()
                        .is_none_or(|host| host.trim().is_empty())
            })
            .filter_map(|model| {
                let id = model["name"].as_str().or_else(|| model["model"].as_str())?;
                Some(entry(id, None))
            })
            .collect(),
    )
}

/// A compatible server's `/models`: OpenAI's `data[].id`, or `models[].name` or `models[].id`.
pub(super) fn parse_listed(document: &Value) -> Vec<Value> {
    let listed = document["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str());
    let named = document["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["name"].as_str().or_else(|| model["id"].as_str()));
    sorted(listed.chain(named).map(|id| entry(id, None)).collect())
}

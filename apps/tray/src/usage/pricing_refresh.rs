use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

const MAX_BYTES: u64 = 20 * 1024 * 1024;
const MAX_RATE: f64 = 1_000_000.0;
const RETRY_MS: i64 = 30 * 60 * 1000;
static LAST_ATTEMPT: AtomicI64 = AtomicI64::new(0);

fn valid_rate(v: f64) -> bool {
    v.is_finite() && (0.0..=MAX_RATE).contains(&v)
}

pub(super) fn cache_path(root: &Path, preview: bool) -> PathBuf {
    if preview {
        crate::preferences::dir().join("pricing-cache.json")
    } else {
        root.join("pricing-cache.json")
    }
}

fn fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|modified| {
            SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age < Duration::from_secs(24 * 60 * 60))
        })
}

pub(super) fn maybe_refresh() {
    let path = cache_path(&crate::data_files::data_root(), crate::PREVIEW);
    if fresh(&path) {
        return;
    }
    let now = super::now();
    let mut previous = LAST_ATTEMPT.load(Ordering::Relaxed);
    loop {
        if now.saturating_sub(previous) < RETRY_MS {
            return;
        }
        match LAST_ATTEMPT.compare_exchange_weak(previous, now, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(actual) => previous = actual,
        }
    }
    std::thread::spawn(move || {
        if let Err(error) = fetch(&path) {
            eprintln!("用量价表更新失败：{error}");
        }
    });
}

fn fetch(path: &Path) -> Result<(), String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none());
    if let Some(url) = crate::proxy::resolve(None) {
        let mut proxy = reqwest::Proxy::all(url.as_str()).map_err(|e| e.to_string())?;
        if let Some(raw) = crate::proxy::resolve_no_proxy(None) {
            proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&raw));
        }
        builder = builder.proxy(proxy);
    }
    let response = builder
        .build()
        .map_err(|e| e.to_string())?
        .get("https://models.dev/api.json")
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("价表响应超过大小限制".into());
    }
    save(path, &bytes)
}

pub(super) fn save(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let source: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let cache = extract(&source)?;
    let parent = path.parent().ok_or("价表缓存路径无父目录")?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("价表缓存路径无效")?;
    crate::data_files::write_atomic(parent, name, &cache)?;
    super::pricing::update(&cache);
    Ok(())
}

fn prices(cost: &Value) -> Map<String, Value> {
    let mut result = Map::new();
    for (source, target) in [
        ("input", "input"),
        ("output", "output"),
        ("cache_read", "cache_read"),
        ("cache_write", "cache_write_5m"),
    ] {
        if let Some(number) = cost[source].as_f64().filter(|v| valid_rate(*v)) {
            result.insert(target.into(), json!(number));
        }
    }
    result
}

fn model_price(cost: &Value) -> Option<Value> {
    let mut result = prices(cost);
    if !result.contains_key("input") || !result.contains_key("output") {
        return None;
    }
    let mut tiers = Vec::new();
    if let Some(source) = cost["tiers"].as_array() {
        for item in source {
            if item["tier"]["type"] == "context"
                && let Some(size) = item["tier"]["size"]
                    .as_u64()
                    .filter(|v| *v > 0 && *v <= i64::MAX as u64)
            {
                let mut tier = prices(item);
                if !tier.is_empty() {
                    tier.insert("above".into(), json!(size));
                    tiers.push(Value::Object(tier));
                }
            }
        }
    } else if cost["context_over_200k"].is_object() {
        let mut tier = prices(&cost["context_over_200k"]);
        if !tier.is_empty() {
            tier.insert("above".into(), json!(200_000));
            tiers.push(Value::Object(tier));
        }
    }
    if !tiers.is_empty() {
        result.insert("tiers".into(), Value::Array(tiers));
    }
    Some(Value::Object(result))
}

/// Providers read from models.dev, official first: a model name already priced by an
/// earlier provider keeps that price. Plan variants and regional mirrors come last.
const PROVIDERS: [&str; 18] = [
    "anthropic",
    "openai",
    "google",
    "xai",
    "deepseek",
    "zhipuai",
    "moonshotai",
    "alibaba",
    "volcengine",
    "minimax",
    "stepfun",
    "bytedance",
    "zai",
    "alibaba-cn",
    "moonshotai-cn",
    "minimax-cn",
    "zhipuai-coding-plan",
    "zai-coding-plan",
];

pub(super) fn extract(source: &Value) -> Result<Value, String> {
    let mut models = Map::new();
    for provider in PROVIDERS {
        if let Some(entries) = source[provider]["models"].as_object() {
            for (name, model) in entries {
                if !name.is_empty()
                    && let Some(price) = model_price(&model["cost"])
                    // a plan subscription lists a zero price: that is no price at all
                    && (!provider.ends_with("-plan") || price["output"].as_f64() > Some(0.0))
                {
                    models.entry(name.to_ascii_lowercase()).or_insert(price);
                }
            }
        }
    }
    let cache = json!({"models": models});
    if !valid_cache(&cache) {
        return Err("有效模型价格不足 20 条".into());
    }
    Ok(cache)
}

pub(super) fn valid_cache(cache: &Value) -> bool {
    cache["models"].as_object().is_some_and(|models| {
        models.len() >= 20
            && models.iter().all(|(name, price)| {
                !name.is_empty()
                    && ["input", "output"]
                        .iter()
                        .all(|key| price[*key].as_f64().is_some_and(valid_rate))
                    && valid_rates(price)
                    && price.get("tiers").is_none_or(|tiers| {
                        tiers.as_array().is_some_and(|tiers| {
                            tiers.iter().all(|tier| {
                                tier["above"].as_i64().is_some_and(|v| v > 0) && valid_rates(tier)
                            })
                        })
                    })
            })
    })
}

fn valid_rates(price: &Value) -> bool {
    [
        "input",
        "output",
        "cache_read",
        "cache_write_5m",
        "cache_write_1h",
    ]
    .iter()
    .all(|key| {
        price
            .get(*key)
            .is_none_or(|value| value.as_f64().is_some_and(valid_rate))
    })
}

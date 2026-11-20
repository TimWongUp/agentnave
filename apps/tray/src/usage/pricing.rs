use super::Event;
use serde_json::Value;
use std::sync::{LazyLock, RwLock};

static TABLE: LazyLock<RwLock<Value>> = LazyLock::new(|| {
    let path = super::pricing_refresh::cache_path(&crate::data_files::data_root(), crate::PREVIEW);
    let cache = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    RwLock::new(merged(cache.as_ref()))
});

pub(super) fn merged(cache: Option<&Value>) -> Value {
    let mut bundled: Value =
        serde_json::from_str(include_str!("pricing_bundled.json")).expect("bundled usage prices");
    if let Some(cache) = cache.filter(|v| super::pricing_refresh::valid_cache(v)) {
        bundled["models"]
            .as_object_mut()
            .expect("bundled models")
            .extend(
                cache["models"]
                    .as_object()
                    .expect("validated models")
                    .clone(),
            );
    }
    bundled
}

pub(super) fn update(cache: &Value) {
    *TABLE.write().unwrap_or_else(|e| e.into_inner()) = merged(Some(cache));
}

fn lookup<'a>(table: &'a Value, model: &str) -> Option<&'a Value> {
    let table = table["models"].as_object()?;
    let mut name = model.to_ascii_lowercase();
    if let Some(v) = table.get(&name) {
        return Some(v);
    }
    for prefix in ["anthropic/", "openai/", "google/"] {
        if let Some(s) = name.strip_prefix(prefix) {
            name = s.to_owned();
            break;
        }
    }
    if let Some(v) = table.get(&name) {
        return Some(v);
    }
    if let Some(i) = name.find('[') {
        name.truncate(i);
    }
    if let Some(v) = table.get(&name) {
        return Some(v);
    }
    if let Some((base, suffix)) = name.rsplit_once('-')
        && suffix.len() == 8
        && suffix.bytes().all(|c| c.is_ascii_digit())
    {
        name = base.to_owned();
    }
    if let Some(v) = table.get(&name) {
        return Some(v);
    }
    loop {
        let stripped = ["-xhigh", "-high", "-medium", "-low", "-max", "-tiered"]
            .into_iter()
            .find_map(|suffix| name.strip_suffix(suffix));
        let Some(base) = stripped else {
            break;
        };
        name = base.to_owned();
        if let Some(v) = table.get(&name) {
            return Some(v);
        }
    }
    let alias = match name.as_str() {
        "sonnet" => "claude-sonnet-4-5",
        "opus" => "claude-opus-4-5",
        "haiku" => "claude-haiku-4-5",
        _ => &name,
    };
    table.get(alias)
}

pub(super) fn cost(e: &Event) -> Option<f64> {
    let table = TABLE.read().unwrap_or_else(|e| e.into_inner());
    let base = lookup(&table, &e.model)?;
    let tier = base["tiers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["above"].as_i64().is_some_and(|v| e.prompt() > v))
        .max_by_key(|t| t["above"].as_i64().unwrap_or(0));
    let price = |field: &str| {
        tier.and_then(|t| t[field].as_f64())
            .or_else(|| base[field].as_f64())
    };
    let input = price("input")?;
    let component = |count: i64, rate: Option<f64>| if count == 0 { Some(0.0) } else { rate };
    let read = component(e.read, price("cache_read"))?;
    let write5 = component(e.write5, price("cache_write_5m"))?;
    let write1 = component(
        e.write1,
        price("cache_write_1h").or_else(|| (e.provider == "claude").then_some(2.0 * input)),
    )?;
    Some(
        (e.fresh as f64 * input
            + e.read as f64 * read
            + e.write5 as f64 * write5
            + e.write1 as f64 * write1
            + e.output as f64 * price("output")?)
            / 1_000_000.0,
    )
}

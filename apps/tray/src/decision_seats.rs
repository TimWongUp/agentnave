//! decision-seats.json and decision-keys.json (ADR-0087, ADR-0093, ADR-0123): the user's own seat
//! catalog, the decision source and model, the random-candidates switch and each source's API key.
//! The App writes them (without the
//! App, decision-seats.json may be written by hand); the Python service reads them on every call.
//! The checks here mirror the service's, so a file this module writes is one the service accepts.
//!
//! Which bosses use the catalog is their `decision` mode in orchestration.json, not anything in
//! this file.

mod keys;
mod models;

use crate::data_files::write_atomic;
use crate::orchestration_roles::{
    MAX_SEATS, object, trim_text, validate_seat_cards, validate_seat_metadata,
};
use crate::providers::PROVIDERS;
use serde_json::{Value, json};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub(crate) use keys::{keys_state, no_keys, save_key};
use models::{Address, Model, Source, source};
pub(crate) use models::{fetch_models, providers};

const FILE: &str = "decision-seats.json";
const SEAT_FIELDS: [&str; 11] = [
    "id",
    "kind",
    "role",
    "description",
    "provider",
    "account",
    "model",
    "effort",
    "fallbacks",
    "access",
    "difficulty",
];

/// Whether a seat id is the name of a fixed seat or of a review role, which the service refuses.
fn reserved(id: &str) -> bool {
    let fixed = crate::orchestration_roles::catalog()
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|role| role["seat_prefix"].as_str())
        .any(|prefix| {
            id.strip_prefix(prefix)
                .is_some_and(|rest| matches!(rest, "-easy" | "-medium" | "-hard"))
        });
    fixed || id == "image-generator" || id == "auto" || crate::members::REVIEW_ROLES.contains(&id)
}

fn validate_seat(seat: &Value, what: &str) -> Result<(), String> {
    let fields = object(seat, what)?;
    if let Some(key) = fields
        .keys()
        .find(|key| !SEAT_FIELDS.contains(&key.as_str()))
    {
        return Err(format!("{what} 含未知字段 {key}"));
    }
    validate_seat_metadata(fields, what)?;
    let text = |key: &str| fields.get(key).and_then(Value::as_str);
    if reserved(text("id").unwrap_or_default()) {
        return Err(format!("{what} 的标识与固定工位或评审角色重名"));
    }
    if !text("provider").is_some_and(|provider| PROVIDERS.contains(&provider)) {
        return Err(format!("{what} 的 CLI 不受支持"));
    }
    if !text("model").is_some_and(|model| !model.trim().is_empty()) {
        return Err(format!("{what} 缺少模型"));
    }
    if fields
        .get("effort")
        .is_some_and(|effort| !effort.is_string())
    {
        return Err(format!("{what} 的 effort 必须为字符串"));
    }
    crate::members::validate_account(fields, what)?;
    crate::members::validate_fallbacks(fields, what)?;
    Ok(())
}

/// The address of a source that takes one, as it is saved; the checks are those of the summary
/// service's custom address.
fn normalize_address(address: &str) -> Result<String, String> {
    let address = address.trim();
    if address.chars().count() > 512 {
        return Err("决策模型 API 地址过长".into());
    }
    crate::voice::normalize_endpoint(address, "决策模型 API")
}

/// The source the file names, the default one while it names none.
fn source_of(top: &serde_json::Map<String, Value>) -> Result<&'static Source, String> {
    let id = match top.get("decision_provider") {
        None => models::DEFAULT_SOURCE,
        Some(Value::String(id)) => id,
        Some(_) => return Err("决策来源必须为字符串".into()),
    };
    source(id).ok_or_else(|| "决策来源不受支持".into())
}

fn validate_source(top: &serde_json::Map<String, Value>) -> Result<(), String> {
    let source = source_of(top)?;
    let address = match top.get("decision_base_url") {
        None => "",
        Some(Value::String(address)) => address.as_str(),
        Some(_) => return Err("decision_base_url 必须为字符串".into()),
    };
    match source.address {
        Address::None if !address.trim().is_empty() => {
            return Err(format!("{} 不使用自定义 API 地址", source.id));
        }
        Address::Required => drop(normalize_address(address)?),
        Address::Optional(_) if !address.is_empty() => drop(normalize_address(address)?),
        _ => {}
    }
    let model = match top.get("decision_model") {
        None => None,
        Some(Value::String(model)) => Some(model.as_str()),
        Some(_) => return Err("决策模型必须为字符串".into()),
    };
    match (&source.model, model) {
        (Model::Fixed(fixed), Some(model)) if model != *fixed => Err("决策模型不受支持".into()),
        (Model::Free, None) => Err("请填写决策模型".into()),
        (_, Some(model)) if !models::valid_model_id(model) => {
            Err("决策模型需为 1 到 200 个可见 ASCII 字符".into())
        }
        _ => Ok(()),
    }
}

/// decision-seats.json: `{"version": 1, "decision_provider"?: "gemini" | "openai" | "typesafe" |
/// "openrouter" | "ollama" | "custom", "decision_model"?, "decision_base_url"?: only for "ollama"
/// and "custom", "random_candidates"?: bool, "seats": [{id, kind, role, description, provider,
/// account?, model, effort?, fallbacks?, access, difficulty}]}`, one seat per difficulty of a card:
/// `kind` is a group of the role catalog, "custom" or "image". An image seat is a card of one seat,
/// write access and no difficulty.
pub fn validate(document: &Value) -> Result<(), String> {
    let top = object(document, FILE)?;
    let allowed = [
        "version",
        "decision_provider",
        "decision_model",
        "decision_base_url",
        "random_candidates",
        "seats",
    ];
    if let Some(key) = top.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{FILE} 含未知字段 {key}"));
    }
    if document["version"].as_u64() != Some(1) {
        return Err(format!("{FILE} 的 version 必须为 1"));
    }
    validate_source(top)?;
    if top
        .get("random_candidates")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("random_candidates 必须为布尔值".into());
    }
    let seats = match top.get("seats") {
        None => &Vec::new(),
        Some(seats) => seats.as_array().ok_or("seats 必须为数组")?,
    };
    if seats.len() > MAX_SEATS {
        return Err(format!("工位最多 {MAX_SEATS} 个"));
    }
    for (index, seat) in seats.iter().enumerate() {
        validate_seat(seat, &format!("seats[{index}]"))?;
    }
    validate_seat_cards(seats)
}

/// The document as the page sends it, made ready to save: roles trimmed, as the service does, the
/// source's own model named when the page names none, and the default source left implicit.
pub fn normalize(document: &mut Value) {
    // Anything but an object is left as it is, for `validate` to refuse.
    let Some(top) = document.as_object_mut() else {
        return;
    };
    // A source that is not one is left for `validate` too.
    if let Ok(source) = source_of(top) {
        if let Some(model) = top.get("decision_model").and_then(Value::as_str) {
            top.insert("decision_model".into(), json!(model.trim()));
        }
        if let Some(default) = source.default_model() {
            top.entry("decision_model").or_insert(json!(default));
        }
        if source.id == models::DEFAULT_SOURCE {
            top.remove("decision_provider");
            top.remove("decision_base_url");
        } else if let Some(address) = top.get("decision_base_url").and_then(Value::as_str) {
            // The address as it is saved when it is a good one; a bad one is left for `validate`.
            match normalize_address(address) {
                Ok(address) => top.insert("decision_base_url".into(), json!(address)),
                Err(_) if address.trim().is_empty() => top.remove("decision_base_url"),
                Err(_) => None,
            };
        }
    }
    for seat in top
        .get_mut("seats")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(role) = seat["role"].as_str().map(|role| trim_text(role).to_owned()) {
            seat["role"] = json!(role);
        }
    }
}

/// None while the file is missing, which is an empty catalog.
pub fn read(root: &Path) -> Result<Option<Value>, String> {
    let content = match fs::read(root.join(FILE)) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 {FILE}：{error}")),
    };
    let mut document =
        serde_json::from_slice(&content).map_err(|error| format!("{FILE} 损坏：{error}"))?;
    crate::retired_providers::decision_seats(&mut document);
    validate(&document).map_err(|error| format!("{FILE} 格式错误：{error}"))?;
    Ok(Some(document))
}

pub fn save(root: &Path, document: &Value) -> Result<(), String> {
    validate(document)?;
    write_atomic(root, FILE, document)
}

#[cfg(test)]
mod tests;

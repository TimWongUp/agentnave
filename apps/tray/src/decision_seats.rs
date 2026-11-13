//! decision-seats.json and decision-keys.json (ADR-0087): the user's own seat catalog and the
//! decision model's API key. The App is their only writer; the Python service reads them on every
//! call. The checks here mirror the service's, so a file this module writes is one the service
//! accepts.
//!
//! Which bosses use the catalog is their `decision` mode in orchestration.json, not anything in
//! this file.

mod keys;

use crate::data_files::write_atomic;
use crate::providers::PROVIDERS;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub(crate) use keys::{key_state, save_key};

const FILE: &str = "decision-seats.json";
const DECISION_MODEL: &str = "gemini-decision-1";
const MAX_SEATS: usize = 255;
const ROLE_MAX_CHARS: usize = 40;
const DESCRIPTION_MAX_CHARS: usize = 200;
/// The kind of a seat whose specialty the user named; any other kind is a group of the role catalog.
const CUSTOM_KIND: &str = "custom";
/// The kind of an image generation seat: write access, no difficulty, one card per seat.
const IMAGE_KIND: &str = "image";
const DIFFICULTIES: [&str; 3] = ["easy", "medium", "hard"];
const SEAT_FIELDS: [&str; 9] = [
    "id",
    "kind",
    "role",
    "description",
    "provider",
    "model",
    "effort",
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
                .is_some_and(|rest| rest == "-easy" || rest == "-hard")
        });
    fixed || id == "image-generator" || id == "auto" || crate::members::REVIEW_ROLES.contains(&id)
}

/// The read or write type a role-catalog group is fixed to; None for any other kind.
fn group_access(kind: &str) -> Option<&'static str> {
    crate::orchestration_roles::catalog()
        .as_array()
        .into_iter()
        .flatten()
        .find(|role| role["group"] == kind)
        .and_then(|role| role["access"].as_str())
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 128
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What the text of a seat may not hold, by code point, listed one by one: the service
/// (`_REFUSED_CODES` in orchestration_roles.py) checks the very same table, so no Unicode category
/// table decides. Control characters, white space but the ordinary space, invisible format
/// characters and private use (surrogates cannot occur in a Rust string).
const REFUSED: [(u32, u32); 19] = [
    (0x0000, 0x001F),
    (0x007F, 0x009F),
    (0x00A0, 0x00A0),
    (0x00AD, 0x00AD),
    (0x061C, 0x061C),
    (0x1680, 0x1680),
    (0x180E, 0x180E),
    (0x2000, 0x200F),
    (0x2028, 0x202F),
    (0x205F, 0x205F),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0x3000, 0x3000),
    (0xD800, 0xDFFF),
    (0xE000, 0xF8FF),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0xF0000, 0xFFFFD),
    (0x100000, 0x10FFFD),
];
/// The white space trimmed from both ends first; inside the text only the ordinary space is kept.
const TRIMMED: [(u32, u32); 10] = [
    (0x09, 0x0D),
    (0x20, 0x20),
    (0x85, 0x85),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
];

fn listed(table: &[(u32, u32)], c: char) -> bool {
    table
        .iter()
        .any(|&(low, high)| (low..=high).contains(&u32::from(c)))
}

fn trim_text(text: &str) -> &str {
    text.trim_matches(|c| listed(&TRIMMED, c))
}

/// Trimmed text of 1 to `limit` characters the table allows.
fn printable_text(text: Option<&str>, limit: usize) -> bool {
    let text = text.map(trim_text).unwrap_or_default();
    !text.is_empty() && text.chars().count() <= limit && !text.chars().any(|c| listed(&REFUSED, c))
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{what} 必须为对象"))
}

fn validate_seat(seat: &Value, what: &str) -> Result<(), String> {
    let fields = object(seat, what)?;
    if let Some(key) = fields
        .keys()
        .find(|key| !SEAT_FIELDS.contains(&key.as_str()))
    {
        return Err(format!("{what} 含未知字段 {key}"));
    }
    let text = |key: &str| fields.get(key).and_then(Value::as_str);
    if !text("id").is_some_and(valid_id) {
        return Err(format!("{what} 的标识无效"));
    }
    if reserved(text("id").unwrap_or_default()) {
        return Err(format!("{what} 的标识与固定工位或评审角色重名"));
    }
    let kind = text("kind").unwrap_or_default();
    let fixed = group_access(kind);
    if ![CUSTOM_KIND, IMAGE_KIND].contains(&kind) && fixed.is_none() {
        return Err(format!("{what} 的类型无效"));
    }
    if !printable_text(text("role"), ROLE_MAX_CHARS) {
        return Err(format!("{what} 的专业需为 1–{ROLE_MAX_CHARS} 个可打印字符"));
    }
    if !printable_text(text("description"), DESCRIPTION_MAX_CHARS) {
        return Err(format!(
            "{what} 的说明需为 1–{DESCRIPTION_MAX_CHARS} 个可打印字符"
        ));
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
    if !matches!(text("access"), Some("read" | "write")) {
        return Err(format!("{what} 的读写类型必须为 read 或 write"));
    }
    if fixed.is_some_and(|access| Some(access) != text("access")) {
        return Err(format!("{what} 的读写类型与所属工位分组不符"));
    }
    if kind == IMAGE_KIND {
        // A null difficulty is as good as none, as the service reads it.
        if text("access") != Some("write")
            || fields
                .get("difficulty")
                .is_some_and(|value| !value.is_null())
        {
            return Err(format!("{what} 是生图工位，只能可写且没有难度"));
        }
    } else if !text("difficulty").is_some_and(|difficulty| DIFFICULTIES.contains(&difficulty)) {
        return Err(format!("{what} 的难度无效"));
    }
    Ok(())
}

/// decision-seats.json: `{"version": 1, "decision_model": "gemini-decision-1", "seats": [{id, kind,
/// role, description, provider, model, effort?, access, difficulty}]}`, one seat per difficulty of
/// a card: `kind` is a group of the role catalog, "custom" or "image". An image seat is a card of
/// one seat, write access and no difficulty.
pub fn validate(document: &Value) -> Result<(), String> {
    let top = object(document, FILE)?;
    let allowed = ["version", "decision_model", "seats"];
    if let Some(key) = top.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{FILE} 含未知字段 {key}"));
    }
    if document["version"].as_u64() != Some(1) {
        return Err(format!("{FILE} 的 version 必须为 1"));
    }
    if top
        .get("decision_model")
        .is_some_and(|model| model != DECISION_MODEL)
    {
        return Err("决策模型不受支持".into());
    }
    let seats = match top.get("seats") {
        None => &Vec::new(),
        Some(seats) => seats.as_array().ok_or("seats 必须为数组")?,
    };
    if seats.len() > MAX_SEATS {
        return Err(format!("工位最多 {MAX_SEATS} 个"));
    }
    let mut ids = HashSet::new();
    let mut levels = HashSet::new();
    let mut group_roles = std::collections::HashMap::new();
    let mut card_access = std::collections::HashMap::new();
    for (index, seat) in seats.iter().enumerate() {
        validate_seat(seat, &format!("seats[{index}]"))?;
        if !ids.insert(seat["id"].as_str().unwrap_or_default()) {
            return Err(format!("工位标识重复：{}", seat["id"]));
        }
        let (kind, role) = (seat["kind"].as_str(), seat["role"].as_str().map(trim_text));
        if !levels.insert((kind, role, seat["difficulty"].as_str())) {
            return Err(format!("seats[{index}] 与同一张卡片的另一档重复"));
        }
        // A card shows one access, so its seats must agree on it.
        if *card_access
            .entry((kind, role))
            .or_insert(seat["access"].as_str())
            != seat["access"].as_str()
        {
            return Err(format!("seats[{index}] 与同一张卡片的读写类型不一致"));
        }
        if ![Some(CUSTOM_KIND), Some(IMAGE_KIND)].contains(&kind)
            && *group_roles.entry(kind).or_insert(role) != role
        {
            return Err(format!("seats[{index}] 的分组对应了不同的名称"));
        }
    }
    Ok(())
}

/// The document as the page sends it, made ready to save: roles trimmed, as the service does,
/// and the decision model named.
pub fn normalize(document: &mut Value) {
    // Anything but an object is left as it is, for `validate` to refuse.
    let Some(top) = document.as_object_mut() else {
        return;
    };
    top.entry("decision_model").or_insert(json!(DECISION_MODEL));
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
    let document =
        serde_json::from_slice(&content).map_err(|error| format!("{FILE} 损坏：{error}"))?;
    validate(&document).map_err(|error| format!("{FILE} 格式错误：{error}"))?;
    Ok(Some(document))
}

pub fn save(root: &Path, document: &Value) -> Result<(), String> {
    validate(document)?;
    write_atomic(root, FILE, document)
}

#[cfg(test)]
mod tests;

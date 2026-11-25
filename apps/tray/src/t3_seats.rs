//! t3-seats.json (ADR-0121): the T3 Code orchestration switch, random-candidates flag, the bosses it
//! is on for, task seat catalog and review roles. The App writes it; the Python service (`src/agentnave/t3_seats.py`)
//! reads it on T3 main threads. Schema checks here mirror `T3SeatsFile` so any file the App writes
//! is one the service accepts.

use crate::data_files::write_atomic;
use crate::orchestration_roles::{
    MAX_SEATS, object, trim_text, validate_seat_cards, validate_seat_metadata,
};
use serde_json::{Value, json};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub const FILE: &str = "t3-seats.json";
const MAX_CANDIDATES: usize = 4;
const MAX_OPTIONS: usize = 1;
const TOP_FIELDS: [&str; 6] = [
    "version",
    "enabled",
    "random_candidates",
    "bosses",
    "seats",
    "review",
];
const SEAT_FIELDS: [&str; 7] = [
    "id",
    "kind",
    "role",
    "description",
    "access",
    "difficulty",
    "candidates",
];
const CANDIDATE_FIELDS: [&str; 3] = ["instance", "model", "options"];
const OPTION_FIELDS: [&str; 2] = ["id", "value"];

/// Options that govern permissions, sandbox, execution mode or service tiers must never be stored
/// in T3 seat candidates (mirrors `FORBIDDEN_T3_OPTION_IDS` in `src/agentnave/t3_seats.py`).
pub(crate) const FORBIDDEN_OPTION_IDS: [&str; 11] = [
    "mode",
    "sandbox",
    "multitask",
    "permission",
    "permissionmode",
    "approvalpolicy",
    "runtimemode",
    "fastmode",
    "ultracode",
    "servicetier",
    "cyberaccessprogram",
];

pub(crate) fn is_forbidden_option_id(id: &str) -> bool {
    FORBIDDEN_OPTION_IDS.contains(&id.to_lowercase().as_str())
}

fn non_empty_trimmed(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn validate_option(option: &Value, what: &str) -> Result<(), String> {
    let fields = object(option, what)?;
    if let Some(key) = fields
        .keys()
        .find(|key| !OPTION_FIELDS.contains(&key.as_str()))
    {
        return Err(format!("{what} 含未知字段 {key}"));
    }
    let Some(id) = fields.get("id").and_then(Value::as_str) else {
        return Err(format!("{what} 缺少推理档标识"));
    };
    if !non_empty_trimmed(id) {
        return Err(format!("{what} 的推理档标识不能为空且不能含首尾空白"));
    }
    if is_forbidden_option_id(id) {
        return Err(format!("{what} 不能保存权限或模式类选项 {id}"));
    }
    let Some(value) = fields.get("value").and_then(Value::as_str) else {
        return Err(format!("{what} 缺少推理档取值"));
    };
    if value.trim().is_empty() {
        return Err(format!("{what} 的推理档取值不能为空"));
    }
    Ok(())
}

fn validate_candidate(candidate: &Value, what: &str) -> Result<(), String> {
    let fields = object(candidate, what)?;
    if let Some(key) = fields
        .keys()
        .find(|key| !CANDIDATE_FIELDS.contains(&key.as_str()))
    {
        return Err(format!("{what} 含未知字段 {key}"));
    }
    if !fields
        .get("instance")
        .and_then(Value::as_str)
        .is_some_and(non_empty_trimmed)
    {
        return Err(format!("{what} 的实例标识无效"));
    }
    if !fields
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(non_empty_trimmed)
    {
        return Err(format!("{what} 的模型名无效"));
    }
    if let Some(options) = fields.get("options") {
        let list = options
            .as_array()
            .ok_or_else(|| format!("{what} 的 options 必须为数组"))?;
        if list.len() > MAX_OPTIONS {
            return Err(format!("{what} 最多只能带 {MAX_OPTIONS} 个推理档选项"));
        }
        for (index, option) in list.iter().enumerate() {
            validate_option(option, &format!("{what}.options[{index}]"))?;
        }
    }
    Ok(())
}

fn validate_candidates(value: &Value, what: &str) -> Result<(), String> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{what} 必须为数组"))?;
    if list.len() > MAX_CANDIDATES {
        return Err(format!("{what} 最多 {MAX_CANDIDATES} 个候选"));
    }
    for (index, candidate) in list.iter().enumerate() {
        validate_candidate(candidate, &format!("{what}[{index}]"))?;
    }
    Ok(())
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
    if let Some(id) = fields.get("id").and_then(Value::as_str)
        && crate::members::REVIEW_ROLES.contains(&id)
    {
        return Err(format!("{what} 的标识与评审角色重名"));
    }
    if let Some(candidates) = fields.get("candidates") {
        validate_candidates(candidates, &format!("{what}.candidates"))?;
    }
    Ok(())
}

/// Validates `t3-seats.json` against the exact `T3SeatsFile` schema on the server.
pub fn validate(document: &Value) -> Result<(), String> {
    let top = object(document, FILE)?;
    if let Some(key) = top.keys().find(|key| !TOP_FIELDS.contains(&key.as_str())) {
        return Err(format!("{FILE} 含未知字段 {key}"));
    }
    if top.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(format!("{FILE} 的 version 必须为 1"));
    }
    for flag in ["enabled", "random_candidates"] {
        if top.get(flag).is_some_and(|value| !value.is_boolean()) {
            return Err(format!("{flag} 必须为布尔值"));
        }
    }
    if let Some(bosses) = top.get("bosses").filter(|bosses| !bosses.is_null()) {
        let list = bosses.as_array().ok_or("bosses 必须为数组")?;
        for (index, boss) in list.iter().enumerate() {
            let Some(id) = boss
                .as_str()
                .filter(|id| crate::rules::T3_BOSSES.contains(id))
            else {
                return Err(format!("bosses[{index}] 不是 T3 支持的 CLI"));
            };
            if list[..index]
                .iter()
                .any(|earlier| earlier.as_str() == Some(id))
            {
                return Err(format!("bosses 含重复的 {id}"));
            }
        }
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
    validate_seat_cards(seats)?;
    if let Some(review) = top.get("review") {
        let roles = object(review, "review")?;
        for (role, candidates) in roles {
            if !crate::members::REVIEW_ROLES.contains(&role.as_str()) {
                return Err(format!("review 含未知评审角色 {role}"));
            }
            validate_candidates(candidates, &format!("review.{role}"))?;
        }
    }
    Ok(())
}

/// Whether at least one task seat or review role has a candidate configured.
pub fn has_candidates(document: &Value) -> bool {
    let task_has = document
        .get("seats")
        .and_then(Value::as_array)
        .is_some_and(|seats| {
            seats.iter().any(|seat| {
                seat.get("candidates")
                    .and_then(Value::as_array)
                    .is_some_and(|list| !list.is_empty())
            })
        });
    let review_has = document
        .get("review")
        .and_then(Value::as_object)
        .is_some_and(|roles| {
            roles
                .values()
                .any(|list| list.as_array().is_some_and(|items| !items.is_empty()))
        });
    task_has || review_has
}

/// Whether a validated `t3-seats.json` activates T3 orchestration.
pub fn is_active(document: &Value) -> bool {
    document.get("enabled").and_then(Value::as_bool) == Some(true) && has_candidates(document)
}

/// Schema validation plus the App save gate: turning T3 orchestration on requires at least one
/// candidate across task seats or review roles.
pub fn validate_for_save(document: &Value) -> Result<(), String> {
    validate(document)?;
    if document.get("enabled").and_then(Value::as_bool) == Some(true) && !has_candidates(document) {
        return Err("开启 T3 编排前，至少要为一个任务工位或评审角色选定候选模型".into());
    }
    Ok(())
}

/// Trims seat roles and descriptions using the shared table before validation and saving.
pub fn normalize(document: &mut Value) {
    let Some(top) = document.as_object_mut() else {
        return;
    };
    for seat in top
        .get_mut("seats")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(fields) = seat.as_object_mut() {
            for key in ["role", "description"] {
                if let Some(value) = fields
                    .get(key)
                    .and_then(Value::as_str)
                    .map(|text| trim_text(text).to_owned())
                {
                    fields.insert(key.into(), json!(value));
                }
            }
        }
    }
}

/// None while the file is missing, which means T3 orchestration has not been configured yet.
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
    validate_for_save(document)?;
    write_atomic(root, FILE, document)
}

#[cfg(test)]
mod tests;

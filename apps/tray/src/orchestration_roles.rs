//! The fixed role catalog the server, the App and the generated rules share.
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;

static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../src/agentnave/orchestration_roles.json"
    ))
    .expect("valid bundled orchestration roles")
});

/// Every role in display order, as the settings page reads it from GET /app/orchestration.
pub fn catalog() -> &'static Value {
    &CATALOG
}

fn roles() -> &'static [Value] {
    catalog().as_array().map_or(&[], Vec::as_slice)
}

fn group(role: &'static Value) -> Option<&'static str> {
    role["group"].as_str()
}

pub fn groups() -> Vec<&'static str> {
    roles().iter().filter_map(group).collect()
}

/// The groups a document turns on, in catalog order. A missing or null `enabled_roles` keeps the
/// groups that existed before the switches; `[]` turns them all off.
pub fn enabled_groups(document: &Value) -> Result<Vec<&'static str>, String> {
    let picked = match document.get("enabled_roles") {
        None | Some(Value::Null) => {
            let legacy = roles().iter().filter(|role| role["legacy_enabled"] == true);
            return Ok(legacy.filter_map(group).collect());
        }
        Some(Value::Array(picked)) => picked,
        Some(_) => return Err("enabled_roles 必须为数组".into()),
    };
    let known = groups();
    let mut seen = HashSet::new();
    for role in picked {
        let role = role.as_str().ok_or("enabled_roles 只能包含角色名")?;
        if !known.contains(&role) {
            return Err(format!("enabled_roles 含未知角色 {role}"));
        }
        if !seen.insert(role) {
            return Err(format!("enabled_roles 重复了角色 {role}"));
        }
    }
    Ok(known
        .into_iter()
        .filter(|role| seen.contains(role))
        .collect())
}

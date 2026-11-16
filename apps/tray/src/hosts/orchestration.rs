//! The row identity and effective orchestration modes shared with the MCP service.
use super::{Host, Hosts};
use serde_json::{Value, json};

/// The variables a connection runs with: the host its service acts as, and the row it came from.
pub(super) fn host_env(host: &Host) -> Value {
    json!({"AGENTNAVE_HOST": host.boss, "AGENTNAVE_HOST_APP": host.id})
}
/// Add the connection's variables to an entry, keeping its other ones.
pub(super) fn set_host_env(entry: &mut Value, host: &Host, code: &str) -> Result<(), String> {
    if entry.get("env").is_none() {
        entry["env"] = json!({});
    }
    let env = entry["env"]
        .as_object_mut()
        .ok_or(format!("{code}:env 必须为对象"))?;
    if let Value::Object(wanted) = host_env(host) {
        env.extend(wanted);
    }
    drop_self_exclusion(env, crate::providers::own_provider(host.boss));
    Ok(())
}

/// The manual setup once excluded each host's own CLI; that choice now lives in
/// `orchestration.json`, so only the other providers stay excluded (ADR-0096).
fn drop_self_exclusion(env: &mut serde_json::Map<String, Value>, own: &str) {
    const KEY: &str = "AGENTNAVE_EXCLUDED_PROVIDERS";
    let Some(listed) = env.get(KEY).and_then(Value::as_str) else {
        return;
    };
    let names: Vec<&str> = listed.split(',').collect();
    if !names
        .iter()
        .any(|name| name.trim().eq_ignore_ascii_case(own))
    {
        return;
    }
    let others: Vec<&str> = names
        .into_iter()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(own))
        .collect();
    if others.is_empty() {
        env.remove(KEY);
    } else {
        env.insert(KEY.into(), json!(others.join(",")));
    }
}

impl Hosts {
    /// Every connected row's mode as the rules files apply it, kept in orchestration.json.
    pub fn effective_apps(&self, document: &Value) -> Value {
        crate::rules::effective(&self.connected_rule_apps(), &self.context.home, document)
    }
}

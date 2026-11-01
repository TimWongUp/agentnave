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
    Ok(())
}

impl Hosts {
    /// Every connected row's mode as the rules files apply it, kept in orchestration.json.
    pub fn effective_apps(&self, document: &Value) -> Value {
        crate::rules::effective(&self.connected_rule_apps(), &self.context.home, document)
    }
}

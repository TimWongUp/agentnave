//! Tool-call limits for AgentNave's ten-minute wait, with two minutes for the reply.
use super::{Host, Value, json};

pub(super) fn ensure(entry: &mut Value, host: &Host) -> Result<(), String> {
    let (key, minimum) = match host.boss {
        "codex" => ("tool_timeout_sec", 720),
        "claude" => ("timeout", 720_000),
        "antigravity" => ("timeoutSeconds", 720),
        "pi" | "hermes" => ("timeout", 720),
        "deepseek" => ("toolCallTimeoutMs", 720_000),
        "zcode" => ("timeoutMs", 720_000),
        "grok" => ("tool_timeout_sec", 720),
        // CodeBuddy reads a host-process variable (default one hour), not a server field.
        // WorkBuddy's limit is unverified. Neither belongs in the child service's env.
        _ => return Ok(()),
    };
    // Grok's native default is 6000 s; do not shorten it by adding an override.
    if host.boss != "grok" || entry.get(key).is_some() {
        raise(entry, key, minimum, host.boss == "antigravity")?;
    }
    if host.boss == "grok"
        && let Some(overrides) = entry.get_mut("tool_timeouts")
    {
        if !overrides.is_object() {
            return Err("tool_timeouts 必须为对象".into());
        }
        for tool in ["wait", "start_agent", "run_round"] {
            if overrides.get(tool).is_some() {
                raise(overrides, tool, 720, false)?;
            }
        }
    }
    Ok(())
}

fn raise(entry: &mut Value, key: &str, minimum: u64, unlimited: bool) -> Result<(), String> {
    if let Some(current) = entry.get(key) {
        let value = current.as_f64().ok_or(format!("{key} 必须为数字"))?;
        if (unlimited && value == -1.0) || value >= minimum as f64 {
            return Ok(());
        }
    }
    entry[key] = json!(minimum);
    Ok(())
}

use super::{event, hash, number, stem, string, timestamp};
use crate::usage::Event;
use serde_json::{Value, json};
use std::path::Path;

fn amounts(v: &Value) -> [i64; 4] {
    [
        number(v, "input_tokens"),
        number(v, "cached_input_tokens"),
        number(v, "cache_write_input_tokens"),
        number(v, "output_tokens"),
    ]
}

pub(super) fn parse(path: &Path, v: &Value, state: &mut Value) -> Option<Event> {
    let payload = &v["payload"];
    match string(v, "type") {
        "session_meta" => {
            state["session"] = json!(string(payload, "id"));
            let cwd = string(payload, "cwd");
            if !cwd.is_empty() {
                state["cwd"] = json!(cwd);
            }
            return None;
        }
        "turn_context" => {
            state["model"] = json!(string(payload, "model"));
            let cwd = string(payload, "cwd");
            if !cwd.is_empty() {
                state["cwd"] = json!(cwd);
            }
            return None;
        }
        "event_msg" if string(payload, "type") == "token_count" => {}
        _ => return None,
    }
    let info = &payload["info"];
    if !info["total_token_usage"].is_object() {
        return None;
    }
    let total = amounts(&info["total_token_usage"]);
    let previous = state["total"]
        .as_array()
        .map(|a| std::array::from_fn::<_, 4, _>(|i| a.get(i).and_then(Value::as_i64).unwrap_or(0)));
    state["total"] = json!(total);
    if previous == Some(total) {
        return None;
    }
    let reset = previous.is_some_and(|old| total.iter().zip(old).any(|(a, b)| *a < b));
    let delta = if info["last_token_usage"].is_object() {
        amounts(&info["last_token_usage"])
    } else if let Some(old) = previous.filter(|_| !reset) {
        std::array::from_fn(|i| total[i].saturating_sub(old[i]))
    } else {
        total
    };
    if delta.iter().all(|n| *n == 0) {
        return None;
    }
    let session = state["session"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stem(path));
    let ts = timestamp(v, "timestamp")?;
    let key = hash(&format!("{ts}:{total:?}"));
    let mut e = event(
        "codex",
        session,
        session,
        ts,
        string(state, "model"),
        format!("codex:{key}"),
    );
    e.cwd = string(state, "cwd").to_owned();
    e.fresh = delta[0].saturating_sub(delta[1].saturating_add(delta[2]));
    e.read = delta[1];
    e.write5 = delta[2];
    e.output = delta[3];
    Some(e)
}

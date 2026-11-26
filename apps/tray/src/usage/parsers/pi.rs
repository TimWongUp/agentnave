use super::{event, number, stem, string, timestamp};
use crate::usage::Event;
use serde_json::{Value, json};
use std::path::Path;

/// Pi (and omp) write one JSON entry per line; usage is `input` without the cache,
/// `output` with the reasoning. Entries before a fork's own header are its parent's.
pub(super) fn parse(path: &Path, v: &Value, state: &mut Value) -> Option<Event> {
    let usage = match string(v, "type") {
        "session" => {
            if state["header"] != true {
                state["header"] = json!(true);
                state["session"] = json!(string(v, "id"));
                let cwd = string(v, "cwd");
                if !cwd.is_empty() {
                    state["cwd"] = json!(cwd);
                }
                if !string(v, "parentSession").is_empty() {
                    state["since"] = json!(timestamp(v, "timestamp").unwrap_or(0));
                }
            }
            return None;
        }
        "model_change" => {
            let model = match string(v, "modelId") {
                "" => string(v, "model").split_once('/').map_or("", |(_, m)| m),
                id => id,
            };
            if !model.is_empty() {
                state["model"] = json!(model);
            }
            return None;
        }
        "message" => {
            let message = &v["message"];
            if string(message, "role") == "assistant" && !string(message, "model").is_empty() {
                state["model"] = json!(string(message, "model"));
            }
            &message["usage"]
        }
        "usage" | "model_usage" => {
            if !string(v, "model").is_empty() {
                state["model"] = json!(string(v, "model"));
            }
            &v["usage"]
        }
        "compaction" | "branch_summary" => &v["usage"],
        _ => return None,
    };
    let model = string(state, "model");
    let ts = timestamp(v, "timestamp")?;
    if model.is_empty() || !usage.is_object() || ts < state["since"].as_i64().unwrap_or(0) {
        return None;
    }
    let session = state["session"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| stem(path).split_once('_').map(|(_, id)| id))
        .unwrap_or_else(|| stem(path));
    let id = string(v, "id");
    let key = if id.is_empty() {
        format!("pi:{session}:{ts}")
    } else {
        format!("pi:{id}:{ts}")
    };
    let mut e = event("pi", session, session, ts, model, key);
    e.cwd = string(state, "cwd").to_owned();
    e.fresh = number(usage, "input");
    e.read = number(usage, "cacheRead");
    e.write5 = number(usage, "cacheWrite");
    e.output = number(usage, "output");
    (e.total() > 0).then_some(e)
}

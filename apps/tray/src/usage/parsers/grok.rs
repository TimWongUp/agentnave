use super::{epoch_millis, event, number, string};
use crate::usage::Event;
use serde_json::{Value, json};
use std::path::Path;

/// What of `summary.json` the turns are counted under: the model an empty
/// `modelUsage` falls back to, the session a subagent belongs to, and for a fork
/// when it began (what it copied from its parent is older).
fn summary(path: &Path, state: &mut Value) -> Option<()> {
    if state["summary"] == true {
        return Some(());
    }
    let summary: Value = crate::usage::read_small_json(&path.parent()?.join("summary.json"))?;
    state["summary"] = json!(true);
    state["model"] = json!(string(&summary, "current_model_id"));
    let parent = string(&summary, "parent_session_id");
    if !parent.is_empty() {
        let began = chrono::DateTime::parse_from_rfc3339(string(&summary, "created_at"))
            .map_or(0, |t| t.timestamp_millis());
        state["since"] = json!(began);
        if string(&summary, "session_kind").starts_with("subagent") {
            state["root"] = json!(parent);
        }
    }
    Some(())
}

/// One `turn_completed` is one event per model in `usage.modelUsage`; `inputTokens`
/// includes the cache read and written.
pub(super) fn parse(path: &Path, v: &Value, state: &mut Value) -> Vec<Event> {
    let update = &v["params"]["update"];
    let usage = &update["usage"];
    if string(update, "sessionUpdate") != "turn_completed" || !usage.is_object() {
        return Vec::new();
    }
    let Some(ts) = v["timestamp_us"]
        .as_i64()
        .filter(|t| *t > 0)
        .map(|t| t / 1000)
        .or_else(|| epoch_millis(v, "timestamp"))
    else {
        return Vec::new();
    };
    let Some(session) = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    else {
        return Vec::new();
    };
    if summary(path, state).is_none() || ts < state["since"].as_i64().unwrap_or(0) {
        return Vec::new();
    }
    let root = state["root"].as_str().unwrap_or(session);
    let turn = [
        string(&v["params"]["_meta"], "eventId"),
        string(update, "prompt_id"),
    ]
    .into_iter()
    .find(|id| !id.is_empty())
    .map_or_else(|| ts.to_string(), str::to_owned);
    let fallback = json!({ string_or(state, "model"): usage });
    let by_model = usage["modelUsage"]
        .as_object()
        .filter(|m| !m.is_empty())
        .or(fallback.as_object());
    by_model
        .into_iter()
        .flatten()
        .filter_map(|(model, usage)| {
            let mut e = event(
                "grok",
                session,
                root,
                ts,
                model,
                format!("grok:{session}:{turn}:{model}"),
            );
            e.read = number(usage, "cachedReadTokens");
            e.write5 = number(usage, "cacheCreationTokens");
            e.fresh = number(usage, "inputTokens").saturating_sub(e.read.saturating_add(e.write5));
            e.output = number(usage, "outputTokens");
            (e.total() > 0).then_some(e)
        })
        .collect()
}

fn string_or(state: &Value, key: &str) -> String {
    match string(state, key) {
        "" => "grok".to_owned(),
        value => value.to_owned(),
    }
}

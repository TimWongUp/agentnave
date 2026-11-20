use super::{event, number, stem, string};
use crate::usage::Event;
use serde_json::{Value, json};
use std::path::Path;

/// One session file: a header, then `{type, seq, time, data}` events. A fork's seed
/// (events before `session/end-seed`, or `seq < seedLength` before format 2) was
/// counted in the session it came from. `inputTokens` excludes the cache.
pub(super) fn parse(path: &Path, v: &Value, state: &mut Value) -> Option<Event> {
    match string(v, "type") {
        "session" => {
            let id = string(v, "id");
            state["session"] = json!(id);
            state["version"] = json!(v["version"].as_i64().unwrap_or(0));
            state["seed_length"] = json!(v["seedLength"].as_i64().unwrap_or(0));
            state["seeding"] =
                json!(v["version"].as_i64().unwrap_or(0) >= 2 && v["isSeeded"] == true);
            let parent = string(v, "parentSession");
            let nested = v["delegationDepth"].as_i64().unwrap_or(0) > 0 && !parent.is_empty();
            state["root"] = json!(if nested { parent } else { id });
            return None;
        }
        "session/end-seed" => {
            state["seeding"] = json!(false);
            return None;
        }
        "assistant/message" => {}
        _ => return None,
    }
    let seq = v["seq"].as_i64();
    if state["seeding"] == true
        || (state["version"].as_i64().unwrap_or(0) < 2
            && seq.is_some_and(|n| n < state["seed_length"].as_i64().unwrap_or(0)))
    {
        return None;
    }
    let data = &v["data"];
    let usage = &data["usage"];
    let source = &data["message"]["source"];
    let response = &source["replayState"]["response"];
    let model = [
        string(response, "responseModel"),
        string(response, "model"),
        string(source, "model"),
    ]
    .into_iter()
    .find(|m| !m.is_empty())?;
    let ts = v["time"].as_i64().filter(|t| *t > 0)?;
    if !usage.is_object() {
        return None;
    }
    let session = state["session"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stem(path));
    let root = state["root"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(session);
    let at = seq.map_or_else(|| ts.to_string(), |n| n.to_string());
    let mut e = event(
        "deepseek",
        session,
        root,
        ts,
        model,
        format!("deepseek:{session}:{at}"),
    );
    e.fresh = number(usage, "inputTokens");
    e.read = number(usage, "cacheReadTokens");
    e.write5 = number(usage, "cacheWriteTokens");
    e.output = number(usage, "outputTokens");
    e.unknown = usage.get("cacheReadTokens").is_none();
    (e.total() > 0).then_some(e)
}

use super::{epoch_millis, event, number, string};
use crate::usage::Event;
use serde_json::{Value, json};
use std::path::Path;

/// Kimi Code `wire.jsonl` parser (`usage.record` lines).
/// Field semantics ported from yetone/magpie `internal/sessions/calls_kimi.go` & `kimi.go` (MIT).
/// Each call writes `inputOther` (fresh), `output` (incl. thinking), `inputCacheRead`,
/// and `inputCacheCreation`. Forks copy parent records verbatim, so the key hashes agent, time,
/// model, and all four counters.
pub(super) fn parse(path: &Path, v: &Value, state: &mut Value) -> Option<Event> {
    if string(v, "type") != "usage.record" {
        return None;
    }
    let usage = &v["usage"];
    let model = string(v, "model");
    if !usage.is_object() || model.is_empty() {
        return None;
    }
    let ts = epoch_millis(v, "time")?;
    load_state_cwd(path, state);
    let agent_dir = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("main");
    let session = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or(agent_dir);
    let agent = match string(v, "agentId") {
        "" => agent_dir,
        id => id,
    };
    let fresh = number(usage, "inputOther");
    let output = number(usage, "output");
    let read = number(usage, "inputCacheRead");
    let write5 = number(usage, "inputCacheCreation");
    let key = format!("kimi:{agent}:{ts}:{model}:{fresh}/{output}/{read}/{write5}");
    let mut e = event("kimi", session, session, ts, model, key);
    e.cwd = string(state, "cwd").to_owned();
    e.fresh = fresh;
    e.read = read;
    e.write5 = write5;
    e.output = output;
    (e.total() > 0).then_some(e)
}

fn load_state_cwd(wire_path: &Path, state: &mut Value) {
    if state["meta_loaded"] == true {
        return;
    }
    state["meta_loaded"] = json!(true);
    // Layout: sessions/<workdir-slug>/<session-id>/agents/<agent>/wire.jsonl
    let Some(session_dir) = wire_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return;
    };
    let Some(meta) = crate::usage::read_small_json(&session_dir.join("state.json")) else {
        return;
    };
    let cwd = meta["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| meta["workDir"].as_str().filter(|s| !s.is_empty()));
    if let Some(cwd) = cwd {
        state["cwd"] = json!(cwd);
    }
}

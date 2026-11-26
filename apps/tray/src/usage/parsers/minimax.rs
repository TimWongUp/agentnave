use super::{epoch_millis, event, number, string};
use crate::usage::Event;
use base64::Engine as _;
use serde_json::{Value, json};
use std::path::Path;

/// MiniMax Code `messages*.jsonl` parser.
/// Field semantics ported from yetone/magpie `internal/sessions/calls_minimax.go` (MIT).
/// Assistant messages carry `usage.{input, output, cacheRead, cacheWrite}` (input excludes cache).
pub(super) fn parse(path: &Path, v: &Value, state: &mut Value) -> Option<Event> {
    let line_cwd = v["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| v["message"]["cwd"].as_str().filter(|s| !s.is_empty()));
    if let Some(cwd) = line_cwd {
        state["cwd"] = json!(cwd);
    }
    let message = &v["message"];
    if string(message, "role") != "assistant" {
        return None;
    }
    let usage = &message["usage"];
    let model = string(message, "model");
    if !usage.is_object() || model.is_empty() {
        return None;
    }
    let ts = epoch_millis(message, "timestamp").or_else(|| epoch_millis(v, "timestamp"))?;
    let dir_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let session = decode_session_id(dir_name);
    let msg_id = string(v, "message_id");
    let key = if msg_id.is_empty() {
        format!("minimax:{session}:{ts}:{model}")
    } else {
        format!("minimax:{msg_id}")
    };
    let mut e = event("minimax", &session, &session, ts, model, key);
    e.cwd = string(state, "cwd").to_owned();
    e.fresh = number(usage, "input");
    e.read = number(usage, "cacheRead");
    e.write5 = number(usage, "cacheWrite");
    e.output = number(usage, "output");
    (e.total() > 0).then_some(e)
}

fn decode_session_id(dir: &str) -> String {
    let Some((_, enc)) = dir.split_once("-session_") else {
        return dir.to_owned();
    };
    for engine in [
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::STANDARD,
    ] {
        if let Ok(bytes) = engine.decode(enc)
            && !bytes.is_empty()
            && let Ok(text) = String::from_utf8(bytes)
        {
            return text;
        }
    }
    enc.to_owned()
}

use super::{event, number, stem, string, timestamp};
use crate::usage::Event;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn parse(provider: &str, path: &Path, v: &Value, state: &mut Value) -> Option<Event> {
    let line_cwd = string(v, "cwd");
    if !line_cwd.is_empty() {
        state["cwd"] = json!(line_cwd);
    }
    let message = &v["message"];
    let id = string(message, "id");
    let model = string(message, "model");
    if id.is_empty() || model.is_empty() || model == "<synthetic>" || !message["usage"].is_object()
    {
        return None;
    }
    let effective_provider = if provider == "claude"
        && matches!(
            string(v, "entrypoint"),
            "claude-desktop" | "claude-desktop-3p"
        ) {
        "claude-desktop"
    } else {
        provider
    };
    let session = v["sessionId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stem(path));
    let parent = path.parent()?;
    let root = if parent.file_name().is_some_and(|n| n == "subagents") {
        parent.parent()?.file_name()?.to_str()?
    } else {
        session
    };
    let usage = &message["usage"];
    let mut e = event(
        effective_provider,
        session,
        root,
        timestamp(v, "timestamp")?,
        model,
        format!("claude:{id}"),
    );
    e.cwd = if !line_cwd.is_empty() {
        line_cwd.to_owned()
    } else {
        string(state, "cwd").to_owned()
    };
    e.fresh = number(usage, "input_tokens");
    e.read = number(usage, "cache_read_input_tokens");
    let creation = &usage["cache_creation"];
    e.write5 = number(creation, "ephemeral_5m_input_tokens");
    e.write1 = number(creation, "ephemeral_1h_input_tokens");
    if e.write5.saturating_add(e.write1) == 0 {
        e.write5 = number(usage, "cache_creation_input_tokens");
    }
    e.output = number(usage, "output_tokens");
    if e.total() == 0 {
        return None;
    }
    Some(e)
}

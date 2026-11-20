use super::{event, number, stem, string};
use crate::usage::Event;
use serde_json::Value;
use std::path::Path;

fn cached(usage: &Value) -> i64 {
    ["inputTokensDetails", "input_tokens_details"]
        .iter()
        .map(|key| match &usage[*key] {
            Value::Array(items) => items
                .iter()
                .map(|d| number(d, "cached_tokens"))
                .fold(0i64, i64::saturating_add),
            details => number(details, "cached_tokens"),
        })
        .fold(0i64, i64::saturating_add)
}

fn either(usage: &Value, key: &str, old: &str) -> i64 {
    number(usage, key).max(number(usage, old))
}

/// CodeBuddy's `inputTokens` includes the cache read and written. Lines of one reply
/// repeat its `messageId`, so the last one saved replaces the earlier ones.
pub(super) fn parse(path: &Path, v: &Value) -> Option<Event> {
    let data = &v["providerData"];
    let usage = &data["usage"];
    let model = match string(data, "model") {
        "" => string(data, "requestModelId"),
        model => model,
    };
    if !usage.is_object() || model.is_empty() {
        return None;
    }
    let ts = super::epoch_millis(v, "timestamp")?;
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
    let message = string(data, "messageId");
    let key = if message.is_empty() {
        format!("codebuddy:{session}:{ts}:{}", string(v, "id"))
    } else {
        format!("codebuddy:{message}")
    };
    let mut e = event("codebuddy", session, root, ts, model, key);
    let input = either(usage, "inputTokens", "input_tokens");
    e.read = cached(usage);
    e.write5 = either(usage, "cacheCreationTokens", "cache_creation_input_tokens");
    e.fresh = input.saturating_sub(e.read.saturating_add(e.write5));
    e.output = either(usage, "outputTokens", "output_tokens");
    (e.total() > 0).then_some(e)
}

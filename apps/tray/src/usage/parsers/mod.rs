pub(super) mod antigravity;
mod claude;
mod codebuddy;
mod codex;
mod deepseek;
mod grok;
mod kimi;
mod minimax;
mod pi;

use super::Event;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

const PREFILTER_THRESHOLD: usize = 8192;

pub(super) fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str) -> i64 {
    v[key].as_i64().unwrap_or(0).max(0)
}
fn timestamp(v: &Value, key: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(string(v, key))
        .ok()
        .map(|t| t.timestamp_millis())
}
pub(super) fn event(
    provider: &str,
    session: &str,
    root: &str,
    ts: i64,
    model: &str,
    key: String,
) -> Event {
    Event {
        key,
        provider: provider.to_owned(),
        session: session.to_owned(),
        root: root.to_owned(),
        thread: hash(&format!("{provider}\0{root}"))[..16].to_owned(),
        ts,
        model: model.to_owned(),
        calls: 1,
        ..Event::default()
    }
}

/// Seconds or milliseconds since the epoch, as a millisecond timestamp.
fn epoch_millis(v: &Value, key: &str) -> Option<i64> {
    let n = v[key].as_i64().filter(|n| *n > 0)?;
    Some(if n > 100_000_000_000 { n } else { n * 1000 })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    if needle.len() > haystack.len() {
        return false;
    }
    let limit = haystack.len() - needle.len();
    let mut i = 0;
    while i <= limit {
        if haystack[i] == first && &haystack[i + 1..i + needle.len()] == rest {
            return true;
        }
        i += 1;
    }
    false
}

/// Fast byte pre-filter for oversized JSONL lines (> 8 KiB) so huge tool outputs or base64 blobs
/// never allocate a `serde_json::Value` tree.
pub(super) fn may_contain_relevant(provider: &str, line: &[u8]) -> bool {
    if line.len() <= PREFILTER_THRESHOLD {
        return true;
    }
    let markers: &[&[u8]] = match provider {
        "claude" | "claude-desktop" => &[b"\"usage\""],
        "codebuddy" | "workbuddy" => &[b"\"usage\"", b"\"providerData\""],
        "codex" => &[b"token_count", b"session_meta", b"turn_context"],
        "antigravity" => &[b"\"MODEL\""],
        "pi" => &[b"\"usage\"", b"\"model_change\"", b"\"session\""],
        "deepseek" => &[b"\"usage\"", b"\"session\"", b"end-seed"],
        "grok" => &[b"turn_completed"],
        "kimi" => &[b"usage.record"],
        "minimax" => &[b"\"usage\""],
        _ => return true,
    };
    markers.iter().any(|needle| contains_bytes(line, needle))
}

#[cfg(test)]
pub(super) fn parse(provider: &str, path: &Path, line: &[u8], state: &mut Value) -> Vec<Event> {
    parse_with(provider, path, line, state, None)
}

pub(super) fn parse_with(
    provider: &str,
    path: &Path,
    line: &[u8],
    state: &mut Value,
    agy_cwds: Option<&HashMap<String, String>>,
) -> Vec<Event> {
    if !may_contain_relevant(provider, line) {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    let one = match provider {
        "claude" | "claude-desktop" => claude::parse(provider, path, &v, state),
        "codex" => codex::parse(path, &v, state),
        "antigravity" => antigravity::parse(path, &v, agy_cwds),
        "pi" => pi::parse(path, &v, state),
        "codebuddy" | "workbuddy" => codebuddy::parse(provider, path, &v, state),
        "deepseek" => deepseek::parse(path, &v, state),
        "grok" => return grok::parse(path, &v, state),
        "kimi" => kimi::parse(path, &v, state),
        "minimax" => minimax::parse(path, &v, state),
        _ => None,
    };
    one.into_iter().collect()
}

fn stem(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}
pub(super) fn initial() -> Value {
    json!({})
}

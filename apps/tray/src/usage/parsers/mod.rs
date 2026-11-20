mod antigravity;
mod claude;
mod codebuddy;
mod codex;
mod deepseek;
mod grok;
mod pi;

use super::Event;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

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
        ..Event::default()
    }
}

/// Seconds or milliseconds since the epoch, as a millisecond timestamp.
fn epoch_millis(v: &Value, key: &str) -> Option<i64> {
    let n = v[key].as_i64().filter(|n| *n > 0)?;
    Some(if n > 100_000_000_000 { n } else { n * 1000 })
}

pub(super) fn parse(provider: &str, path: &Path, line: &[u8], state: &mut Value) -> Vec<Event> {
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    let one = match provider {
        "claude" => claude::parse(path, &v),
        "codex" => codex::parse(path, &v, state),
        "antigravity" => antigravity::parse(path, &v),
        "pi" => pi::parse(path, &v, state),
        "codebuddy" => codebuddy::parse(path, &v),
        "deepseek" => deepseek::parse(path, &v, state),
        "grok" => return grok::parse(path, &v, state),
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

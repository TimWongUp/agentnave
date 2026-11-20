mod antigravity;
mod claude;
mod codex;

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
fn event(provider: &str, session: &str, root: &str, ts: i64, model: &str, key: String) -> Event {
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

pub(super) fn parse(provider: &str, path: &Path, line: &[u8], state: &mut Value) -> Option<Event> {
    let v: Value = serde_json::from_slice(line).ok()?;
    match provider {
        "claude" => claude::parse(path, &v),
        "codex" => codex::parse(path, &v, state),
        "antigravity" => antigravity::parse(path, &v),
        _ => None,
    }
}

fn stem(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}
pub(super) fn initial() -> Value {
    json!({})
}

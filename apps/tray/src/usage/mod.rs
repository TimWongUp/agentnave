//! Local CLI usage, owned entirely by the tray.
mod db;
mod frames;
mod parsers;
mod pricing;
mod pricing_refresh;
mod query;
mod scan;
#[cfg(test)]
mod tests;
mod zcode;

use crate::{data::Reader, response::Response};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

pub(crate) const COVERED_PROVIDERS: [&str; 8] = [
    "claude",
    "codex",
    "antigravity",
    "pi",
    "codebuddy",
    "deepseek",
    "grok",
    "zcode",
];

const MAX_META_JSON: u64 = 1024 * 1024;

static SCANNING: AtomicBool = AtomicBool::new(false);
static UPDATED: AtomicI64 = AtomicI64::new(0);

struct ScanningGuard;

impl Drop for ScanningGuard {
    fn drop(&mut self) {
        SCANNING.store(false, Ordering::Release);
    }
}

pub(super) fn read_small_json(path: &Path) -> Option<Value> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_META_JSON + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_META_JSON {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[derive(Clone, Debug, Default)]
struct Event {
    key: String,
    provider: String,
    session: String,
    root: String,
    thread: String,
    ts: i64,
    model: String,
    fresh: i64,
    read: i64,
    write5: i64,
    write1: i64,
    output: i64,
    unknown: bool,
}

impl Event {
    fn prompt(&self) -> i64 {
        self.fresh
            .saturating_add(self.read)
            .saturating_add(self.write5)
            .saturating_add(self.write1)
    }
    fn total(&self) -> i64 {
        self.prompt().saturating_add(self.output)
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

const MIN_REFRESH_MS: i64 = 10_000;
const BACKGROUND_POLL: Duration = Duration::from_secs(600);

fn refresh(force: bool, homes: impl FnOnce() -> crate::data::boss_app::Homes) {
    pricing_refresh::maybe_refresh();
    if !force && now() - UPDATED.load(Ordering::Relaxed) < MIN_REFRESH_MS {
        return;
    }
    if SCANNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let homes = homes();
    std::thread::spawn(move || {
        let _guard = ScanningGuard;
        if let Err(error) = scan::run(&crate::data_files::data_root(), &homes) {
            eprintln!("用量扫描失败：{error}");
        }
        UPDATED.store(now(), Ordering::Relaxed);
    });
}

pub(crate) fn start(reader: Arc<Reader>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        loop {
            refresh(false, || reader.boss_homes());
            std::thread::sleep(BACKGROUND_POLL);
        }
    });
}

pub(crate) fn request(
    reader: &Reader,
    method: &str,
    path: &str,
    query: Option<&str>,
    action: Option<&str>,
) -> Response {
    if path == "/app/usage/refresh" {
        return crate::response::action(method, action, || {
            refresh(true, || reader.boss_homes());
            Response::json(&json!({"ok": true, "scanning": true}))
        });
    }
    if method != "GET" {
        return Response::error(405, "此路由只接受 GET");
    }
    if path == "/app/usage/workspace" {
        let Some(group) = query_group(query) else {
            return Response::error(400, "缺少工作区");
        };
        return match query::workspace(reader, &group) {
            Ok(v) => Response::json(&v),
            Err(e) => Response::error(500, &e.to_string()),
        };
    }
    let range = query
        .unwrap_or("")
        .split('&')
        .find_map(|part| part.strip_prefix("range="))
        .unwrap_or("today");
    if !matches!(range, "today" | "7d" | "30d") {
        return Response::error(400, "无效的时间范围");
    }
    refresh(false, || reader.boss_homes());
    match query::respond(reader, range) {
        Ok(v) => Response::json(&v),
        Err(e) => Response::error(500, &e.to_string()),
    }
}

fn query_group(query: Option<&str>) -> Option<String> {
    let raw = query?
        .split('&')
        .find_map(|part| part.strip_prefix("group="))?;
    let group = percent_decode(raw)?;
    (!group.is_empty()).then_some(group)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            out.push((hex(bytes[i + 1])? << 4) | hex(bytes[i + 2])?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(byte: u8) -> Option<u8> {
    Some(match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    })
}

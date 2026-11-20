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
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

static SCANNING: AtomicBool = AtomicBool::new(false);
static UPDATED: AtomicI64 = AtomicI64::new(0);

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
        self.fresh + self.read + self.write5 + self.write1
    }
    fn total(&self) -> i64 {
        self.prompt() + self.output
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

const MIN_REFRESH_MS: i64 = 10_000;
const BACKGROUND_POLL: Duration = Duration::from_secs(600);

fn refresh(force: bool) {
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
    std::thread::spawn(|| {
        if let Err(error) = scan::run(&crate::data_files::data_root()) {
            eprintln!("用量扫描失败：{error}");
        }
        UPDATED.store(now(), Ordering::Relaxed);
        SCANNING.store(false, Ordering::Release);
    });
}

pub(crate) fn start() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(5));
        loop {
            refresh(false);
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
            refresh(true);
            Response::json(&json!({"ok": true, "scanning": true}))
        });
    }
    if method != "GET" {
        return Response::error(405, "此路由只接受 GET");
    }
    let range = query
        .unwrap_or("")
        .split('&')
        .find_map(|part| part.strip_prefix("range="))
        .unwrap_or("today");
    if !matches!(range, "today" | "7d" | "30d") {
        return Response::error(400, "无效的时间范围");
    }
    refresh(false);
    match query::respond(reader, range) {
        Ok(v) => Response::json(&v),
        Err(e) => Response::error(500, &e.to_string()),
    }
}

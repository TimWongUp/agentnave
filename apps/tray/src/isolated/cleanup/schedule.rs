//! When passes run: once after the tray starts, then hourly, and on the user's request.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::report::{self, Trigger};
use super::{Env, Report, Rules, run};
use crate::data::Reader;
use crate::git_pr::gh::GhClient;
use crate::response::{self, Response};
use crate::terminal::Terminals;

/// Let the tray settle (history scan, host refresh) before the first pass.
const FIRST_DELAY: Duration = Duration::from_secs(60);
const INTERVAL: Duration = Duration::from_secs(60 * 60);

static RUNNING: AtomicBool = AtomicBool::new(false);

/// One pass at a time; the guard frees the slot when dropped.
struct Slot;

impl Slot {
    fn take() -> Option<Self> {
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// A preview build never changes production data, so it never cleans.
pub(crate) fn start(app: AppHandle, reader: Arc<Reader>) {
    if crate::PREVIEW {
        return;
    }
    thread::spawn(move || {
        thread::sleep(FIRST_DELAY);
        loop {
            pass(&app, &reader, Trigger::Auto);
            thread::sleep(INTERVAL);
        }
    });
}

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
}

fn current_rules() -> Rules {
    Rules::from_document(&crate::preferences::read(&crate::preferences::dir()).unwrap_or_default())
}

/// Run one pass and remember its report. `None` when another pass is running, or when the
/// scheduler has nothing to do because no rule is on.
fn pass(app: &AppHandle, reader: &Reader, trigger: Trigger) -> Option<Report> {
    let rules = current_rules();
    if trigger == Trigger::Auto && !rules.any() {
        return None;
    }
    let _slot = Slot::take()?;
    let inputs = reader.isolated_cleanup_inputs();
    let terminals = app.state::<Terminals>();
    let terminal_busy = |record: &super::Record| {
        terminals.has_active_terminal_for_isolated(&record.id, Path::new(&record.worktree_path))
    };
    let gh = GhClient::system();
    let env = Env {
        root: reader.data_root(),
        gh: &gh,
        now: now_micros(),
        rules,
        inputs: &inputs,
        terminal_busy: &terminal_busy,
    };
    let report = run(&env, trigger);
    report::remember(&report);
    Some(report)
}

fn state() -> Value {
    json!({
        "running": RUNNING.load(Ordering::Acquire),
        "report": report::last().map(|report| report.to_json()),
        "recent": report::recent_json(),
    })
}

/// `GET /app/isolated/cleanup` and `POST /app/isolated/cleanup/run`.
pub(crate) fn request(
    app: &AppHandle,
    reader: &Reader,
    method: &str,
    path: &str,
    action: Option<&str>,
) -> Response {
    match path {
        "/app/isolated/cleanup" if method == "GET" => Response::json(&state()),
        "/app/isolated/cleanup/run" => response::action(method, action, || {
            if crate::PREVIEW {
                return Response::error(403, "预览版只读：请在正式版里保存改动");
            }
            match pass(app, reader, Trigger::Manual) {
                Some(_) => Response::json(&state()),
                None => Response::error(409, "清理正在进行"),
            }
        }),
        _ => Response::not_found(),
    }
}

//! The embedded terminal (ADR-0101): installed CLIs the user starts from the App, each in a folder
//! they picked. They are the user's own processes: the service never sees them, nothing is
//! written to the conversation history, and they end with the App.
//!
//! Pages read output by long polling and send keys, size changes, stop and close as action POSTs,
//! all over the `agentnave` scheme like every other page. Every route but the list names its
//! terminal with `?session=<id>`.

mod session;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::providers::PROVIDERS;
use crate::response::{self, Response};
use session::{Launch, Session};

/// How long an output request waits for new output before answering empty.
const POLL: Duration = Duration::from_secs(20);
/// A terminal that wrote within this long counts as working.
const ACTIVE: Duration = Duration::from_millis(2500);

#[derive(Default)]
pub(crate) struct Terminals {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    open: Vec<Running>,
    /// The folder last picked or used, offered for the next start.
    cwd: Option<PathBuf>,
    started: u64,
}

struct Running {
    id: u64,
    provider: String,
    cwd: PathBuf,
    session: Arc<Session>,
}

impl Running {
    fn describe(&self) -> Value {
        json!({"id": self.id, "provider": self.provider, "cwd": self.cwd.display().to_string(),
            "exit": self.session.exit(), "active": self.session.wrote_within(ACTIVE)})
    }
}

pub(crate) fn request(
    app: &AppHandle,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let terminals = app.state::<Terminals>();
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let id = number(query, "session");
    match (method, route) {
        ("GET", "/app/terminal/state") => Response::json(&terminals.describe()),
        ("GET", "/app/terminal/clis") => Response::json(&installed()),
        ("GET", "/app/terminal/output") => terminals.output(id, number(query, "from")),
        (_, "/app/terminal/pick") => response::action(method, action, || terminals.pick(app)),
        (_, "/app/terminal/start") => {
            response::post(method, action, body, |payload| terminals.start(&payload))
        }
        (_, "/app/terminal/input") => response::action(method, action, || {
            terminals.with_session(id, |session| session.write(body))
        }),
        (_, "/app/terminal/resize") => response::post(method, action, body, |payload| {
            let (cols, rows) = cells(&payload);
            terminals.with_session(id, |session| session.resize(cols, rows))
        }),
        (_, "/app/terminal/stop") => response::action(method, action, || {
            terminals.with_session(id, |session| {
                session.stop();
                Ok(())
            })
        }),
        (_, "/app/terminal/close") => response::action(method, action, || terminals.close(id)),
        _ => Response::not_found(),
    }
}

/// Ends every CLI when the App quits.
pub(crate) fn shutdown(app: &AppHandle) {
    for running in &app.state::<Terminals>().lock().open {
        running.session.stop();
    }
}

fn number(query: &str, name: &str) -> Option<u64> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .and_then(|value| value.parse().ok())
}

impl Terminals {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn describe(&self) -> Value {
        let state = self.lock();
        let cwd = state.cwd.clone().or_else(dirs::home_dir);
        let open: Vec<Value> = state.open.iter().map(Running::describe).collect();
        json!({"sessions": open, "cwd": cwd.map(|cwd| cwd.display().to_string())})
    }

    fn pick(&self, app: &AppHandle) -> Response {
        match crate::video_packs::picker::choose(app) {
            Ok(Some(cwd)) => {
                self.lock().cwd = Some(cwd);
                Response::json(&self.describe())
            }
            Ok(None) => Response::json(&self.describe()),
            Err(error) => Response::error(500, &error),
        }
    }

    fn start(&self, payload: &Value) -> Response {
        let provider = payload["provider"].as_str().unwrap_or_default();
        if !PROVIDERS.contains(&provider) {
            return Response::error(400, "未知的 CLI");
        }
        let Some(program) = installed_path(provider) else {
            return Response::error(400, "这个 CLI 没有安装");
        };
        let cwd = PathBuf::from(payload["cwd"].as_str().unwrap_or_default());
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Response::error(400, "文件夹不存在");
        }
        let (cols, rows) = cells(payload);
        let launch = Launch {
            program,
            cwd: cwd.clone(),
            cols,
            rows,
        };
        let session = match Session::spawn(&launch) {
            Ok(session) => session,
            Err(error) => return Response::error(500, &error),
        };
        let mut state = self.lock();
        state.started += 1;
        state.cwd = Some(cwd.clone());
        let running = Running {
            id: state.started,
            provider: provider.to_owned(),
            cwd,
            session,
        };
        let described = running.describe();
        state.open.push(running);
        Response::json(&json!({"session": described}))
    }

    fn session(&self, id: Option<u64>) -> Option<Arc<Session>> {
        let state = self.lock();
        let running = state.open.iter().find(|running| Some(running.id) == id)?;
        Some(running.session.clone())
    }

    fn with_session(
        &self,
        id: Option<u64>,
        act: impl FnOnce(&Session) -> Result<(), String>,
    ) -> Response {
        let Some(session) = self.session(id) else {
            return Response::error(404, "找不到这个终端");
        };
        match act(&session) {
            Ok(()) => Response::json(&json!({"ok": true})),
            Err(error) => Response::error(500, &error),
        }
    }

    /// Ends the terminal's CLI if it still runs and forgets the terminal.
    fn close(&self, id: Option<u64>) -> Response {
        let mut state = self.lock();
        let Some(at) = state.open.iter().position(|running| Some(running.id) == id) else {
            return Response::error(404, "找不到这个终端");
        };
        state.open.remove(at).session.stop();
        Response::json(&json!({"ok": true}))
    }

    /// Output after `from`; the bytes go as they are, with their place in the headers.
    fn output(&self, id: Option<u64>, from: Option<u64>) -> Response {
        let Some(session) = self.session(id) else {
            return Response::error(404, "找不到这个终端");
        };
        let chunk = session.read(from.unwrap_or(0), POLL);
        let mut reply = Response::ok("application/octet-stream", chunk.bytes);
        reply.headers = vec![
            ("X-Terminal-Start", chunk.start.to_string()),
            ("X-Terminal-End", chunk.end.to_string()),
            (
                "X-Terminal-Exit",
                chunk.exit.map(|code| code.to_string()).unwrap_or_default(),
            ),
        ];
        reply
    }
}

fn cells(payload: &Value) -> (u16, u16) {
    let cells = |key: &str, default: u16| {
        payload[key]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(default)
    };
    (cells("cols", 80), cells("rows", 24))
}

/// The CLIs detection found on this machine, in the App's usual order.
fn installed() -> Value {
    let detected = crate::detect::detected(false, crate::members::codex_source);
    let clis: Vec<Value> = PROVIDERS
        .iter()
        .filter(|provider| detected[**provider]["installed"] == true)
        .filter(|provider| detected[**provider]["path"].is_string())
        .map(|provider| json!({"provider": provider}))
        .collect();
    json!({"clis": clis})
}

fn installed_path(provider: &str) -> Option<PathBuf> {
    let detected = crate::detect::detected(false, crate::members::codex_source);
    detected[provider]["installed"]
        .as_bool()
        .filter(|installed| *installed)
        .and(detected[provider]["path"].as_str())
        .map(PathBuf::from)
}

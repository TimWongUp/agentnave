//! The embedded terminal (ADR-0101): an installed CLI the user starts from the App, in a folder
//! they picked. It is the user's own process: the service never sees it, nothing is written to
//! the conversation history, and it ends with the App.
//!
//! The page reads output by long polling and sends keys, size changes and stop as action POSTs,
//! all over the `agentnave` scheme like every other page.

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

#[derive(Default)]
pub(crate) struct Terminals {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    current: Option<Running>,
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

pub(crate) fn request(
    app: &AppHandle,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let terminals = app.state::<Terminals>();
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    match (method, route) {
        ("GET", "/app/terminal/state") => Response::json(&terminals.describe()),
        ("GET", "/app/terminal/clis") => Response::json(&installed()),
        ("GET", "/app/terminal/output") => terminals.output(query),
        (_, "/app/terminal/pick") => response::action(method, action, || terminals.pick(app)),
        (_, "/app/terminal/start") => {
            response::post(method, action, body, |payload| terminals.start(&payload))
        }
        (_, "/app/terminal/input") => response::action(method, action, || {
            terminals.with_session(|session| session.write(body))
        }),
        (_, "/app/terminal/resize") => response::post(method, action, body, |payload| {
            let (cols, rows) = cells(&payload);
            terminals.with_session(|session| session.resize(cols, rows))
        }),
        (_, "/app/terminal/stop") => response::action(method, action, || {
            terminals.with_session(|session| {
                session.stop();
                Ok(())
            })
        }),
        _ => Response::not_found(),
    }
}

/// Ends the running CLI when the App quits.
pub(crate) fn shutdown(app: &AppHandle) {
    if let Some(running) = &app.state::<Terminals>().lock().current {
        running.session.stop();
    }
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
        let current = state.current.as_ref().map(|running| {
            json!({"id": running.id, "provider": running.provider,
                "cwd": running.cwd.display().to_string(), "exit": running.session.exit()})
        });
        json!({"session": current, "cwd": cwd.map(|cwd| cwd.display().to_string())})
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
        let mut state = self.lock();
        if state
            .current
            .as_ref()
            .is_some_and(|running| running.session.exit().is_none())
        {
            return Response::error(409, "终端里的 CLI 还在运行");
        }
        let (cols, rows) = cells(payload);
        let launch = Launch {
            program,
            cwd: cwd.clone(),
            cols,
            rows,
        };
        match Session::spawn(&launch) {
            Ok(session) => {
                state.started += 1;
                state.cwd = Some(cwd.clone());
                state.current = Some(Running {
                    id: state.started,
                    provider: provider.to_owned(),
                    cwd,
                    session,
                });
                drop(state);
                Response::json(&self.describe())
            }
            Err(error) => Response::error(500, &error),
        }
    }

    fn session(&self) -> Option<(u64, Arc<Session>)> {
        let state = self.lock();
        let running = state.current.as_ref()?;
        Some((running.id, running.session.clone()))
    }

    fn with_session(&self, act: impl FnOnce(&Session) -> Result<(), String>) -> Response {
        let Some((_, session)) = self.session() else {
            return Response::error(404, "终端没有在运行");
        };
        match act(&session) {
            Ok(()) => Response::json(&json!({"ok": true})),
            Err(error) => Response::error(500, &error),
        }
    }

    /// Output after `from` of session `session`; another session starts over from its beginning.
    /// The bytes go as they are, with their place in the headers.
    fn output(&self, query: &str) -> Response {
        let Some((id, session)) = self.session() else {
            return Response::error(404, "终端没有在运行");
        };
        let number = |name: &str| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
                .and_then(|value| value.parse::<u64>().ok())
        };
        let from = if number("session") == Some(id) {
            number("from").unwrap_or(0)
        } else {
            0
        };
        let chunk = session.read(from, POLL);
        let mut reply = Response::ok("application/octet-stream", chunk.bytes);
        reply.headers = vec![
            ("X-Terminal-Session", id.to_string()),
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

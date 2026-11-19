//! The embedded terminal (ADR-0103): installed CLIs the user starts from the office, each in a
//! project's folder. They are the user's own processes and end with the App; a CLI that works
//! through AgentNave is the boss of the workspaces it dispatched from, which the office shows it
//! beside.
//!
//! Pages read output by long polling and send keys, size changes, stop and close as action POSTs,
//! all over the `agentnave` scheme like every other page. Every route but the list names its
//! terminal with `?session=<id>`.

mod links;
mod session;
#[cfg(test)]
mod tests;
mod theme;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    /// Workspaces this terminal's CLI has confirmed as boss, latest last. Kept after exit until close.
    workspaces: Vec<String>,
}

impl Running {
    fn describe(&self) -> Value {
        json!({"id": self.id, "provider": self.provider, "cwd": self.cwd.display().to_string(),
            "exit": self.session.exit(), "active": self.session.wrote_within(ACTIVE),
            "workspaces": self.workspaces, "project_key": crate::projects::OTHER})
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
    let light = number(query, "light") == Some(1);
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
        (_, "/app/terminal/restart") => {
            response::action(method, action, || terminals.restart(id, light))
        }
        (_, "/app/terminal/resume") => {
            let workspace = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("workspace="))
                .unwrap_or_default()
                .to_owned();
            response::action(method, action, || terminals.resume(&workspace, light))
        }
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
    pub(crate) fn cwds(&self) -> Vec<String> {
        self.lock()
            .open
            .iter()
            .map(|running| running.cwd.display().to_string())
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Updates confirmed workspaces from active relays under `root` and snapshots every open terminal.
    fn snapshot(&self, root: &Path) -> (Option<PathBuf>, Vec<(u64, PathBuf, Value)>) {
        let pids: Vec<(u64, u32)> = {
            let state = self.lock();
            state
                .open
                .iter()
                .filter(|running| running.session.exit().is_none())
                .filter_map(|running| Some((running.id, running.session.pid()?)))
                .collect()
        };
        let mut links = links::workspaces(root, &pids);
        let mut state = self.lock();
        for running in &mut state.open {
            if let Some(found) = links.remove(&running.id) {
                links::remember(&mut running.workspaces, found);
            }
        }
        let open = state
            .open
            .iter()
            .map(|running| (running.id, running.cwd.clone(), running.describe()))
            .collect();
        (state.cwd.clone().or_else(dirs::home_dir), open)
    }

    /// Every terminal with the workspaces its CLI is the boss of and the project of its folder.
    fn describe(&self) -> Value {
        let (cwd, mut open) = self.snapshot(&crate::data_files::data_root());
        let folders: BTreeMap<String, String> = open
            .iter()
            .map(|(id, cwd, _)| (id.to_string(), cwd.display().to_string()))
            .collect();
        let projects = crate::projects::lookup(&folders, false);
        for (id, _, value) in &mut open {
            if let Some(project) = projects.get(&id.to_string()) {
                value["project_key"] = json!(project.key);
                value["project_label"] = json!(project.label);
                value["project_path"] = json!(project.path);
            }
        }
        let open: Vec<Value> = open.into_iter().map(|(_, _, value)| value).collect();
        json!({"sessions": open, "cwd": cwd.map(|cwd| cwd.display().to_string())})
    }

    fn pick(&self, app: &AppHandle) -> Response {
        // A cancelled pick names no folder, so nothing starts.
        match crate::video_packs::picker::choose(app) {
            Ok(cwd) => Response::json(&json!({"cwd": cwd.map(|cwd| cwd.display().to_string())})),
            Err(error) => Response::error(500, &error),
        }
    }

    fn start(&self, payload: &Value) -> Response {
        let provider = payload["provider"].as_str().unwrap_or_default();
        if !PROVIDERS.contains(&provider) {
            return Response::error(400, "未知的 CLI");
        }
        let cwd = PathBuf::from(payload["cwd"].as_str().unwrap_or_default());
        let (cols, rows) = cells(payload);
        let light = payload["light"].as_bool().unwrap_or(false);
        self.launch(provider, cwd, (cols, rows), None, Vec::new(), light)
    }

    /// Starts an ended terminal's CLI again in its folder, in its place: continuing the chat it
    /// was the boss of when its CLI can, else afresh as a new agent.
    fn restart(&self, id: Option<u64>, light: bool) -> Response {
        let (provider, cwd, workspaces) = {
            let state = self.lock();
            let Some(running) = state.open.iter().find(|running| Some(running.id) == id) else {
                return Response::error(404, "找不到这个终端");
            };
            if running.session.exit().is_none() {
                return Response::error(409, "终端还在运行");
            }
            (
                running.provider.clone(),
                running.cwd.clone(),
                running.workspaces.clone(),
            )
        };
        let resume = workspaces
            .last()
            .filter(|_| session::resumes(&provider))
            .map(|workspace| chat_session(workspace, &provider).map(|(id, _)| id))
            .transpose();
        let resume = match resume {
            Ok(resume) => resume,
            Err(error) => return Response::error(409, error),
        };
        // Continuing keeps the workspaces it was the boss of; a fresh start is a new agent.
        let kept = if resume.is_some() {
            workspaces
        } else {
            Vec::new()
        };
        let reply = self.launch(&provider, cwd, (100, 30), resume.as_deref(), kept, light);
        if reply.status == 200 {
            self.lock().open.retain(|running| Some(running.id) != id);
        }
        reply
    }

    /// Continues the chat of a workspace that has no terminal here, in the folder it ran in. A
    /// chat already open in a terminal here is not started twice.
    fn resume(&self, workspace: &str, light: bool) -> Response {
        {
            let state = self.lock();
            let open = state.open.iter().find(|running| {
                running.session.exit().is_none()
                    && running.workspaces.last().map(String::as_str) == Some(workspace)
            });
            if let Some(running) = open {
                return Response::json(&json!({"session": running.describe()}));
            }
        }
        let Some((host, _, _)) = links::chat(&crate::data_files::data_root(), workspace) else {
            return Response::error(404, "找不到这段聊天的记录");
        };
        let (id, record) = match chat_session(workspace, &host) {
            Ok(found) => found,
            Err(error) => return Response::error(409, error),
        };
        let Some(cwd) = crate::data::boss_app::session_cwd(&record) else {
            return Response::error(409, "会话记录里没有它的文件夹");
        };
        let workspaces = vec![workspace.to_owned()];
        let reply = self.launch(&host, cwd, (100, 30), Some(&id), workspaces, light);
        if reply.status == 200 {
            // An ended terminal of this chat gives way to the new one.
            let mut state = self.lock();
            let latest = state.started;
            state.open.retain(|running| {
                running.id == latest
                    || running.workspaces.last().map(String::as_str) != Some(workspace)
            });
        }
        reply
    }

    fn launch(
        &self,
        provider: &str,
        cwd: PathBuf,
        (cols, rows): (u16, u16),
        resume: Option<&str>,
        workspaces: Vec<String>,
        light: bool,
    ) -> Response {
        let Some(program) = installed_path(provider) else {
            return Response::error(400, "这个 CLI 没有安装");
        };
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Response::error(400, "文件夹不存在");
        }
        let theme = theme::for_cli(provider, light);
        let launch = Launch {
            program,
            args: [session::cli_args(provider, resume), theme.args].concat(),
            env: theme.env,
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
            workspaces,
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

/// The native ID and session record of a workspace's boss chat, when its CLI is `provider` and
/// can continue it.
fn chat_session(workspace: &str, provider: &str) -> Result<(String, PathBuf), &'static str> {
    if !session::resumes(provider) {
        return Err("这个 CLI 还不能在 App 里续接");
    }
    let (host, key, file) =
        links::chat(&crate::data_files::data_root(), workspace).ok_or("找不到这段聊天的记录")?;
    if host != provider {
        return Err("这段聊天不是这个 CLI 的");
    }
    crate::data::boss_app::native_session(&host, &key, file.as_deref())
        .ok_or("找不到这段聊天的会话记录")
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

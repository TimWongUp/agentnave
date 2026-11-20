//! The embedded terminal (ADR-0103): installed CLIs the user starts from the office, each in a
//! project's folder. They are the user's own processes and end with the App; a CLI that works
//! through AgentNave is the boss of the workspaces it dispatched from, which the office shows it
//! beside.
//!
//! Pages read output by long polling and send keys, size changes, stop and close as action POSTs,
//! all over the `agentnave` scheme like every other page. Every route but the list names its
//! terminal with `?session=<id>`.

mod elsewhere;
mod links;
mod modes;
mod session;
#[cfg(test)]
mod tests;
mod theme;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::data::boss_app::CliTitle;
use crate::providers::PROVIDERS;
use crate::response::{self, Response};
use session::{Launch, Session};

/// How long an output request waits for new output before answering empty.
const POLL: Duration = Duration::from_secs(20);
/// A terminal that wrote within this long counts as working.
const ACTIVE: Duration = Duration::from_millis(2500);
/// Minimum interval between directory scans for a terminal whose session record is not yet located.
const TITLE_LOOKUP_TTL: Duration = Duration::from_secs(5);
/// Minimum interval between full connection-file + process-table scans on `/app/terminal/state`.
const LINKS_TTL: Duration = Duration::from_secs(5);

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
    links_checked: Option<(Instant, PathBuf)>,
}

struct Running {
    id: u64,
    provider: String,
    cwd: PathBuf,
    session: Arc<Session>,
    /// Workspaces this terminal's CLI has confirmed as boss, latest last. Kept after exit until close.
    workspaces: Vec<String>,
    /// The CLI's own ID of the chat it holds, when the terminal chose it (Claude) or found it
    /// (Codex), to read the chat's title from the CLI's session record.
    native: Option<String>,
    title_located: bool,
    title_searched: Option<Instant>,
    started: SystemTime,
}

impl Running {
    fn describe(&self) -> Value {
        json!({"id": self.id, "provider": self.provider, "cwd": self.cwd.display().to_string(),
            "exit": self.session.exit(), "active": self.session.wrote_within(ACTIVE),
            "workspaces": self.workspaces, "project_key": crate::projects::OTHER,
            "title": null, "title_custom": false, "display_title": null})
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
        ("GET", "/app/terminal/output") => {
            terminals.output(id, number(query, "from"), param(query, "view"))
        }
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
            response::action(method, action, || terminals.restart(id, &Attach::of(query)))
        }
        (_, "/app/terminal/resume") => {
            let workspace = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("workspace="))
                .unwrap_or_default()
                .to_owned();
            response::action(method, action, || {
                terminals.resume(&workspace, &Attach::of(query))
            })
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

fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

fn number(query: &str, name: &str) -> Option<u64> {
    param(query, name).and_then(|value| value.parse().ok())
}

/// How a page asks to continue a chat: in which theme, how large a screen, and whether it has
/// confirmed the chat may be open elsewhere.
struct Attach {
    light: bool,
    confirm: bool,
    size: (u16, u16),
}

impl Attach {
    fn of(query: &str) -> Self {
        Self {
            light: number(query, "light") == Some(1),
            confirm: number(query, "confirm") == Some(1),
            size: launch_size(number(query, "cols"), number(query, "rows")),
        }
    }
}

/// What `launch` starts: the CLI, where, at what size, and the chat it continues.
struct Start<'a> {
    provider: &'a str,
    cwd: PathBuf,
    size: (u16, u16),
    /// The CLI's own ID of the chat to continue.
    resume: Option<&'a str>,
    workspaces: Vec<String>,
    light: bool,
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

    /// The id and CLI pid of every terminal whose CLI still runs.
    fn live_pids(&self) -> Vec<(u64, u32)> {
        let state = self.lock();
        state
            .open
            .iter()
            .filter(|running| running.session.exit().is_none())
            .filter_map(|running| Some((running.id, running.session.pid()?)))
            .collect()
    }

    /// What each terminal's chat is called in its CLI, read from the CLI's session record. A
    /// session record not yet found is looked for at most once every `TITLE_LOOKUP_TTL`.
    fn titles(&self) -> HashMap<u64, CliTitle> {
        self.titles_with(TITLE_LOOKUP_TTL, crate::data::boss_app::terminal_title)
    }

    fn titles_with(
        &self,
        ttl: Duration,
        mut lookup: impl FnMut(
            &str,
            Option<&str>,
            &Path,
            SystemTime,
        ) -> (Option<String>, bool, Option<CliTitle>),
    ) -> HashMap<u64, CliTitle> {
        let asks: Vec<_> = {
            let state = self.lock();
            state
                .open
                .iter()
                .filter(|running| {
                    running.title_located
                        || running.title_searched.is_none_or(|at| at.elapsed() >= ttl)
                })
                .map(|running| {
                    let ask = (running.provider.clone(), running.cwd.clone());
                    (running.id, ask, running.native.clone(), running.started)
                })
                .collect()
        };
        let (mut titles, mut updates) = (HashMap::new(), Vec::new());
        for (id, (provider, cwd), native, started) in asks {
            let checked = Instant::now();
            let (found, located, title) = lookup(&provider, native.as_deref(), &cwd, started);
            updates.push((
                id,
                native.is_none().then_some(found).flatten(),
                located,
                checked,
            ));
            titles.extend(title.map(|title| (id, title)));
        }
        if !updates.is_empty() {
            let mut state = self.lock();
            for (id, found, located, checked) in updates {
                if let Some(running) = state.open.iter_mut().find(|running| running.id == id) {
                    running.title_searched = Some(checked);
                    if running.native.is_none() && found.is_some() {
                        running.native = found;
                    }
                    if located {
                        running.title_located = true;
                    }
                }
            }
        }
        titles
    }

    /// Updates confirmed workspaces from active relays under `root` (throttled across polls) and
    /// snapshots every open terminal.
    fn snapshot(&self, root: &Path) -> (Option<PathBuf>, Vec<(u64, PathBuf, Value)>) {
        self.snapshot_with(root, LINKS_TTL)
    }

    fn snapshot_with(
        &self,
        root: &Path,
        ttl: Duration,
    ) -> (Option<PathBuf>, Vec<(u64, PathBuf, Value)>) {
        let (fresh, pids): (bool, Vec<(u64, u32)>) = {
            let state = self.lock();
            let fresh = state
                .links_checked
                .as_ref()
                .is_some_and(|(at, path)| path == root && at.elapsed() < ttl);
            let pids = state
                .open
                .iter()
                .filter(|running| running.session.exit().is_none())
                .filter_map(|running| Some((running.id, running.session.pid()?)))
                .collect();
            (fresh, pids)
        };
        let mut links = if !fresh && !pids.is_empty() {
            links::workspaces(root, &pids)
        } else {
            HashMap::new()
        };
        let mut state = self.lock();
        if !fresh && !pids.is_empty() {
            state.links_checked = Some((Instant::now(), root.to_path_buf()));
        }
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
        let titles = self.titles();
        for (id, _, value) in &mut open {
            if let Some(title) = titles.get(id) {
                value["title"] = json!(title.text);
                value["title_custom"] = json!(title.custom);
                value["display_title"] = json!(title.text);
            }
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
        match crate::video_packs::picker::choose(app, false) {
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
        self.launch(Start {
            provider,
            cwd,
            size: launch_size(payload["cols"].as_u64(), payload["rows"].as_u64()),
            resume: None,
            workspaces: Vec::new(),
            light: payload["light"].as_bool().unwrap_or(false),
        })
    }

    /// Starts an ended terminal's CLI again in its folder, in its place: continuing the chat it
    /// was the boss of when its CLI can, else afresh as a new agent.
    fn restart(&self, id: Option<u64>, attach: &Attach) -> Response {
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
            .map(|workspace| chat_session(workspace, &provider))
            .transpose();
        let resume = match resume {
            Ok(resume) => resume,
            Err(error) => return Response::error(409, error),
        };
        if let Some(chat) = &resume
            && let Some(refusal) = self.unless_elsewhere(&chat.key, attach)
        {
            return refusal;
        }
        // Continuing keeps the workspaces it was the boss of; a fresh start is a new agent.
        let kept = if resume.is_some() {
            workspaces
        } else {
            Vec::new()
        };
        let reply = self.launch(Start {
            provider: &provider,
            cwd,
            size: attach.size,
            resume: resume.as_ref().map(|chat| chat.id.as_str()),
            workspaces: kept,
            light: attach.light,
        });
        if reply.status == 200 {
            self.lock().open.retain(|running| Some(running.id) != id);
        }
        reply
    }

    /// Continues the chat of a workspace that has no terminal here, in the folder it ran in. A
    /// chat already open in a terminal here is not started twice, and one open elsewhere is
    /// continued only once the page has confirmed it.
    fn resume(&self, workspace: &str, attach: &Attach) -> Response {
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
        let chat = match chat_session(workspace, &host) {
            Ok(found) => found,
            Err(error) => return Response::error(409, error),
        };
        let Some(cwd) = crate::data::boss_app::session_cwd_of(&host, &chat.record, &chat.id) else {
            return Response::error(409, "会话记录里没有它的文件夹");
        };
        if let Some(refusal) = self.unless_elsewhere(&chat.key, attach) {
            return refusal;
        }
        let reply = self.launch(Start {
            provider: &host,
            cwd,
            size: attach.size,
            resume: Some(&chat.id),
            workspaces: vec![workspace.to_owned()],
            light: attach.light,
        });
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

    /// The conflict answer when the chat of boss `key` is open outside this App's terminals and
    /// the page has not confirmed continuing it anyway.
    fn unless_elsewhere(&self, key: &str, attach: &Attach) -> Option<Response> {
        if attach.confirm {
            return None;
        }
        let root = crate::data_files::data_root();
        let found = elsewhere::find(&root, key, &self.live_pids())?;
        let place = found.app.as_deref().unwrap_or("别的程序");
        let state = if found.paused {
            "已暂停"
        } else {
            "运行中"
        };
        let mut reply = Response::json(&json!({
            "error": format!("这段聊天正在{place}里打开（{state}）"),
            "elsewhere": {"app": found.app, "paused": found.paused},
        }));
        reply.status = 409;
        Some(reply)
    }

    fn launch(&self, start: Start) -> Response {
        let Start {
            provider,
            cwd,
            size: (cols, rows),
            resume,
            workspaces,
            light,
        } = start;
        let Some(program) = installed_path(provider) else {
            return Response::error(400, "这个 CLI 没有安装");
        };
        if !has_terminal_ui(provider, &program) {
            return Response::error(400, "这个 CLI 没有终端界面");
        }
        if !cwd.is_absolute() || !cwd.is_dir() {
            return Response::error(400, "文件夹不存在");
        }
        // Claude and Grok take the ID of a new chat up front, which is how their session record
        // is found before the chat has a workspace; it names nothing else.
        let chosen = (matches!(provider, "claude" | "grok") && resume.is_none())
            .then(session::fresh_session_id);
        let theme = theme::for_cli(provider, light);
        let launch = Launch {
            program,
            args: [
                session::cli_args(provider, resume, chosen.as_deref()),
                theme.args,
            ]
            .concat(),
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
        state.links_checked = None;
        let running = Running {
            id: state.started,
            provider: provider.to_owned(),
            cwd,
            session,
            workspaces,
            native: resume.map(str::to_owned).or(chosen),
            title_located: false,
            title_searched: None,
            started: SystemTime::now(),
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
    fn output(&self, id: Option<u64>, from: Option<u64>, view: Option<&str>) -> Response {
        let Some(session) = self.session(id) else {
            return Response::error(404, "找不到这个终端");
        };
        let chunk = session.read_view(from.unwrap_or(0), POLL, view);
        let mut reply = Response::ok("application/octet-stream", chunk.bytes);
        reply.headers = vec![
            ("X-Terminal-Start", chunk.start.to_string()),
            ("X-Terminal-End", chunk.end.to_string()),
            (
                "X-Terminal-Exit",
                chunk.exit.map(|code| code.to_string()).unwrap_or_default(),
            ),
            (
                "X-Terminal-Appearance",
                u8::from(chunk.appearance).to_string(),
            ),
        ];
        if chunk.replay {
            reply.headers.push(("X-Terminal-Replay", "1".to_owned()));
        }
        reply
    }
}

/// A boss chat that can be continued: its CLI's own ID, its session record and its boss key.
struct Chat {
    id: String,
    record: PathBuf,
    key: String,
}

/// The native ID and session record of a workspace's boss chat, when its CLI is `provider` and
/// can continue it.
fn chat_session(workspace: &str, provider: &str) -> Result<Chat, &'static str> {
    if !session::resumes(provider) {
        return Err("这个 CLI 还不能在 App 里续接");
    }
    let (host, key, file) =
        links::chat(&crate::data_files::data_root(), workspace).ok_or("找不到这段聊天的记录")?;
    if host != provider {
        return Err("这段聊天不是这个 CLI 的");
    }
    let (id, record) = crate::data::boss_app::native_session(&host, &key, file.as_deref())
        .ok_or("找不到这段聊天的会话记录")?;
    Ok(Chat { id, record, key })
}

/// The size a CLI starts at: what the page measured, or 100 by 30 when it did not say, kept to
/// a screen a terminal can sensibly have.
fn launch_size(cols: Option<u64>, rows: Option<u64>) -> (u16, u16) {
    let within = |value: Option<u64>, default: u64, least: u64, most: u64| {
        u16::try_from(value.unwrap_or(default).clamp(least, most)).unwrap_or(u16::MAX)
    };
    (within(cols, 100, 20, 500), within(rows, 30, 5, 200))
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
/// Whether the installed CLI has an interactive terminal UI to run. DeepSeek Harness ships only
/// its ACP, headless, SDK and web profiles (and the Desktop app's own). ZCode's Desktop app bundles
/// its CLI without the TUI package, so only a separately installed `zcode` command opens one.
fn has_terminal_ui(provider: &str, program: &Path) -> bool {
    match provider {
        "deepseek" => false,
        "zcode" => !program.ends_with("Contents/Resources/glm/zcode.cjs"),
        _ => true,
    }
}

fn installed() -> Value {
    let detected = crate::detect::detected(false, crate::members::codex_source);
    let clis: Vec<Value> = PROVIDERS
        .iter()
        .filter(|provider| detected[**provider]["installed"] == true)
        .filter(|provider| {
            detected[**provider]["path"]
                .as_str()
                .is_some_and(|path| has_terminal_ui(provider, Path::new(path)))
        })
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

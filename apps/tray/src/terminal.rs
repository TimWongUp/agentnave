//! The embedded terminal (ADR-0103): installed CLIs the user starts from the office, each in a
//! project's folder. They are the user's own processes and end with the App; a CLI that works
//! through AgentNave is the boss of the workspaces it dispatched from, which the office shows it
//! beside.
//!
//! Pages read output by long polling and send keys, size changes, stop and close as action POSTs,
//! all over the `agentnave` scheme like every other page. Every route but the list names its
//! terminal with `?session=<id>`.

mod account;
#[cfg(unix)]
mod codex_server;
mod derived;
mod elsewhere;
mod input_gate;
mod links;
mod modes;
mod session;
mod spawn_requests;
mod status;
#[cfg(test)]
mod tests;
mod theme;
mod wakeup;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::data::boss_app::CliTitle;
use crate::providers::PROVIDERS;
use crate::response::{self, Response};
use account::{Active, Login};
pub(crate) use session::parent_session;
use session::{Launch, Session};

/// How long an output request waits for new output before answering empty.
const POLL: Duration = Duration::from_secs(20);
/// A terminal that wrote within this long counts as working.
const ACTIVE: Duration = Duration::from_millis(2500);
/// Minimum interval between directory scans for a terminal whose session record is not yet located.
const TITLE_LOOKUP_TTL: Duration = Duration::from_secs(5);
/// Minimum interval between full connection-file + process-table scans on `/app/terminal/state`.
const LINKS_TTL: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
pub(crate) struct Terminals {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    open: Vec<Running>,
    /// Sign-in terminals, kept apart so nothing that lists or wakes the open ones sees them.
    logins: Vec<Login>,
    /// The folder last picked or used, offered for the next start.
    cwd: Option<PathBuf>,
    started: u64,
    links_checked: Option<(Instant, PathBuf)>,
}

pub(super) struct DerivedInfo {
    pub parent_boss_key: String,
    pub parent_workspace_id: Option<String>,
    pub busy: bool,
    pub blocked_kind: Option<String>,
    pub isolation_id: Option<String>,
    pub _lease: Option<crate::isolated::leases::HeldLease>,
    pub _log_dir: Option<tempfile::TempDir>,
    pub codex_ready_file: Option<PathBuf>,
    pub active_invocation_id: Option<String>,
}

impl Drop for DerivedInfo {
    fn drop(&mut self) {
        if let Some(path) = self.codex_ready_file.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[derive(Clone)]
pub(super) struct DerivedHandle {
    pub(in crate::terminal) id: u64,
    pub(in crate::terminal) provider: String,
    pub(in crate::terminal) session: Arc<Session>,
    pub(in crate::terminal) native: Option<String>,
    pub(in crate::terminal) account: Option<Active>,
    #[cfg(unix)]
    pub(in crate::terminal) codex: Option<Arc<codex_server::Client>>,
}

pub(super) struct NewDerived {
    pub(in crate::terminal) provider: String,
    pub(in crate::terminal) cwd: PathBuf,
    pub(in crate::terminal) session: Arc<Session>,
    pub(in crate::terminal) native: Option<String>,
    pub(in crate::terminal) account: Option<Active>,
    #[cfg(unix)]
    pub(in crate::terminal) codex: Option<Arc<codex_server::Client>>,
    pub(in crate::terminal) info: DerivedInfo,
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
    pending: usize,
    /// The named account it runs as; None for the default account.
    account: Option<Active>,
    #[cfg(unix)]
    codex: Option<Arc<codex_server::Client>>,
    derived: Option<DerivedInfo>,
}

impl Running {
    fn to_derived_handle(&self) -> DerivedHandle {
        DerivedHandle {
            id: self.id,
            provider: self.provider.clone(),
            session: self.session.clone(),
            native: self.native.clone(),
            account: self.account.clone(),
            #[cfg(unix)]
            codex: self.codex.clone(),
        }
    }

    fn describe(&self, root: &Path) -> Value {
        // Dresses the boss figure of a terminal that has dispatched nothing yet; never a name.
        let art_model = crate::detect::default_model(
            &self.provider,
            self.account.as_ref().map(|account| account.dir.as_path()),
            root,
        );
        let is_derived = self.derived.is_some();
        let busy = self.derived.as_ref().is_some_and(|d| d.busy);
        let active = if is_derived {
            busy && self.session.exit().is_none()
        } else {
            self.session.wrote_within(ACTIVE)
        };
        json!({"id": self.id, "provider": self.provider, "art_model": art_model, "cwd": self.cwd.display().to_string(),
            "exit": self.session.exit(), "active": active,
            "workspaces": self.workspaces, "project_key": crate::projects::OTHER,
            "account": self.account.as_ref().map(|account| &account.id),
            "account_label": self.account.as_ref().map(|account| &account.label),
            "title": null, "title_custom": false, "display_title": null, "pending_results": self.pending,
            "derived": is_derived,
            "parent_boss_key": self.derived.as_ref().map(|d| &d.parent_boss_key),
            "parent_workspace_id": self.derived.as_ref().and_then(|d| d.parent_workspace_id.as_deref()),
            "derived_busy": busy,
            "native": self.native,
            "blocked_kind": self.derived.as_ref().and_then(|d| d.blocked_kind.as_deref())})
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
        (_, "/app/terminal/handover") => {
            response::post(method, action, body, |payload| terminals.handover(&payload))
        }
        (_, "/app/terminal/input") => response::action(method, action, || {
            terminals.with_session(id, |session| {
                if param(query, "source") == Some("auto") {
                    session.write(body)
                } else {
                    session.user_input(body, param(query, "source") == Some("paste"))
                }
            })
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

pub(crate) fn start_wakeup(app: AppHandle) {
    wakeup::start(app);
}

/// Ends every CLI when the App quits.
pub(crate) fn shutdown(app: &AppHandle) {
    wakeup::shutdown();
    let terminals = app.state::<Terminals>();
    let drained: Vec<Running> = {
        let mut state = terminals.lock();
        std::mem::take(&mut state.open)
    };
    #[cfg(unix)]
    for codex in drained.iter().filter_map(|r| r.codex.as_ref()) {
        codex.close();
    }
    for running in &drained {
        if let Some(inv_id) = running
            .derived
            .as_ref()
            .and_then(|d| d.active_invocation_id.as_deref())
        {
            wakeup::on_terminal_closed(inv_id);
        }
        running.session.stop();
    }
    drop(drained);
    let logins: Vec<_> = {
        let mut state = terminals.lock();
        std::mem::take(&mut state.logins)
    };
    for login in logins {
        login.session.stop();
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
    /// The named account to run as; None for the default account.
    account: Option<Active>,
}

impl Terminals {
    pub(super) fn reserve_id(&self) -> u64 {
        let mut state = self.lock();
        state.started += 1;
        state.started
    }

    pub(super) fn register_derived(&self, spec: NewDerived) -> u64 {
        let id = self.reserve_id();
        self.insert_reserved_derived(id, spec);
        id
    }

    pub(super) fn insert_reserved_derived(&self, id: u64, spec: NewDerived) {
        let mut state = self.lock();
        state.cwd = Some(spec.cwd.clone());
        state.open.push(Running {
            id,
            provider: spec.provider,
            cwd: spec.cwd,
            session: spec.session,
            workspaces: Vec::new(),
            native: spec.native,
            title_located: false,
            title_searched: None,
            started: SystemTime::now(),
            pending: 0,
            account: spec.account,
            #[cfg(unix)]
            codex: spec.codex,
            derived: Some(spec.info),
        });
    }

    pub(super) fn find_derived_session(
        &self,
        provider: &str,
        native_id: &str,
    ) -> Option<DerivedHandle> {
        let state = self.lock();
        state
            .open
            .iter()
            .find(|r| {
                r.derived.is_some()
                    && r.provider == provider
                    && r.native.as_deref() == Some(native_id)
                    && r.session.exit().is_none()
            })
            .map(Running::to_derived_handle)
    }

    pub(super) fn derived_by_invocation(&self, invocation_id: &str) -> Option<DerivedHandle> {
        let state = self.lock();
        state
            .open
            .iter()
            .find(|r| {
                r.derived
                    .as_ref()
                    .is_some_and(|d| d.active_invocation_id.as_deref() == Some(invocation_id))
            })
            .map(Running::to_derived_handle)
    }

    pub(super) fn has_lease_for_isolation(&self, isolation_id: &str) -> bool {
        let state = self.lock();
        state.open.iter().any(|r| {
            r.session.exit().is_none()
                && r.derived.as_ref().is_some_and(|d| {
                    d.isolation_id.as_deref() == Some(isolation_id) && d._lease.is_some()
                })
        })
    }

    pub(super) fn remove_exited_derived(&self, provider: &str, native_id: &str) {
        let removed: Vec<Running> = {
            let mut state = self.lock();
            let mut gone = Vec::new();
            let mut kept = Vec::with_capacity(state.open.len());
            for r in state.open.drain(..) {
                if r.derived.is_some()
                    && r.provider == provider
                    && r.native.as_deref() == Some(native_id)
                    && r.session.exit().is_some()
                {
                    gone.push(r);
                } else {
                    kept.push(r);
                }
            }
            state.open = kept;
            gone
        };
        drop(removed);
    }

    pub(super) fn try_claim_derived_turn(
        &self,
        terminal_id: u64,
        invocation_id: &str,
        expected_account: Option<&str>,
    ) -> Result<DerivedHandle, String> {
        let mut state = self.lock();
        let Some(running) = state.open.iter_mut().find(|r| r.id == terminal_id) else {
            return Err("派生终端已关闭".to_owned());
        };
        if running.session.exit().is_some() {
            return Err("派生终端已退出".to_owned());
        }
        if running.account.as_ref().map(|a| a.id.as_str()) != expected_account {
            return Err("派生会话原账号与请求账号不一致".to_owned());
        }
        let Some(derived) = running.derived.as_mut() else {
            return Err("目标不是派生终端".to_owned());
        };
        if derived.busy || derived.active_invocation_id.is_some() {
            return Err("派生终端正忙，无法同时续派".to_owned());
        }
        derived.busy = true;
        derived.blocked_kind = None;
        derived.active_invocation_id = Some(invocation_id.to_owned());
        Ok(running.to_derived_handle())
    }

    pub(super) fn set_derived_blocker(
        &self,
        terminal_id: u64,
        native: Option<&str>,
        blocked_kind: Option<&str>,
    ) {
        let mut state = self.lock();
        let Some(running) = state.open.iter_mut().find(|r| r.id == terminal_id) else {
            return;
        };
        if let Some(sid) = native
            && running.native.as_deref() != Some(sid)
        {
            running.native = Some(sid.to_owned());
            running.title_located = false;
            running.title_searched = None;
        }
        if let Some(derived) = running.derived.as_mut() {
            derived.busy = true;
            derived.blocked_kind = blocked_kind.map(str::to_owned);
        }
    }

    pub(super) fn clear_derived_turn(&self, terminal_id: u64, native: Option<&str>) {
        let mut state = self.lock();
        let Some(running) = state.open.iter_mut().find(|r| r.id == terminal_id) else {
            return;
        };
        if let Some(sid) = native
            && running.native.as_deref() != Some(sid)
        {
            running.native = Some(sid.to_owned());
            running.title_located = false;
            running.title_searched = None;
        }
        if let Some(derived) = running.derived.as_mut() {
            derived.busy = false;
            derived.blocked_kind = None;
            derived.active_invocation_id = None;
        }
    }

    pub(super) fn attach_derived_lease(
        &self,
        terminal_id: u64,
        isolation_id: Option<&str>,
        lease: crate::isolated::leases::HeldLease,
    ) {
        let mut state = self.lock();
        if let Some(running) = state.open.iter_mut().find(|r| r.id == terminal_id)
            && let Some(derived) = running.derived.as_mut()
            && derived._lease.is_none()
        {
            if derived.isolation_id.is_none() {
                derived.isolation_id = isolation_id.map(str::to_owned);
            }
            derived._lease = Some(lease);
        }
    }

    pub(super) fn release_derived_lease(&self, terminal_id: u64) {
        let lease = {
            let mut state = self.lock();
            state
                .open
                .iter_mut()
                .find(|r| r.id == terminal_id)
                .and_then(|r| r.derived.as_mut())
                .and_then(|d| d._lease.take())
        };
        drop(lease);
    }

    pub(super) fn remove_derived_terminal(&self, terminal_id: u64) {
        let removed = {
            let mut state = self.lock();
            state
                .open
                .iter()
                .position(|r| r.id == terminal_id && r.derived.is_some())
                .map(|idx| state.open.remove(idx))
        };
        if let Some(running) = removed {
            #[cfg(unix)]
            if let Some(codex) = running.codex.as_ref() {
                codex.close();
            }
            running.session.stop();
            drop(running);
        }
    }

    pub(super) fn replace_derived_session(
        &self,
        terminal_id: u64,
        session: Arc<Session>,
        native: Option<&str>,
    ) {
        let mut state = self.lock();
        if let Some(running) = state.open.iter_mut().find(|r| r.id == terminal_id) {
            running.session = session;
            if let Some(sid) = native {
                running.native = Some(sid.to_owned());
                running.title_located = false;
                running.title_searched = None;
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn take_codex_ready_file(&self, terminal_id: u64) -> Option<PathBuf> {
        let mut state = self.lock();
        let running = state.open.iter_mut().find(|r| r.id == terminal_id)?;
        running.derived.as_mut()?.codex_ready_file.take()
    }

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

    fn login(
        &self,
        account: &str,
        dir: Option<&Path>,
        program: &Path,
        home: &Path,
        (cols, rows): (u16, u16),
    ) -> Result<u64, String> {
        if !home.is_dir() || dir.is_some_and(|dir| !dir.is_dir()) {
            return Err("账号目录不存在".into());
        }
        let args = vec!["auth".to_owned(), "login".to_owned()];
        let (program, args, env) = match dir {
            Some(dir) => {
                let active = Active {
                    id: account.to_owned(),
                    label: String::new(),
                    dir: dir.to_owned(),
                };
                account::launch_as(&active, program.to_owned(), args, false)
            }
            None => (program.to_owned(), args, Vec::new()),
        };
        let session = Session::spawn(&Launch {
            program,
            args,
            env,
            cwd: home.to_owned(),
            cols,
            rows,
            derived: false,
        })?;
        let mut state = self.lock();
        // One sign-in per account: an earlier terminal of it gives way.
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.logins)
            .into_iter()
            .partition(|login| login.account == account);
        state.logins = kept;
        state.started += 1;
        let id = state.started;
        state.logins.push(Login {
            id,
            account: account.to_owned(),
            session,
        });
        drop(state);
        for login in gone {
            login.session.stop();
        }
        Ok(id)
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
            &HashSet<String>,
            Option<&Path>,
        ) -> (Option<String>, bool, Option<CliTitle>),
    ) -> HashMap<u64, CliTitle> {
        let (asks, mut claimed): (Vec<_>, HashMap<String, HashSet<String>>) = {
            let state = self.lock();
            let mut claimed: HashMap<String, HashSet<String>> = HashMap::new();
            for running in &state.open {
                if let Some(id) = &running.native {
                    claimed
                        .entry(running.provider.clone())
                        .or_default()
                        .insert(id.clone());
                }
            }
            let asks = state
                .open
                .iter()
                .filter(|running| {
                    running.title_located
                        || running.title_searched.is_none_or(|at| at.elapsed() >= ttl)
                })
                .map(|running| {
                    let ask = (running.provider.clone(), running.cwd.clone());
                    let home = running.account.as_ref().map(|account| account.dir.clone());
                    (
                        running.id,
                        ask,
                        running.native.clone(),
                        running.started,
                        home,
                    )
                })
                .collect();
            (asks, claimed)
        };
        let empty = HashSet::new();
        let (mut titles, mut updates) = (HashMap::new(), Vec::new());
        for (id, (provider, cwd), native, started, home) in asks {
            let checked = Instant::now();
            let exclude = claimed.get(&provider).unwrap_or(&empty);
            let (found, located, title) = lookup(
                &provider,
                native.as_deref(),
                &cwd,
                started,
                exclude,
                home.as_deref(),
            );
            if native.is_none()
                && let Some(found_id) = &found
            {
                claimed
                    .entry(provider)
                    .or_default()
                    .insert(found_id.clone());
            }
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
                .filter(|running| running.session.exit().is_none() && running.derived.is_none())
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
            .map(|running| (running.id, running.cwd.clone(), running.describe(root)))
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
        let account =
            match account::requested(payload).and_then(|id| account::resolve(provider, id)) {
                Ok(account) => account,
                Err(error) => return Response::error(400, &error),
            };
        let cwd = PathBuf::from(payload["cwd"].as_str().unwrap_or_default());
        self.launch(Start {
            provider,
            cwd,
            size: launch_size(payload["cols"].as_u64(), payload["rows"].as_u64()),
            resume: None,
            workspaces: Vec::new(),
            light: payload["light"].as_bool().unwrap_or(false),
            account,
        })
    }

    fn handover(&self, payload: &Value) -> Response {
        if crate::PREVIEW {
            return Response::error(403, "预览版不能转交工作区");
        }
        self.handover_in(
            &crate::data_files::data_root(),
            payload,
            |provider| {
                installed_path(provider).is_some_and(|path| has_terminal_ui(provider, &path))
            },
            |start, args, env| self.launch_with_extra(start, args, env),
        )
    }

    fn handover_in(
        &self,
        root: &Path,
        payload: &Value,
        available: impl FnOnce(&str) -> bool,
        launch: impl FnOnce(Start<'_>, &[String], &[(&str, &str)]) -> Response,
    ) -> Response {
        let Some(workspace) = payload["workspace"]
            .as_str()
            .filter(|id| crate::data_files::valid_id(id))
        else {
            return Response::error(400, "需要有效的工作区 ID");
        };
        let provider = payload["provider"].as_str().unwrap_or_default();
        if !matches!(provider, "claude" | "codex") {
            return Response::error(400, "这个 CLI 暂不支持自动接手");
        }
        let account = match account::requested(payload)
            .and_then(|id| account::resolve_in(root, provider, id))
        {
            Ok(account) => account,
            Err(error) => return Response::error(400, &error),
        };
        let Some((host, key, file)) = links::chat(root, workspace) else {
            return Response::error(404, "找不到这段聊天的记录");
        };
        let owner = links::owner_account(root, workspace);
        let source_account = match account::resolve_in(root, &host, owner.as_deref()) {
            Ok(account) => account,
            Err(error) => {
                return Response::error(
                    409,
                    account::chat_refusal(&error, "这段聊天的账号已被移除"),
                );
            }
        };
        let home = source_account.as_ref().map(|account| account.dir.as_path());
        let Some((id, record)) =
            crate::data::boss_app::desktop_session(&host, &key, file.as_deref(), home)
        else {
            return Response::error(409, "找不到桌面端聊天的会话记录");
        };
        let Some(cwd) = crate::data::boss_app::session_cwd_of(&host, &record, &id)
            .filter(|path| path.is_absolute() && path.is_dir())
        else {
            return Response::error(409, "会话的文件夹不存在或无法确定");
        };
        if !available(provider) {
            return Response::error(400, "这个 CLI 没有安装或没有终端界面");
        }
        let marked = crate::manage::handover(root, workspace);
        if marked["status"] != "waiting" {
            return Response::error(409, marked["message"].as_str().unwrap_or("转交失败"));
        }
        let prompt = format!("接手工作区 {workspace}");
        let args = if provider == "claude" || !cfg!(unix) {
            vec![prompt.clone()]
        } else {
            Vec::new()
        };
        let env = if provider == "codex" && cfg!(unix) {
            vec![("AGENTNAVE_HANDOVER_PROMPT", prompt.as_str())]
        } else {
            Vec::new()
        };
        let reply = launch(
            Start {
                provider,
                cwd,
                size: launch_size(payload["cols"].as_u64(), payload["rows"].as_u64()),
                resume: None,
                workspaces: Vec::new(),
                light: payload["light"].as_bool().unwrap_or(false),
                account,
            },
            &args,
            &env,
        );
        if reply.status != 200 {
            let error: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            let detail = error["error"].as_str().unwrap_or("启动终端失败");
            return Response::error(reply.status, &format!("工作区已标为待接手，但{detail}"));
        }
        reply
    }

    /// Starts an ended terminal's CLI again in its folder, in its place: continuing the chat it
    /// was the boss of when its CLI can, else afresh as a new agent.
    fn restart(&self, id: Option<u64>, attach: &Attach) -> Response {
        let (provider, cwd, workspaces, account) = {
            let state = self.lock();
            let Some(running) = state.open.iter().find(|running| Some(running.id) == id) else {
                return Response::error(404, "找不到这个终端");
            };
            if running.derived.is_some() {
                return Response::error(409, "派生终端不能从侧栏重启");
            }
            if running.session.exit().is_none() {
                return Response::error(409, "终端还在运行");
            }
            (
                running.provider.clone(),
                running.cwd.clone(),
                running.workspaces.clone(),
                running.account.clone(),
            )
        };
        let resume = workspaces
            .last()
            .filter(|_| session::resumes(&provider))
            .map(|workspace| chat_session(workspace, &provider, account.as_ref()))
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
            account,
        });
        if reply.status == 200 {
            let mut state = self.lock();
            let mut removed = Vec::new();
            let mut kept = Vec::with_capacity(state.open.len());
            for running in state.open.drain(..) {
                if Some(running.id) == id {
                    removed.push(running);
                } else {
                    kept.push(running);
                }
            }
            state.open = kept;
            drop(state);
            drop(removed);
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
                    && running.derived.is_none()
                    && running.workspaces.last().map(String::as_str) == Some(workspace)
            });
            if let Some(running) = open {
                return Response::json(
                    &json!({"session": running.describe(&crate::data_files::data_root())}),
                );
            }
        }
        let Some((host, _, _)) = links::chat(&crate::data_files::data_root(), workspace) else {
            return Response::error(404, "找不到这段聊天的记录");
        };
        // A chat goes on as the account that began it; a removed account cannot be stood in for.
        let owner = links::owner_account(&crate::data_files::data_root(), workspace);
        let account = match account::resolve(&host, owner.as_deref()) {
            Ok(account) => account,
            Err(error) => {
                let refusal = account::chat_refusal(&error, "这段聊天的账号已被移除，无法续接");
                return Response::error(409, refusal);
            }
        };
        let chat = match chat_session(workspace, &host, account.as_ref()) {
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
            account,
        });
        if reply.status == 200 {
            // An ended terminal of this chat gives way to the new one.
            let mut state = self.lock();
            let latest = state.started;
            let mut removed = Vec::new();
            let mut kept = Vec::with_capacity(state.open.len());
            for running in state.open.drain(..) {
                if running.id == latest
                    || running.workspaces.last().map(String::as_str) != Some(workspace)
                {
                    kept.push(running);
                } else {
                    removed.push(running);
                }
            }
            state.open = kept;
            drop(state);
            drop(removed);
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
        self.launch_with_extra(start, &[], &[])
    }

    fn launch_with_extra(
        &self,
        start: Start,
        extra_args: &[String],
        extra_env: &[(&str, &str)],
    ) -> Response {
        let Start {
            provider,
            cwd,
            size: (cols, rows),
            resume,
            workspaces,
            light,
            account,
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
        let id = {
            let mut state = self.lock();
            state.started += 1;
            state.started
        };
        // Claude and Grok take the ID of a new chat up front, which is how their session record
        // is found before the chat has a workspace; it names nothing else.
        let chosen = (matches!(provider, "claude" | "grok") && resume.is_none())
            .then(session::fresh_session_id);
        let mut theme = theme::for_cli(provider, light);
        match provider {
            "antigravity" => theme.env.push(("AGY_CLI_OSC7501", "on")),
            "pi" => theme.env.push(("PI_PROGRAM_STATUS", "1")),
            _ => {}
        }
        #[cfg(unix)]
        let codex_sock = (provider == "codex").then(|| codex_server::socket_path(id));
        #[cfg(unix)]
        let (launch_program, launch_args) = if let Some(sock) = &codex_sock {
            let (prog, args) = codex_server::launch_args(&program, sock, &cwd, resume, extra_args);
            (prog, [args, theme.args].concat())
        } else {
            let mut args = session::cli_args(provider, resume, chosen.as_deref());
            args.extend(extra_args.iter().cloned());
            args.extend(theme.args);
            (program, args)
        };
        #[cfg(not(unix))]
        let (launch_program, launch_args) = {
            let mut args = session::cli_args(provider, resume, chosen.as_deref());
            args.extend(extra_args.iter().cloned());
            args.extend(theme.args);
            (program, args)
        };
        let mut env: Vec<(String, String)> = theme
            .env
            .into_iter()
            .chain(extra_env.iter().copied())
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let (launch_program, launch_args) = match &account {
            Some(account) => {
                let (program, args, more) =
                    account::launch_as(account, launch_program, launch_args, false);
                env.extend(more);
                (program, args)
            }
            None => (launch_program, launch_args),
        };
        let launch = Launch {
            program: launch_program,
            args: launch_args,
            env,
            cwd: cwd.clone(),
            cols,
            rows,
            derived: false,
        };
        let session = match Session::spawn(&launch) {
            Ok(session) => session,
            Err(error) => return Response::error(500, &error),
        };
        #[cfg(unix)]
        let codex = codex_sock
            .map(|sock| codex_server::Client::connect_in_background(sock, session.clone()));
        let mut state = self.lock();
        state.cwd = Some(cwd.clone());
        state.links_checked = None;
        let running = Running {
            id,
            provider: provider.to_owned(),
            cwd,
            session,
            workspaces,
            native: resume.map(str::to_owned).or(chosen),
            title_located: false,
            title_searched: None,
            started: SystemTime::now(),
            pending: 0,
            account,
            #[cfg(unix)]
            codex,
            derived: None,
        };
        let described = running.describe(&crate::data_files::data_root());
        state.open.push(running);
        Response::json(&json!({"session": described}))
    }

    fn session(&self, id: Option<u64>) -> Option<Arc<Session>> {
        let state = self.lock();
        let found = state
            .open
            .iter()
            .find(|running| Some(running.id) == id)
            .map(|running| running.session.clone());
        found.or_else(|| {
            let login = state.logins.iter().find(|login| Some(login.id) == id)?;
            Some(login.session.clone())
        })
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
        let removed = {
            let mut state = self.lock();
            if let Some(at) = state.logins.iter().position(|login| Some(login.id) == id) {
                let login = state.logins.remove(at);
                drop(state);
                login.session.stop();
                return Response::json(&json!({"ok": true}));
            }
            let Some(at) = state.open.iter().position(|running| Some(running.id) == id) else {
                return Response::error(404, "找不到这个终端");
            };
            state.open.remove(at)
        };
        #[cfg(unix)]
        if let Some(codex) = &removed.codex {
            codex.close();
        }
        if let Some(inv_id) = removed
            .derived
            .as_ref()
            .and_then(|d| d.active_invocation_id.as_deref())
        {
            wakeup::on_terminal_closed(inv_id);
        }
        removed.session.stop();
        drop(removed);
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
/// can continue it. A chat of a named account is looked for in that account's folder.
fn chat_session(
    workspace: &str,
    provider: &str,
    account: Option<&Active>,
) -> Result<Chat, &'static str> {
    chat_session_in(
        &crate::data_files::data_root(),
        workspace,
        provider,
        account,
    )
}

fn chat_session_in(
    root: &Path,
    workspace: &str,
    provider: &str,
    account: Option<&Active>,
) -> Result<Chat, &'static str> {
    if !session::resumes(provider) {
        return Err("这个 CLI 还不能在 App 里续接");
    }
    let (host, key, file) = links::chat(root, workspace).ok_or("找不到这段聊天的记录")?;
    if host != provider {
        return Err("这段聊天不是这个 CLI 的");
    }
    let home = account.map(|account| account.dir.as_path());
    let (id, record) = crate::data::boss_app::native_session_as(&host, &key, file.as_deref(), home)
        .ok_or("找不到这段聊天的会话记录")?;
    if crate::data::boss_app::desktop_session(&host, &key, Some(&record), home).is_some() {
        return Err("桌面端聊天不能在这里续接，请转交到终端");
    }
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
    let accounts = crate::accounts::read(&crate::data_files::data_root()).unwrap_or_default();
    let clis: Vec<Value> = PROVIDERS
        .iter()
        .filter(|provider| detected[**provider]["installed"] == true)
        .filter(|provider| {
            detected[**provider]["path"]
                .as_str()
                .is_some_and(|path| has_terminal_ui(provider, Path::new(path)))
        })
        .map(|provider| {
            // Only a CLI with named accounts offers a choice; the default account is no entry.
            let named: Vec<Value> = accounts
                .iter()
                .filter(|_| *provider == crate::accounts::PROVIDER)
                .map(|account| json!({"id": account.id, "label": account.label}))
                .collect();
            json!({"provider": provider, "accounts": named})
        })
        .collect();
    json!({"clis": clis})
}

/// Whether a terminal of this account still runs here: an agent, or its sign-in.
pub(crate) fn account_in_use(app: &AppHandle, id: &str) -> bool {
    let state = app.state::<Terminals>();
    let state = state.lock();
    state.open.iter().any(|running| {
        running.session.exit().is_none()
            && running
                .account
                .as_ref()
                .is_some_and(|account| account.id == id)
    }) || state
        .logins
        .iter()
        .any(|login| login.session.exit().is_none() && login.account == id)
}

/// Starts `claude auth login` for one account (or the default environment when `dir` is None) in
/// a terminal of its own and returns the terminal's id; the page shows it in a dialog and closes
/// it when the sign-in is done.
pub(crate) fn open_login(
    app: &AppHandle,
    account: &str,
    dir: Option<&Path>,
    program: &Path,
    home: &Path,
    size: (Option<u64>, Option<u64>),
) -> Result<u64, String> {
    let (cols, rows) = launch_size(size.0, size.1);
    app.state::<Terminals>()
        .login(account, dir, program, home, (cols, rows))
}

fn installed_path(provider: &str) -> Option<PathBuf> {
    let detected = crate::detect::detected(false, crate::members::codex_source);
    detected[provider]["installed"]
        .as_bool()
        .filter(|installed| *installed)
        .and(detected[provider]["path"].as_str())
        .map(PathBuf::from)
}

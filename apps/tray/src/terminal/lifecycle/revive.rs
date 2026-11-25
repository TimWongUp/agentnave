//! Starts a derived terminal whose process has ended again in its own row: for the user, who opens
//! it and nothing is submitted, or for a re-dispatch by the boss.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use super::super::account::Active;
use super::super::session::Session;
use super::super::{
    Attach, DerivedHandle, DerivedLaunchConfig, NewDerived, Running, State, Terminals, derived,
    normalize_cwd, wakeup,
};
use super::{Life, account_error};
use crate::isolated::leases::{self, HeldLease};
use crate::response::Response;

/// How long a wake or re-dispatch waits for a process on its way out, or for another start.
const PENDING_WAIT: Duration = Duration::from_secs(5);
const PENDING_STEP: Duration = Duration::from_millis(50);
const LEASE_WAIT: Duration = Duration::from_secs(3);
pub(super) const PENDING_WAKE_ERR: &str = "终端正在结束或启动进程，请稍后再试";
pub(in crate::terminal) const RESUME_PENDING_ERR: &str = "派生终端正在结束或启动进程，请稍后续派";

/// The row of a session a boss re-dispatches to.
pub(in crate::terminal) enum ResumeSlot {
    /// No row: a terminal of its own is opened, the session held until it is there.
    Free(Reviving),
    Live(Box<DerivedHandle>),
    /// A row whose process has ended, held for the process about to be started in it.
    Dormant(Reviving),
}

/// A session held for one new process, so no second one starts on it meanwhile: its row, or the
/// session alone while it has none. Let go of when dropped.
pub(in crate::terminal) struct Reviving {
    terminals: Terminals,
    held: Held,
}

enum Held {
    Row(u64),
    Session(String, String),
}

impl Reviving {
    fn row(terminals: &Terminals, id: u64) -> Self {
        Self {
            terminals: terminals.clone(),
            held: Held::Row(id),
        }
    }

    /// Whether the row held was closed meanwhile.
    pub(in crate::terminal) fn is_gone(&self) -> bool {
        match &self.held {
            Held::Row(id) => !self.terminals.lock().open.iter().any(|r| r.id == *id),
            Held::Session(..) => false,
        }
    }

    /// Holds a session that has no row, unless something else already does.
    pub(super) fn session(
        terminals: &Terminals,
        state: &mut State,
        provider: &str,
        native: &str,
    ) -> Option<Self> {
        let key = (provider.to_owned(), native.to_owned());
        state.opening.insert(key.clone()).then(|| Self {
            terminals: terminals.clone(),
            held: Held::Session(key.0, key.1),
        })
    }
}

impl Drop for Reviving {
    fn drop(&mut self) {
        let mut state = self.terminals.lock();
        match &self.held {
            Held::Row(id) => {
                if let Some(running) = state.open.iter_mut().find(|r| r.id == *id)
                    && running.session.exit().is_some()
                    && let Some(derived) = running.derived.as_mut()
                {
                    derived.life.reviving = false;
                }
            }
            Held::Session(provider, native) => {
                state.opening.remove(&(provider.clone(), native.clone()));
            }
        }
    }
}

/// What a session whose process has ended is started again with.
#[derive(Clone)]
pub(in crate::terminal) struct WakePlan {
    pub provider: String,
    pub cwd: PathBuf,
    pub native: String,
    pub account: Option<Active>,
    pub config: Option<DerivedLaunchConfig>,
    /// The permission mode Claude fell back to when it was first started.
    pub fallback_mode: Option<String>,
    pub isolation_id: Option<String>,
    /// `config` was assumed for want of a saved launch.
    pub assumed: bool,
}

impl WakePlan {
    /// Fills in a launch the row does not have: the saved one, else the CLI's own settings.
    fn complete(&mut self, terminals: &Terminals) {
        if self.config.is_some() {
            return;
        }
        if let Some(saved) = terminals.saved_launch(&self.provider, &self.native) {
            self.config = Some(saved.config());
            self.fallback_mode = self.fallback_mode.take().or(saved.fallback_mode);
            return;
        }
        self.config = Some(assumed_config(&self.cwd, None, None));
        self.assumed = true;
    }
}

/// The launch of a session nothing was saved for: where and on what model it ran, with the
/// permissions the CLI itself is set to.
pub(super) fn assumed_config(
    cwd: &Path,
    model: Option<String>,
    effort: Option<String>,
) -> DerivedLaunchConfig {
    DerivedLaunchConfig {
        cwd: normalize_cwd(cwd),
        model,
        effort,
        permissions: json!({}),
    }
}

/// A CLI started again, and what lives as long as it does.
pub(in crate::terminal) struct Relaunched {
    pub session: Arc<Session>,
    pub log_dir: Option<tempfile::TempDir>,
    pub hook_dir: Option<tempfile::TempDir>,
    #[cfg(unix)]
    pub codex: Option<Arc<super::super::codex_server::Client>>,
}

/// What a wake finds at a row: a terminal that already runs, or a plan with the row held for it.
enum Wake {
    Live(Value),
    Begin(Box<WakePlan>, Reviving, u64),
}

impl Running {
    /// Its process has ended, nothing is starting another and no turn still has to settle.
    fn can_revive(&self) -> bool {
        self.session.exit().is_some()
            && self
                .derived
                .as_ref()
                .is_some_and(|d| !d.life.reviving && !d.in_turn())
    }

    fn wake_plan(&self) -> Option<WakePlan> {
        let derived = self.derived.as_ref()?;
        Some(WakePlan {
            provider: self.provider.clone(),
            cwd: self.cwd.clone(),
            native: self.native.clone().filter(|id| !id.is_empty())?,
            account: self.account.clone(),
            config: derived.launch_config.clone(),
            fallback_mode: derived.life.fallback_mode.clone(),
            isolation_id: derived.isolation_id.clone(),
            assumed: false,
        })
    }

    fn hold_for_revival(&mut self) {
        if let Some(derived) = self.derived.as_mut() {
            derived.life.reviving = true;
        }
    }

    /// Puts a new process into this row; what belonged to the old one is returned to be dropped.
    fn replace_process(&mut self, spec: NewDerived) -> impl Sized + use<> {
        let old_life = self
            .derived
            .as_mut()
            .map(|d| std::mem::take(&mut d.life))
            .unwrap_or_default();
        let same_parent = self.derived.as_ref().is_some_and(|d| {
            d.parent_workspace_id == spec.info.parent_workspace_id
                && d.parent_boss_key == spec.info.parent_boss_key
        });
        let mut info = spec.info;
        info.life = Life {
            revived: true,
            boss: old_life.boss.filter(|_| same_parent),
            last_invocation: info.active_invocation_id.clone(),
            title: old_life.title,
            ..Life::default()
        };
        self.cwd = spec.cwd;
        self.account = spec.account;
        self.started = SystemTime::now();
        let session = std::mem::replace(&mut self.session, spec.session);
        #[cfg(unix)]
        {
            self.codex = spec.codex;
        }
        (session, self.derived.replace(info))
    }
}

impl State {
    pub(super) fn derived_row(&mut self, provider: &str, native: &str) -> Option<&mut Running> {
        self.open
            .iter_mut()
            .filter(|r| {
                r.derived.is_some() && r.provider == provider && r.native.as_deref() == Some(native)
            })
            .max_by_key(|r| r.session.exit().is_none())
    }
}

impl Terminals {
    /// The row of the session a boss re-dispatches to. A process on its way out, or one being
    /// started, is waited for, so a session is never resumed by two processes.
    pub(in crate::terminal) fn resume_slot(
        &self,
        provider: &str,
        native: &str,
    ) -> Result<ResumeSlot, String> {
        let deadline = Instant::now() + PENDING_WAIT;
        loop {
            {
                let mut state = self.lock();
                match state.derived_row(provider, native) {
                    None => {
                        if let Some(held) = Reviving::session(self, &mut state, provider, native) {
                            return Ok(ResumeSlot::Free(held));
                        }
                    }
                    Some(running) => {
                        let stopping = running.derived.as_ref().is_some_and(|d| d.life.stopping());
                        if running.session.exit().is_none() && !stopping {
                            return Ok(ResumeSlot::Live(Box::new(running.to_derived_handle())));
                        }
                        if running.can_revive() {
                            running.hold_for_revival();
                            return Ok(ResumeSlot::Dormant(Reviving::row(self, running.id)));
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(RESUME_PENDING_ERR.to_owned());
            }
            thread::sleep(PENDING_STEP);
        }
    }

    /// Puts a new derived terminal into the row held for its session, under that row's ID; gives
    /// it back when there is none.
    pub(in crate::terminal) fn fill_held_row(
        &self,
        spec: NewDerived,
    ) -> Result<u64, Box<NewDerived>> {
        let Some(native) = spec.native.clone() else {
            return Err(Box::new(spec));
        };
        let mut state = self.lock();
        let Some(running) = state
            .derived_row(&spec.provider, &native)
            .filter(|r| r.derived.as_ref().is_some_and(|d| d.life.reviving))
        else {
            return Err(Box::new(spec));
        };
        let id = running.id;
        let old = running.replace_process(spec);
        drop(state);
        drop(old);
        Ok(id)
    }

    /// Ends the process of a session whose launch was only assumed, for a re-dispatch with another
    /// launch, and holds the row for the process started in its place. None when its launch was
    /// given, which a re-dispatch must keep to.
    pub(in crate::terminal) fn retire_assumed(
        &self,
        id: u64,
        provider: &str,
        native: &str,
    ) -> Option<Result<Reviving, String>> {
        let ending = {
            let mut state = self.lock();
            let running = state.open.iter_mut().find(|r| r.id == id)?;
            let derived = running.derived.as_ref()?;
            if !derived.life.config_assumed && derived.launch_config.is_some() {
                return None;
            }
            if derived.in_turn()
                || running.session.has_draft()
                || derived::shows_busy(&running.session)
            {
                return Some(Err(derived::LIVE_BUSY_ERR.to_owned()));
            }
            if derived.life.stopping() || running.session.exit().is_some() {
                return Some(Err(RESUME_PENDING_ERR.to_owned()));
            }
            running.begin_stop()
        };
        ending.end();
        Some(match self.resume_slot(provider, native) {
            Ok(ResumeSlot::Dormant(held) | ResumeSlot::Free(held)) => Ok(held),
            Ok(ResumeSlot::Live(_)) => Err(RESUME_PENDING_ERR.to_owned()),
            Err(error) => Err(error),
        })
    }

    /// A start that failed in a row that was there before leaves its process ended and the row
    /// in place. False when the row is no older than the failed process.
    pub(in crate::terminal) fn collapse_failed_start(&self, id: u64) -> bool {
        let ending = {
            let mut state = self.lock();
            let Some(running) = state.open.iter_mut().find(|r| r.id == id) else {
                return false;
            };
            if !running.derived.as_ref().is_some_and(|d| d.life.revived) {
                return false;
            }
            running.begin_stop()
        };
        ending.end();
        true
    }

    /// Opens a session whose process has ended again in its row. Nothing is submitted to it.
    pub(in crate::terminal) fn wake(&self, id: Option<u64>, attach: &Attach) -> Response {
        self.wake_in(
            &crate::data_files::data_root(),
            id,
            |provider, native| self.unless_derived_elsewhere(provider, native, attach),
            |plan| derived::relaunch(self, plan),
        )
    }

    /// `elsewhere` answers instead when the session runs outside this App's terminals.
    pub(super) fn wake_in(
        &self,
        root: &Path,
        id: Option<u64>,
        elsewhere: impl FnOnce(&str, &str) -> Option<Response>,
        launch: impl FnOnce(&WakePlan) -> Result<Relaunched, String>,
    ) -> Response {
        let (mut plan, held, id) = match self.begin_wake(root, id) {
            Ok(Wake::Begin(plan, held, id)) => (plan, held, id),
            Ok(Wake::Live(live)) => return Response::json(&json!({"session": live})),
            Err(refusal) => return refusal,
        };
        if let Some(refusal) = elsewhere(&plan.provider, &plan.native) {
            return refusal;
        }
        plan.complete(self);
        let (lease, started) = match start(root, &plan, launch) {
            Ok(pair) => pair,
            Err(error) => return Response::error(409, &error),
        };
        let reply = self.install_woken(root, id, &plan, lease, started);
        drop(held);
        wakeup::sync_derived_pids(self);
        reply
    }

    fn begin_wake(&self, root: &Path, id: Option<u64>) -> Result<Wake, Response> {
        let deadline = Instant::now() + PENDING_WAIT;
        loop {
            if let Some(found) = self.look_to_wake(root, id)? {
                return Ok(found);
            }
            if Instant::now() >= deadline {
                return Err(Response::error(409, PENDING_WAKE_ERR));
            }
            thread::sleep(PENDING_STEP);
        }
    }

    /// None while its process is on the way out or another one is being started.
    fn look_to_wake(&self, root: &Path, id: Option<u64>) -> Result<Option<Wake>, Response> {
        let mut state = self.lock();
        let Some(at) = state.open.iter().position(|r| Some(r.id) == id) else {
            return Err(Response::error(404, "找不到这个终端"));
        };
        let running = &state.open[at];
        let Some(derived) = running.derived.as_ref() else {
            return Err(Response::error(409, "这不是派生终端"));
        };
        if running.session.exit().is_none() && !derived.life.stopping() {
            return Ok(Some(Wake::Live(state.describe_one(running, root))));
        }
        if !running.can_revive() {
            return Ok(None);
        }
        let Some(plan) = running.wake_plan() else {
            return Err(Response::error(409, "这个派生会话没有可恢复的会话记录"));
        };
        let running = &mut state.open[at];
        running.hold_for_revival();
        let held = Reviving::row(self, running.id);
        Ok(Some(Wake::Begin(Box::new(plan), held, running.id)))
    }

    fn install_woken(
        &self,
        root: &Path,
        id: u64,
        plan: &WakePlan,
        lease: Option<HeldLease>,
        started: Relaunched,
    ) -> Response {
        let mut state = self.lock();
        let Some(running) = state.open.iter_mut().find(|r| r.id == id) else {
            drop(state);
            #[cfg(unix)]
            if let Some(codex) = &started.codex {
                codex.close();
            }
            started.session.stop();
            return Response::error(409, "终端已关闭");
        };
        let old = running.wake_with(plan, lease, started);
        state.observe_derived(root);
        let described = state
            .open
            .iter()
            .find(|r| r.id == id)
            .map(|running| state.describe_one(running, root));
        drop(state);
        drop(old);
        Response::json(&json!({"session": described}))
    }
}

impl Running {
    fn wake_with(
        &mut self,
        plan: &WakePlan,
        lease: Option<HeldLease>,
        started: Relaunched,
    ) -> impl Sized + use<> {
        let session = std::mem::replace(&mut self.session, started.session);
        #[cfg(unix)]
        {
            self.codex = started.codex;
        }
        self.started = SystemTime::now();
        let old = self.derived.as_mut().map(|derived| {
            derived.blocked_kind = None;
            derived.life.process = Default::default();
            derived.life.reviving = false;
            derived.life.revived = true;
            // Opened by the user: in use, even when its boss is gone.
            derived.life.outlived_boss = true;
            if derived.launch_config.is_none() {
                derived.launch_config = plan.config.clone();
                derived.life.config_assumed = plan.assumed;
            }
            (
                std::mem::replace(&mut derived._lease, lease),
                std::mem::replace(&mut derived._log_dir, started.log_dir),
                std::mem::replace(&mut derived._hook_dir, started.hook_dir),
            )
        });
        (session, old)
    }
}

/// Checks the session, takes its lease and starts its CLI.
pub(super) fn start(
    root: &Path,
    plan: &WakePlan,
    launch: impl FnOnce(&WakePlan) -> Result<Relaunched, String>,
) -> Result<(Option<HeldLease>, Relaunched), String> {
    let lease = prepare(root, plan)?;
    Ok((lease, launch(plan)?))
}

/// Checks that the session can go on where and as whom it ran, and takes the lease of its
/// isolated directory again before the CLI starts in it.
fn prepare(root: &Path, plan: &WakePlan) -> Result<Option<HeldLease>, String> {
    if let Some(error) = account_error(root, &plan.provider, plan.account.as_ref()) {
        return Err(format!("账号不可用：{error}"));
    }
    let missing = || match plan.isolation_id {
        Some(_) => "隔离目录已释放，无法恢复".to_owned(),
        None => "工作目录已不存在，无法恢复".to_owned(),
    };
    if !plan.cwd.is_dir() {
        return Err(missing());
    }
    let Some(isolation_id) = plan.isolation_id.as_deref() else {
        return Ok(None);
    };
    let common_dir = leases::resolve_repo_common_dir(&plan.cwd)?;
    let lease = leases::acquire_lease(&common_dir, isolation_id, LEASE_WAIT)?;
    // Released between the check and the lease; with the lease held it stays.
    if !plan.cwd.is_dir() {
        return Err(missing());
    }
    Ok(Some(lease))
}

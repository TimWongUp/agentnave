//! Derived terminals from busy to idle, their process ended and started again (ADR-0124): the one
//! state the pages show, which terminal the user is looking at, and what `lifecycle/` does with
//! both. Everything here lives in memory only, but for the launches `saved` keeps.

mod boss;
mod idle;
mod open;
mod revive;
mod saved;
#[cfg(all(test, unix))]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::derived::StopTrail;
use super::session::Session;
use super::spawn_requests::now_unix_secs;
use super::{Running, State, Terminals, account};

pub(super) use boss::end_removed;
pub(super) use idle::tick;
pub(super) use revive::{RESUME_PENDING_ERR, Relaunched, ResumeSlot, Reviving, WakePlan};
pub(super) use saved::Store;

/// How long a page's report of the terminal the user is looking at holds.
const SHOWN_TTL: Duration = Duration::from_secs(10);

/// What a derived session is to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DerivedState {
    Starting,
    Running,
    Waiting,
    Uncollected,
    Idle,
    /// Its process has ended and can be started again from its native session.
    Dormant,
    Exited,
}

impl DerivedState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Uncollected => "uncollected",
            Self::Idle => "idle",
            Self::Dormant => "dormant",
            Self::Exited => "exited",
        }
    }
}

/// The lifecycle of one derived terminal's row, across the processes it goes through.
#[derive(Default)]
pub(super) struct Life {
    pub(super) process: Process,
    /// A wake or a re-dispatch is starting a new process for this row.
    reviving: bool,
    /// The row was there before its current process, so a failed start leaves its process ended
    /// and the row in place.
    revived: bool,
    /// The last turn the boss dispatched, and whether its result still waits in the boss's inbox.
    last_invocation: Option<String>,
    uncollected: bool,
    /// The boss terminal it was last seen to belong to.
    boss: Option<u64>,
    /// The boss was already gone while this one was still in use, or when the user opened it.
    outlived_boss: bool,
    state: Option<(DerivedState, f64)>,
    /// The permission mode Claude fell back to, which a wake starts in again.
    fallback_mode: Option<String>,
    /// Its launch was assumed from the service history for want of a saved one: a re-dispatch
    /// with another launch starts it again instead of being refused.
    config_assumed: bool,
    /// Last CLI title observed for this derived session so restarts keep the same name.
    pub(super) title: Option<String>,
}

/// What is known of the CLI process a derived terminal currently runs.
#[derive(Default)]
pub(super) struct Process {
    /// It has been ready for input at least once.
    ready: bool,
    /// It has worked on a turn at least once.
    worked: bool,
    /// It was told to end and may still be on its way out.
    stopping: bool,
    /// Since when nothing has kept its process from being ended.
    quiet_since: Option<Instant>,
    stops: StopTrail,
}

impl Life {
    pub(super) fn claimed_by(&mut self, invocation_id: &str) {
        self.last_invocation = Some(invocation_id.to_owned());
        self.uncollected = false;
        self.process.quiet_since = None;
    }

    pub(super) fn stopping(&self) -> bool {
        self.process.stopping
    }

    pub(super) fn relaunched_in(&mut self, mode: Option<&str>) {
        self.process = Process::default();
        self.fallback_mode = mode.map(str::to_owned);
    }
}

impl Running {
    /// The state shown for a derived session; None for any other terminal.
    pub(super) fn derived_state(&self, root: &Path) -> Option<DerivedState> {
        let derived = self.derived.as_ref()?;
        if self.session.exit().is_some() || derived.life.process.stopping {
            return Some(if self.recoverable(root) {
                DerivedState::Dormant
            } else {
                DerivedState::Exited
            });
        }
        if self.waits_for_user() {
            return Some(DerivedState::Waiting);
        }
        if !derived.life.process.ready && !self.looks_ready() {
            return Some(DerivedState::Starting);
        }
        if self.is_working() {
            return Some(DerivedState::Running);
        }
        Some(if derived.life.uncollected {
            DerivedState::Uncollected
        } else {
            DerivedState::Idle
        })
    }

    /// Whether its session can be taken up again: it has a native ID and an account to run as.
    fn recoverable(&self, root: &Path) -> bool {
        self.native.as_deref().is_some_and(|id| !id.is_empty())
            && account_error(root, &self.provider, self.account.as_ref()).is_none()
    }

    fn waits_for_user(&self) -> bool {
        #[cfg(unix)]
        let thread_waits = self
            .codex
            .as_ref()
            .zip(self.native.as_deref())
            .and_then(|(codex, native)| codex.thread_status(native))
            .is_some_and(|status| status.is_waiting_on_approval());
        #[cfg(not(unix))]
        let thread_waits = false;
        thread_waits
            || self
                .derived
                .as_ref()
                .is_some_and(|d| d.blocked_kind.is_some())
            || self.session.osc_snapshot().1.as_deref() == Some("blocked")
    }

    /// Whether the CLI shows that it takes input by now.
    fn looks_ready(&self) -> bool {
        #[cfg(unix)]
        if let Some(codex) = &self.codex {
            use super::codex_server::ThreadStatus;
            return self.native.as_deref().is_some_and(|native| {
                matches!(
                    codex.thread_status(native),
                    Some(
                        ThreadStatus::Idle
                            | ThreadStatus::Active { .. }
                            | ThreadStatus::SystemError
                    )
                )
            });
        }
        matches!(
            self.session.osc_snapshot().1.as_deref(),
            Some("idle" | "done" | "working" | "error")
        )
    }

    /// Takes note of what the CLI shows now: that it became ready, that it worked, and when the
    /// derived state last changed.
    fn observe(&mut self, root: &Path) {
        let ready = self.looks_ready();
        let worked = self.session.osc_activity_seqs().0 > 0 || self.is_working();
        if self.session.exit().is_none()
            && let Some(derived) = self.derived.as_mut()
        {
            derived.life.process.ready |= ready;
            derived.life.process.worked |= worked;
        }
        let Some(state) = self.derived_state(root) else {
            return;
        };
        if let Some(derived) = self.derived.as_mut()
            && derived.life.state.is_none_or(|(old, _)| old != state)
        {
            derived.life.state = Some((state, now_unix_secs()));
        }
    }

    /// Marks a derived terminal's process as ending, so no turn is dispatched to it any more.
    /// `Ending::end` then ends it and the row stays, its session to be taken up again.
    fn begin_stop(&mut self) -> Ending {
        if let Some(derived) = self.derived.as_mut() {
            derived.life.process.stopping = true;
            derived.life.process.quiet_since = None;
        }
        Ending {
            session: self.session.clone(),
            #[cfg(unix)]
            codex: self.codex.clone().filter(|_| self.derived.is_some()),
        }
    }
}

/// A process marked as ending under the lock, to be ended outside it.
struct Ending {
    session: Arc<Session>,
    #[cfg(unix)]
    codex: Option<Arc<super::codex_server::Client>>,
}

impl Ending {
    fn end(self) {
        #[cfg(unix)]
        if let Some(codex) = &self.codex {
            codex.close();
        }
        self.session.stop();
    }
}

/// Why `account` cannot run `provider` any more: it was removed, moved or its folder is gone.
fn account_error(root: &Path, provider: &str, account: Option<&account::Active>) -> Option<String> {
    let account = account?;
    match account::resolve_in(root, provider, Some(&account.id)) {
        Ok(Some(current)) if current.dir == account.dir && current.dir.is_dir() => None,
        Ok(_) => Some("这个账号的目录已变化".to_owned()),
        Err(error) => Some(error),
    }
}

impl State {
    /// `describe` with what only a derived session has.
    pub(super) fn describe_one(&self, running: &Running, root: &Path) -> Value {
        let mut value = running.describe(root);
        let (Some(derived), Some(state)) = (running.derived.as_ref(), running.derived_state(root))
        else {
            return value;
        };
        let since = derived
            .life
            .state
            .filter(|(seen, _)| *seen == state)
            .map_or_else(now_unix_secs, |(_, since)| since);
        value["derived_state"] = json!(state.as_str());
        value["derived_since"] = json!(since);
        value["has_draft"] = json!(running.session.has_draft());
        // Whatever keeps it from coming back is told when it is opened.
        value["resumable"] = json!(state == DerivedState::Dormant);
        value["parent_terminal_id"] = json!(self.boss_of(running, &HashMap::new()));
        value
    }

    fn is_shown(&self, id: u64, now: Instant) -> bool {
        self.shown
            .get(&id)
            .is_some_and(|at| now.saturating_duration_since(*at) < SHOWN_TTL)
    }

    /// Brings every derived terminal's shown state up to date before it is described.
    pub(super) fn observe_derived(&mut self, root: &Path) {
        for running in self.open.iter_mut().filter(|r| r.derived.is_some()) {
            running.observe(root);
        }
    }
}

impl Terminals {
    /// A page reports the terminal the user is looking at.
    pub(super) fn mark_shown(&self, id: u64) {
        let now = Instant::now();
        let mut state = self.lock();
        state
            .shown
            .retain(|_, at| now.saturating_duration_since(*at) < SHOWN_TTL);
        state.shown.insert(id, now);
    }
}

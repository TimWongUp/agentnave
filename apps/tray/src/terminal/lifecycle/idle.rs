//! Ends the process of derived terminals that stayed idle, and clears away those whose boss is
//! gone. Runs on the background scan's beat, after spawn requests were taken in on the same
//! thread: a dispatch accepted by then has its receipt, which keeps its terminal, and one accepted
//! later finds the terminal marked as ending and waits to start it again.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use super::super::derived::StopTrail;
use super::super::spawn_requests::{Coordinator, RunReceipt};
use super::super::unread::RawBoss;
use super::super::{DerivedInfo, Running, State, Terminals, derived, wakeup};
use super::boss::{Boss, end_removed};
use super::{DerivedState, Ending};
use crate::data::boss_app::boss_key;

/// How long nothing may keep a derived terminal before its process is ended.
pub(super) const IDLE_GRACE: Duration = Duration::from_secs(15 * 60);

/// What one beat of the background scan saw.
pub(super) struct Beat<'a> {
    pub root: &'a Path,
    pub now: Instant,
    /// The boss chats each live terminal's CLI holds.
    pub chats: &'a HashMap<u64, Vec<String>>,
    pub inboxes: &'a HashMap<String, RawBoss>,
    /// Dispatched turns not yet settled, or whose settlement the service has not confirmed.
    pub runs: Vec<RunReceipt>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Due {
    Keep,
    Collapse,
    Remove,
}

pub(in crate::terminal) fn tick(
    terminals: &Terminals,
    coordinator: &Coordinator,
    root: &Path,
    seen: &wakeup::Observed,
    now: Instant,
) {
    let beat = Beat {
        root,
        now,
        chats: &seen.chats,
        inboxes: &seen.inboxes,
        runs: coordinator.open_runs(),
    };
    terminals.collapse_idle(&beat);
}

impl Terminals {
    pub(super) fn collapse_idle(&self, beat: &Beat) {
        let (mut due, leases) = {
            let mut state = self.lock();
            (state.due(beat), state.leases_of_ended())
        };
        drop(leases);
        self.keep_launches();
        if due.is_empty() {
            return;
        }
        // Codex pushes thread states only over a subscription that may have been lost, so they are
        // read afresh before they end anything.
        #[cfg(unix)]
        self.refresh_codex(&mut due);
        let (ended, removed) = {
            let mut state = self.lock();
            let mut ended: Vec<Ending> = Vec::new();
            let mut gone = HashSet::new();
            for id in due {
                let Some(at) = state.open.iter().position(|r| r.id == id) else {
                    continue;
                };
                match state.verdict(at, beat) {
                    Due::Keep => {}
                    Due::Collapse => ended.push(state.open[at].begin_stop()),
                    Due::Remove => {
                        gone.insert(id);
                    }
                }
            }
            (ended, state.take_open(&gone))
        };
        for ending in ended {
            ending.end();
        }
        end_removed(removed);
    }

    #[cfg(unix)]
    fn refresh_codex(&self, due: &mut HashSet<u64>) {
        let clients: Vec<_> = self
            .lock()
            .open
            .iter()
            .filter(|r| due.contains(&r.id) && r.session.exit().is_none())
            .filter_map(|r| Some((r.id, r.codex.clone()?)))
            .collect();
        for (id, client) in clients {
            if !client.refresh_threads() {
                due.remove(&id);
            }
        }
    }
}

impl State {
    fn due(&mut self, beat: &Beat) -> HashSet<u64> {
        let mut due = HashSet::new();
        for at in 0..self.open.len() {
            if self.open[at].derived.is_none() {
                continue;
            }
            self.open[at].note_inbox(beat.inboxes);
            if !self.open[at].is_known_ended() {
                self.open[at].observe(beat.root);
            }
            if self.verdict(at, beat) != Due::Keep {
                due.insert(self.open[at].id);
            }
        }
        due
    }

    /// The leases of derived terminals whose process has ended, taken to be dropped.
    fn leases_of_ended(&mut self) -> Vec<crate::isolated::leases::HeldLease> {
        self.open
            .iter_mut()
            .filter(|r| r.session.exit().is_some())
            .filter_map(|r| r.derived.as_mut())
            .filter(|d| !d.life.reviving && !d.in_turn())
            .filter_map(|d| d._lease.take())
            .collect()
    }

    /// What is due for one derived terminal. Anything that cannot be told keeps it.
    fn verdict(&mut self, at: usize, beat: &Beat) -> Due {
        let boss = self.track_boss(at, beat.chats);
        let shown = self.is_shown(self.open[at].id, beat.now);
        let running = &mut self.open[at];
        let dispatched = running.is_dispatched(&beat.runs);
        let exited = running.session.exit().is_some();
        let quiet = !exited && !shown && !dispatched && running.is_quiet(beat);
        let Some(derived) = running.derived.as_mut() else {
            return Due::Keep;
        };
        let life = &mut derived.life;
        life.outlived_boss &= boss == Boss::Gone;
        if life.reviving || (life.process.stopping && !exited) {
            return Due::Keep;
        }
        if exited {
            return if boss == Boss::Gone && !dispatched {
                Due::Remove
            } else {
                Due::Keep
            };
        }
        if !quiet {
            life.process.quiet_since = None;
            life.outlived_boss = boss == Boss::Gone;
            return Due::Keep;
        }
        let since = *life.process.quiet_since.get_or_insert(beat.now);
        let waited = beat.now.saturating_duration_since(since) >= IDLE_GRACE;
        match boss {
            // Idle the moment its boss went: nothing to wait for.
            Boss::Gone if waited || !life.outlived_boss => Due::Remove,
            Boss::Live | Boss::Unknown if waited => Due::Collapse,
            _ => Due::Keep,
        }
    }
}

impl DerivedInfo {
    /// A dispatched turn holds it, or is about to let go of it.
    pub(super) fn in_turn(&self) -> bool {
        self.busy || self.active_invocation_id.is_some()
    }
}

impl Running {
    /// Its ended state was noted before, so there is nothing to look up again on every beat.
    fn is_known_ended(&self) -> bool {
        self.session.exit().is_some()
            && self.derived.as_ref().is_some_and(|d| {
                matches!(
                    d.life.state,
                    Some((DerivedState::Dormant | DerivedState::Exited, _))
                )
            })
    }

    /// Whether the last dispatched turn's result still waits in the boss's inbox.
    fn note_inbox(&mut self, inboxes: &HashMap<String, RawBoss>) {
        let Some(derived) = self.derived.as_mut() else {
            return;
        };
        let last = derived.life.last_invocation.as_deref();
        derived.life.uncollected = last.is_some_and(|invocation| {
            inboxes.get(&derived.parent_boss_key).is_some_and(|boss| {
                boss.items.iter().any(|item| {
                    item.key == invocation || item.key.strip_prefix("task_end:") == Some(invocation)
                })
            })
        });
    }

    fn is_dispatched(&self, runs: &[RunReceipt]) -> bool {
        self.derived.as_ref().is_some_and(|derived| {
            derived.in_turn()
                || runs.iter().any(|run| {
                    Some(run.invocation_id.as_str()) == derived.life.last_invocation.as_deref()
                        || (run.session_id.is_some()
                            && run.session_id == self.native
                            && run
                                .provider
                                .as_deref()
                                .is_none_or(|provider| provider == self.provider))
                })
        })
    }

    /// Nothing in it would be lost or cut short by ending its process now.
    fn is_quiet(&mut self, beat: &Beat) -> bool {
        matches!(
            self.derived_state(beat.root),
            Some(DerivedState::Idle | DerivedState::Uncollected)
        ) && !self.session.has_draft()
            && !self.is_a_busy_boss(beat)
            && self.has_no_background_work()
    }

    /// It dispatched work itself that still runs or waits to be collected.
    fn is_a_busy_boss(&self, beat: &Beat) -> bool {
        let own = self
            .native
            .as_deref()
            .map(|native| boss_key(&self.provider, native));
        beat.chats
            .get(&self.id)
            .into_iter()
            .flatten()
            .chain(own.as_ref())
            .filter_map(|key| beat.inboxes.get(key))
            .any(|inbox| inbox.in_flight > 0 || inbox.has_unread || inbox.waiting)
    }

    /// Whether the CLI can be told to have nothing left running in the background: Codex by its
    /// threads, Antigravity by its status (it reports working until background commands are
    /// done), Claude by its last `Stop`.
    fn has_no_background_work(&mut self) -> bool {
        #[cfg(unix)]
        if let Some(codex) = &self.codex {
            return self
                .native
                .as_deref()
                .is_some_and(|native| codex.is_idle(native));
        }
        let (_, status, _) = self.session.osc_snapshot();
        if !matches!(status.as_deref(), Some("idle" | "done")) {
            return false;
        }
        match self.provider.as_str() {
            "antigravity" => true,
            "claude" => self.claude_stopped_clear(),
            _ => false,
        }
    }

    fn claude_stopped_clear(&mut self) -> bool {
        let (worked_at, _, _) = self.session.osc_activity_seqs();
        let Some(derived) = self.derived.as_mut() else {
            return false;
        };
        let process = &mut derived.life.process;
        // A process that never worked on a turn started nothing.
        if !process.worked && worked_at == 0 {
            return true;
        }
        let (Some(dir), Some(native)) = (derived._hook_dir.as_ref(), self.native.as_deref()) else {
            return false;
        };
        let events = derived::claude_events_path(dir.path());
        StopTrail::clear(&mut process.stops, &events, native, worked_at)
    }
}

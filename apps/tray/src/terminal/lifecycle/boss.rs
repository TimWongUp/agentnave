//! Which boss terminal a derived terminal belongs to, and what ends with that boss.

use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::super::{Running, State, Terminals, wakeup};
use crate::response::Response;

/// Whether the boss terminal of a derived one still runs. `Unknown` was never seen in the App.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boss {
    Live,
    Gone,
    Unknown,
}

impl State {
    /// The boss terminal a derived one belongs to: one that speaks or spoke for the workspace it
    /// was dispatched from (a boss keeps the workspaces it had before `/clear`), else one whose
    /// CLI holds the boss chat itself (`chats`), else the one it was last seen to belong to. A
    /// terminal that still runs is preferred, then the one currently in that workspace.
    pub(super) fn boss_of(
        &self,
        running: &Running,
        chats: &HashMap<u64, Vec<String>>,
    ) -> Option<u64> {
        let derived = running.derived.as_ref()?;
        let workspace = derived.parent_workspace_id.as_deref();
        self.open
            .iter()
            .filter(|boss| boss.derived.is_none())
            .filter(|boss| {
                workspace.is_some_and(|ws| boss.workspaces.iter().any(|known| known == ws))
                    || chats
                        .get(&boss.id)
                        .is_some_and(|keys| keys.contains(&derived.parent_boss_key))
                    || derived.life.boss == Some(boss.id)
            })
            .max_by_key(|boss| {
                (
                    boss.session.exit().is_none(),
                    workspace.is_some() && boss.workspaces.last().map(String::as_str) == workspace,
                    boss.id,
                )
            })
            .map(|boss| boss.id)
    }

    /// Whether the derived terminal's boss is there, remembering the terminal it belongs to.
    pub(super) fn track_boss(&mut self, at: usize, chats: &HashMap<u64, Vec<String>>) -> Boss {
        let found = self.boss_of(&self.open[at], chats);
        let live = found.is_some_and(|id| {
            self.open
                .iter()
                .any(|r| r.id == id && r.session.exit().is_none())
        });
        let Some(derived) = self.open[at].derived.as_mut() else {
            return Boss::Unknown;
        };
        derived.life.boss = found.or(derived.life.boss);
        match derived.life.boss {
            None => Boss::Unknown,
            Some(_) if live => Boss::Live,
            Some(_) => Boss::Gone,
        }
    }

    fn derived_under(&self, boss: u64) -> HashSet<u64> {
        self.open
            .iter()
            .filter(|r| r.derived.is_some() && self.boss_of(r, &HashMap::new()) == Some(boss))
            .map(|r| r.id)
            .collect()
    }

    pub(super) fn take_open(&mut self, ids: &HashSet<u64>) -> Vec<Running> {
        let (taken, kept) = std::mem::take(&mut self.open)
            .into_iter()
            .partition(|r| ids.contains(&r.id));
        self.open = kept;
        taken
    }
}

/// Ends terminals already taken off the list; a turn still in flight is settled as cut short.
pub(in crate::terminal) fn end_removed(removed: Vec<Running>) {
    for running in removed {
        // Settled first, or a Codex turn cut off by the closed connection reports itself failed.
        if let Some(invocation) = running
            .derived
            .as_ref()
            .and_then(|d| d.active_invocation_id.as_deref())
        {
            wakeup::on_terminal_closed(invocation);
        }
        #[cfg(unix)]
        if let Some(codex) = &running.codex {
            codex.close();
        }
        running.session.stop();
    }
}

impl Terminals {
    /// Ends the terminal's CLI if it still runs and forgets the terminal, and the derived
    /// terminals it dispatched with it.
    pub(in crate::terminal) fn close(&self, id: Option<u64>) -> Response {
        let removed = {
            let mut state = self.lock();
            if let Some(at) = state.logins.iter().position(|login| Some(login.id) == id) {
                let login = state.logins.remove(at);
                drop(state);
                login.session.stop();
                return Response::json(&json!({"ok": true}));
            }
            let Some(at) = state.open.iter().position(|r| Some(r.id) == id) else {
                return Response::error(404, "找不到这个终端");
            };
            let mut removed = state.take_under(at);
            removed.extend(state.take_open(&id.into_iter().collect()));
            removed
        };
        end_removed(removed);
        Response::json(&json!({"ok": true}))
    }

    /// Closes every derived terminal dispatched from `workspace`.
    pub(in crate::terminal) fn close_derived(&self, workspace: &str) -> Response {
        if workspace.is_empty() {
            return Response::error(400, "需要工作区 ID");
        }
        let removed = {
            let mut state = self.lock();
            let ids = state
                .open
                .iter()
                .filter(|r| {
                    r.derived
                        .as_ref()
                        .is_some_and(|d| d.parent_workspace_id.as_deref() == Some(workspace))
                })
                .map(|r| r.id)
                .collect();
            state.take_open(&ids)
        };
        let closed = removed.len();
        end_removed(removed);
        Response::json(&json!({"closed": closed}))
    }
}

impl State {
    /// Takes the derived terminals of the boss at `at` off the list; none when it is no boss.
    fn take_under(&mut self, at: usize) -> Vec<Running> {
        if self.open[at].derived.is_some() {
            return Vec::new();
        }
        let ids = self.derived_under(self.open[at].id);
        self.take_open(&ids)
    }
}

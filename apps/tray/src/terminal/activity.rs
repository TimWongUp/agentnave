//! Real activity state of a non-derived boss terminal for the office status badge (ADR-0127 §4).

#[cfg(all(test, unix))]
mod tests;

use super::Running;

fn osc_activity(osc: Option<&str>) -> &'static str {
    match osc {
        Some("blocked") => "blocked",
        Some("working") => "working",
        Some("idle" | "done" | "error") => "idle",
        _ => "unknown",
    }
}

impl Running {
    /// Activity reported for a live non-derived boss terminal: `"blocked"`, `"working"`,
    /// `"idle"`, or `"unknown"` when the CLI has not emitted an explicit signal yet.
    pub(super) fn boss_activity(&self) -> &'static str {
        let osc = self.session.osc_snapshot().1;
        if osc.as_deref() == Some("blocked") {
            return "blocked";
        }
        #[cfg(unix)]
        if let Some(codex) = &self.codex {
            use super::codex_server::ThreadStatus;
            let status = codex.cached_thread_status_for_boss(
                self.native.as_deref(),
                self.workspaces.first().map(String::as_str),
            );
            return match status {
                Some(ThreadStatus::Active {
                    waiting_on_approval: true,
                }) => "blocked",
                Some(ThreadStatus::Active {
                    waiting_on_approval: false,
                }) => "working",
                Some(ThreadStatus::Idle | ThreadStatus::SystemError) => "idle",
                Some(ThreadStatus::NotLoaded | ThreadStatus::Unknown) | None => {
                    osc_activity(osc.as_deref())
                }
            };
        }
        osc_activity(osc.as_deref())
    }
}

//! What Claude's permanent `Stop` hook last reported running in the background.

use std::path::Path;

use super::turn_state::{HookEvent, StopBackgroundTasks, read_hook_events};

#[derive(Default)]
pub(in crate::terminal) struct StopTrail {
    offset: u64,
    /// Whether the last `Stop` listed no background task that would wake the session; None when
    /// there was none yet, or it could not be told.
    clear: Option<bool>,
    /// The CLI's latest `working` report when that `Stop` was read.
    worked_at: u64,
}

impl StopTrail {
    /// Whether nothing runs in the background. Read only while the CLI is not working: a turn
    /// that ended without a `Stop` (interrupted, failed) leaves this unknown until the next one.
    pub(in crate::terminal) fn clear(
        &mut self,
        events: &Path,
        session_id: &str,
        worked_at: u64,
    ) -> bool {
        let read = read_hook_events(events, &mut self.offset);
        for event in read.events {
            match event {
                HookEvent::Stop(stop) if stop.session_id == session_id => {
                    self.worked_at = worked_at;
                    self.clear = match stop.background_tasks {
                        StopBackgroundTasks::Missing => None,
                        StopBackgroundTasks::Present(tasks) => Some(tasks.is_empty()),
                    };
                }
                HookEvent::StopFailure {
                    session_id: failed, ..
                } if failed == session_id => self.clear = None,
                _ => {}
            }
        }
        if !read.warnings.is_empty() {
            // Events were dropped, so the last one seen may not be the last one written.
            self.clear = None;
        }
        self.clear == Some(true) && self.worked_at >= worked_at
    }
}

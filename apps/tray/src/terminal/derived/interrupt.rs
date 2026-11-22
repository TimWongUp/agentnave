//! Esc-based per-turn interruption for the Claude and Antigravity TUIs (ADR-0109).

use std::thread;
use std::time::{Duration, Instant};

use super::super::DerivedHandle;
use super::super::spawn_requests::{
    Coordinator, ReceiptState, RunHandle, SpawnRequestEnvelope, truncate_short,
};
use super::{POLL_STEP, finished_cancel_outcome};

const ESC_CONFIRM_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(800)
} else {
    Duration::from_secs(5)
};

/// What the CLI's native log says about the turn the cancel request targets.
pub(super) enum TurnProbe {
    Active,
    Completed,
    Superseded,
}

/// Keeps the turn monitor from settling while an Esc outcome is still undecided.
struct PendingGuard<'a>(Option<&'a RunHandle>);

impl<'a> PendingGuard<'a> {
    fn new(run: Option<&'a RunHandle>) -> Self {
        if let Some(run) = run {
            run.set_interrupt_pending(true);
        }
        Self(run)
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(run) = self.0 {
            run.set_interrupt_pending(false);
        }
    }
}

/// Press Esc on the TUI and report `interrupted` only after OSC 7501 leaves the
/// working state on the same turn. The run itself is marked cancelled only then.
pub(super) fn interrupt_turn_with_esc(
    coordinator: &Coordinator,
    env: &SpawnRequestEnvelope,
    handle: &DerivedHandle,
    sid: &str,
    probe: &dyn Fn(Option<&str>) -> TurnProbe,
) {
    let finish = |status: &str, reason: Option<&str>| {
        coordinator.mark_finished(&env.request_id, status, reason, Some(sid));
    };
    let (seq0, state, _) = handle.session.osc_snapshot();
    match probe(state.as_deref()) {
        TurnProbe::Completed => return finish("already_finished", None),
        TurnProbe::Superseded => return finish("not_safe_to_interrupt", Some("已进入后续轮次")),
        TurnProbe::Active => {}
    }
    match state.as_deref() {
        Some("working" | "blocked") => {}
        Some("done" | "error") => return finish("already_finished", None),
        _ => return finish("not_safe_to_interrupt", Some("当前轮尚未处于运行状态")),
    }

    let run = coordinator.active_run(&env.invocation_id);
    let _pending = PendingGuard::new(run.as_deref());
    if let Some(outcome) = settled_run_outcome(coordinator, &env.invocation_id) {
        return finish(outcome, None);
    }
    if let Err(err) = handle.session.write(b"\x1b") {
        return finish("not_safe_to_interrupt", Some(&truncate_short(&err)));
    }

    let deadline = Instant::now() + ESC_CONFIRM_TIMEOUT;
    let settled_state = loop {
        let (seq, state, _) = handle.session.osc_snapshot();
        if seq > seq0 {
            match state.as_deref() {
                Some("idle" | "done") => break state,
                Some("error") => return finish("already_finished", Some("终端已报告错误状态")),
                _ => {}
            }
        }
        if let Some(outcome) = settled_run_outcome(coordinator, &env.invocation_id) {
            return finish(outcome, None);
        }
        if Instant::now() >= deadline {
            return finish(
                "not_safe_to_interrupt",
                Some("按下 Esc 后终端状态未变化，当前轮继续运行"),
            );
        }
        thread::sleep(POLL_STEP);
    };

    match probe(settled_state.as_deref()) {
        TurnProbe::Completed => finish("already_finished", None),
        TurnProbe::Superseded => finish("not_safe_to_interrupt", Some("已进入后续轮次")),
        TurnProbe::Active => {
            if let Some(run) = &run {
                run.confirm_interrupt();
            }
            finish("interrupted", None);
        }
    }
}

fn settled_run_outcome(coordinator: &Coordinator, invocation_id: &str) -> Option<&'static str> {
    coordinator
        .run_receipt_by_invocation(invocation_id)
        .filter(|r| r.state == ReceiptState::Finished)
        .map(|r| finished_cancel_outcome(&r))
}

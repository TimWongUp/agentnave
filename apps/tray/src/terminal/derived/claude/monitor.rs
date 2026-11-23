//! Claude Code derived-turn monitor loop (Stop + turn_duration pairing, interrupts, bg reminders).

use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::spawn_requests::{Coordinator, RunHandle};
use super::super::super::{DerivedHandle, Terminals};
use super::super::tui::{
    MISSING_ANCHOR_WAIT, TurnBaseline, finish_exited, finish_settled, finish_settled_with,
    sync_turn_blocker,
};
use super::super::{POLL_STEP, claude_hook, native_logs};
use super::ClaudeTurnResult;
use super::turn_state::{ClaudeMonitorConfig, ClaudeTurnDecision, ClaudeTurnTracker};

const TRANSCRIPT_POLL: Duration = Duration::from_millis(100);

#[allow(clippy::too_many_arguments)]
pub(in crate::terminal::derived) fn monitor_claude_turn(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    sid: &str,
    baseline: &TurnBaseline,
    prompt: &str,
    handle: &Arc<RunHandle>,
    hook_dir: Option<&Path>,
    check_mode_hook: bool,
) -> ClaudeTurnResult {
    monitor_claude_turn_with_config(
        coordinator,
        terminals,
        launched,
        req_id,
        sid,
        baseline,
        prompt,
        handle,
        hook_dir,
        check_mode_hook,
        ClaudeMonitorConfig::default(),
    )
}

struct ClaudeMonitorState {
    started: bool,
    mode_confirmed: bool,
    prompt_id: Option<String>,
    blocker: Option<(String, f64)>,
    saw_working: bool,
    missing_anchor_since: Option<Instant>,
    last_transcript_check: Instant,
    tracker: ClaudeTurnTracker,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::terminal::derived) fn monitor_claude_turn_with_config(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    sid: &str,
    baseline: &TurnBaseline,
    prompt: &str,
    handle: &Arc<RunHandle>,
    hook_dir: Option<&Path>,
    check_mode_hook: bool,
    cfg: ClaudeMonitorConfig,
) -> ClaudeTurnResult {
    let mut state = ClaudeMonitorState {
        started: false,
        mode_confirmed: !check_mode_hook || hook_dir.is_none(),
        prompt_id: None,
        blocker: None,
        saw_working: false,
        missing_anchor_since: None,
        last_transcript_check: Instant::now() - TRANSCRIPT_POLL,
        tracker: ClaudeTurnTracker::new(sid, baseline.hook_events_offset, cfg),
    };
    let events_file = hook_dir.map(claude_hook::events_path);

    loop {
        if let Some(res) = step_claude_mode_and_hooks(
            coordinator,
            terminals,
            launched,
            req_id,
            sid,
            hook_dir,
            events_file.as_deref(),
            handle,
            &mut state,
        ) {
            return res;
        }
        try_bind_claude_anchor(coordinator, req_id, sid, baseline, prompt, &mut state);

        let (seq, osc_state, osc_kind) = launched.session.osc_snapshot();
        let state_str = osc_state.as_deref();
        if let Some(res) = step_claude_osc(
            coordinator,
            terminals,
            launched,
            req_id,
            sid,
            baseline.seq_before,
            seq,
            state_str,
            osc_kind.as_deref(),
            hook_dir,
            handle,
            &mut state,
        ) {
            return res;
        }

        if let Some(res) = step_claude_transcript(
            coordinator,
            terminals,
            launched,
            req_id,
            sid,
            baseline,
            state_str,
            handle,
            &mut state,
        ) {
            return res;
        }

        if let Some(code) = launched.session.exit() {
            handle.set_bg_only_waiting(false);
            if !state.mode_confirmed
                && let Some(dir) = hook_dir
                && claude_hook::read_observed_mode(dir).as_deref() == Some("default")
            {
                return ClaudeTurnResult::ModeBlocked;
            }
            finish_exited(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                code,
                false,
                handle.is_interrupt_confirmed(),
            );
            return ClaudeTurnResult::Finished;
        }
        thread::sleep(POLL_STEP);
    }
}

#[allow(clippy::too_many_arguments)]
fn step_claude_mode_and_hooks(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    sid: &str,
    hook_dir: Option<&Path>,
    events_file: Option<&Path>,
    handle: &Arc<RunHandle>,
    state: &mut ClaudeMonitorState,
) -> Option<ClaudeTurnResult> {
    if !state.mode_confirmed
        && let Some(dir) = hook_dir
        && let Some(actual) = claude_hook::read_observed_mode(dir)
    {
        if actual == "default" {
            return Some(ClaudeTurnResult::ModeBlocked);
        }
        state.mode_confirmed = true;
    }
    if let Some(path) = events_file
        && let Some(err_msg) = state.tracker.ingest_hook_file(path, Instant::now())
    {
        handle.set_bg_only_waiting(false);
        if !state.mode_confirmed
            && let Some(dir) = hook_dir
            && claude_hook::read_observed_mode(dir).as_deref() == Some("default")
        {
            return Some(ClaudeTurnResult::ModeBlocked);
        }
        finish_settled(
            coordinator,
            terminals,
            launched.id,
            req_id,
            Some(sid),
            true,
            false,
            &err_msg,
        );
        return Some(ClaudeTurnResult::Finished);
    }
    None
}

fn try_bind_claude_anchor(
    coordinator: &Coordinator,
    req_id: &str,
    sid: &str,
    baseline: &TurnBaseline,
    prompt: &str,
    state: &mut ClaudeMonitorState,
) {
    if state.mode_confirmed
        && state.prompt_id.is_none()
        && let Some(path) = native_logs::claude_transcript_path(&baseline.claude_homes, sid)
        && let Some(pid) = native_logs::claude_prompt_anchor(&path, baseline.pre_offset, prompt)
    {
        state.prompt_id = Some(pid.clone());
        state.missing_anchor_since = None;
        let anchor = json!({"session_id": sid, "prompt_id": pid});
        if !state.started {
            state.started = true;
            coordinator.mark_started(req_id, Some(sid), Some(anchor));
        } else {
            coordinator.update_anchor(req_id, anchor);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn step_claude_osc(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    sid: &str,
    seq_before: u64,
    seq: u64,
    state_str: Option<&str>,
    osc_kind: Option<&str>,
    hook_dir: Option<&Path>,
    handle: &Arc<RunHandle>,
    state: &mut ClaudeMonitorState,
) -> Option<ClaudeTurnResult> {
    if !handle.interrupt_pending() && handle.is_interrupt_confirmed() {
        handle.set_bg_only_waiting(false);
        finish_settled(
            coordinator,
            terminals,
            launched.id,
            req_id,
            Some(sid),
            false,
            true,
            "",
        );
        return Some(ClaudeTurnResult::Finished);
    }
    if seq <= seq_before {
        return None;
    }
    if state_str == Some("working") {
        state.saw_working = true;
        state.missing_anchor_since = None;
        if state.mode_confirmed && !state.started {
            state.started = true;
            coordinator.mark_started(req_id, Some(sid), Some(json!({"session_id": sid})));
        }
    }
    sync_turn_blocker(
        coordinator,
        terminals,
        launched.id,
        req_id,
        Some(sid),
        state_str,
        osc_kind,
        &mut state.blocker,
        &mut state.missing_anchor_since,
    );
    if !handle.interrupt_pending() && state_str == Some("error") {
        handle.set_bg_only_waiting(false);
        finish_settled(
            coordinator,
            terminals,
            launched.id,
            req_id,
            Some(sid),
            true,
            false,
            "CLI 返回错误状态",
        );
        return Some(ClaudeTurnResult::Finished);
    }
    let osc_idle_or_done =
        state_str == Some("done") || (state.saw_working && state_str == Some("idle"));
    if state.prompt_id.is_none() && osc_idle_or_done {
        if state.mode_confirmed && !state.started {
            state.started = true;
            coordinator.mark_started(req_id, Some(sid), Some(json!({"session_id": sid})));
        }
        let at = *state.missing_anchor_since.get_or_insert_with(Instant::now);
        if !handle.interrupt_pending() && at.elapsed() >= MISSING_ANCHOR_WAIT {
            if !state.mode_confirmed
                && let Some(dir) = hook_dir
                && claude_hook::read_observed_mode(dir).as_deref() == Some("default")
            {
                return Some(ClaudeTurnResult::ModeBlocked);
            }
            finish_settled(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                true,
                false,
                "缺少本轮锚点（未在 Claude 转录中匹配到本次 prompt）",
            );
            return Some(ClaudeTurnResult::Finished);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn step_claude_transcript(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    sid: &str,
    baseline: &TurnBaseline,
    state_str: Option<&str>,
    handle: &Arc<RunHandle>,
    state: &mut ClaudeMonitorState,
) -> Option<ClaudeTurnResult> {
    if !state.mode_confirmed
        || handle.interrupt_pending()
        || state.last_transcript_check.elapsed() < TRANSCRIPT_POLL
    {
        return None;
    }
    let pid = state.prompt_id.as_deref()?;
    state.last_transcript_check = Instant::now();
    let scan = native_logs::claude_transcript_path(&baseline.claude_homes, sid)
        .map(|p| native_logs::claude_dispatch_scan(&p, pid))
        .unwrap_or_default();
    let (decision, bg_only, reminder) = state.tracker.step(
        pid,
        &scan,
        state_str,
        handle.is_bg_only_cancel_requested(),
        Instant::now(),
    );
    handle.set_bg_only_waiting(bg_only);
    if let Some(rem) = reminder {
        coordinator.set_background_tasks_pending(req_id, rem);
    }
    match decision {
        ClaudeTurnDecision::Continue => None,
        ClaudeTurnDecision::Settled {
            end_prompt_id,
            end_row_index,
            warnings,
        } => {
            handle.set_bg_only_waiting(false);
            let mut patch = serde_json::Map::new();
            if let Some(ep) = end_prompt_id {
                patch.insert("end_prompt_id".to_owned(), Value::String(ep));
            }
            if let Some(er) = end_row_index {
                patch.insert("end_row_index".to_owned(), json!(er));
            }
            finish_settled_with(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                false,
                false,
                "",
                (!patch.is_empty()).then_some(Value::Object(patch)),
                warnings,
            );
            Some(ClaudeTurnResult::Finished)
        }
        ClaudeTurnDecision::InterruptedByUser => {
            handle.set_bg_only_waiting(false);
            finish_settled_with(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                false,
                true,
                "当前轮被用户中断",
                None,
                Vec::new(),
            );
            Some(ClaudeTurnResult::Finished)
        }
        ClaudeTurnDecision::Failed(reason) => {
            handle.set_bg_only_waiting(false);
            finish_settled(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                true,
                false,
                &reason,
            );
            Some(ClaudeTurnResult::Finished)
        }
    }
}

/// This prompt's turn has a `turn_duration` row (with no active `<task-notification>` sub-turn
/// after it) or a later human prompt in the transcript. `Unknown` never counts as ended.
#[cfg(test)]
pub(in crate::terminal::derived) fn claude_turn_ended(
    baseline: &TurnBaseline,
    sid: &str,
    prompt_id: &str,
) -> bool {
    native_logs::claude_transcript_path(&baseline.claude_homes, sid).is_some_and(|p| {
        matches!(
            native_logs::claude_turn_state(&p, prompt_id),
            native_logs::ClaudeTurnState::Completed | native_logs::ClaudeTurnState::Superseded(_)
        )
    })
}

//! Antigravity (agy) derived-turn monitor (OSC 7501 `done` + final reply, bg reminder, bg-only cancel).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::super::spawn_requests::{
    BackgroundTasksReminder, Coordinator, RunHandle, now_unix_secs,
};
use super::super::super::{DerivedHandle, Terminals};
use super::super::claude::BG_WAIT_REMINDER_DELAY;
use super::super::{POLL_STEP, native_logs};
use super::{
    MISSING_ANCHOR_WAIT, TurnBaseline, finish_exited, finish_settled, finish_settled_with,
    sync_turn_blocker,
};

pub(in crate::terminal::derived) const AGY_DONE_REPLY_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(600)
} else {
    Duration::from_secs(5)
};

#[derive(Clone, Copy, Debug)]
pub(in crate::terminal::derived) struct AgyMonitorConfig {
    pub done_reply_wait: Duration,
    pub bg_reminder_delay: Duration,
}

impl Default for AgyMonitorConfig {
    fn default() -> Self {
        Self {
            done_reply_wait: AGY_DONE_REPLY_WAIT,
            bg_reminder_delay: BG_WAIT_REMINDER_DELAY,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::terminal::derived) fn monitor_agy_turn(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    existing_cid: Option<&str>,
    log_path: Option<&Path>,
    baseline: &TurnBaseline,
    prompt: &str,
    handle: &Arc<RunHandle>,
) {
    monitor_agy_turn_with_config(
        coordinator,
        terminals,
        launched,
        req_id,
        existing_cid,
        log_path,
        baseline,
        prompt,
        handle,
        native_logs::agy_home(),
        AgyMonitorConfig::default(),
    );
}

struct AgyMonitorState {
    confirmed_cid: Option<String>,
    user_input_index: Option<i64>,
    blocker: Option<(String, f64)>,
    done_since: Option<Instant>,
    bg_waiting_since: Option<Instant>,
    bg_reminder_emitted: bool,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::terminal::derived) fn monitor_agy_turn_with_config(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    existing_cid: Option<&str>,
    log_path: Option<&Path>,
    baseline: &TurnBaseline,
    prompt: &str,
    handle: &Arc<RunHandle>,
    agy_home: Option<PathBuf>,
    cfg: AgyMonitorConfig,
) {
    let mut state = AgyMonitorState {
        confirmed_cid: existing_cid.map(str::to_owned),
        user_input_index: None,
        blocker: None,
        done_since: None,
        bg_waiting_since: None,
        bg_reminder_emitted: false,
    };

    loop {
        try_bind_agy_anchor(
            coordinator,
            terminals,
            launched,
            req_id,
            agy_home.as_deref(),
            log_path,
            baseline,
            prompt,
            &mut state,
        );

        let (seq, osc_state, osc_kind) = launched.session.osc_snapshot();
        let state_str = osc_state.as_deref();
        if !handle.interrupt_pending() && handle.is_interrupt_confirmed() {
            handle.set_bg_only_waiting(false);
            finish_settled(
                coordinator,
                terminals,
                launched.id,
                req_id,
                state.confirmed_cid.as_deref(),
                false,
                true,
                "",
            );
            return;
        }
        if seq > baseline.seq_before {
            if state_str != Some("done") {
                state.done_since = None;
            }
            sync_turn_blocker(
                coordinator,
                terminals,
                launched.id,
                req_id,
                state.confirmed_cid.as_deref(),
                state_str,
                osc_kind.as_deref(),
                &mut state.blocker,
                &mut state.done_since,
            );
            if !handle.interrupt_pending() && state_str == Some("error") {
                handle.set_bg_only_waiting(false);
                finish_settled(
                    coordinator,
                    terminals,
                    launched.id,
                    req_id,
                    state.confirmed_cid.as_deref(),
                    true,
                    false,
                    "Antigravity 返回错误状态",
                );
                return;
            }
            if state_str == Some("done") {
                state.done_since.get_or_insert_with(Instant::now);
            }
        }

        if step_agy_transcript(
            coordinator,
            terminals,
            launched,
            req_id,
            agy_home.as_deref(),
            state_str,
            handle,
            cfg,
            &mut state,
        ) {
            return;
        }

        if let Some(code) = launched.session.exit() {
            handle.set_bg_only_waiting(false);
            finish_exited(
                coordinator,
                terminals,
                launched.id,
                req_id,
                state.confirmed_cid.as_deref(),
                code,
                false,
                handle.is_interrupt_confirmed(),
            );
            return;
        }
        thread::sleep(POLL_STEP);
    }
}

#[allow(clippy::too_many_arguments)]
fn try_bind_agy_anchor(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    agy_home: Option<&Path>,
    log_path: Option<&Path>,
    baseline: &TurnBaseline,
    prompt: &str,
    state: &mut AgyMonitorState,
) {
    if state.user_input_index.is_none()
        && let Some(home) = agy_home
        && let Some((cid, idx)) = locate_agy_turn(
            home,
            state.confirmed_cid.as_deref(),
            log_path,
            &baseline.agy_baseline_cids,
            baseline.agy_prev_idx,
            prompt,
        )
    {
        state.confirmed_cid = Some(cid.clone());
        state.user_input_index = Some(idx);
        terminals.set_derived_blocker(launched.id, Some(&cid), None);
        coordinator.mark_started(
            req_id,
            Some(&cid),
            Some(json!({"session_id": cid, "user_input_index": idx})),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn step_agy_transcript(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    agy_home: Option<&Path>,
    state_str: Option<&str>,
    handle: &Arc<RunHandle>,
    cfg: AgyMonitorConfig,
    state: &mut AgyMonitorState,
) -> bool {
    if handle.interrupt_pending() {
        return false;
    }
    let scan = match (
        agy_home,
        state.confirmed_cid.as_deref(),
        state.user_input_index,
    ) {
        (Some(home), Some(cid), Some(idx)) => Some(native_logs::agy_dispatch_scan(
            &native_logs::agy_transcript_path(home, cid),
            idx,
        )),
        _ => None,
    };

    if let Some(scan) = &scan {
        if let Some((_, next_row)) = scan.superseded_by {
            handle.set_bg_only_waiting(false);
            if scan.has_final_reply {
                let end_row = scan
                    .last_reply_end_row
                    .map(|r| r.min(next_row))
                    .unwrap_or(next_row);
                finish_settled_with(
                    coordinator,
                    terminals,
                    launched.id,
                    req_id,
                    state.confirmed_cid.as_deref(),
                    false,
                    false,
                    "",
                    Some(json!({"end_row_index": end_row})),
                    vec![
                        "用户在当前派发进行中发送了新消息，已按插话前回复结算（用户参与）"
                            .to_owned(),
                    ],
                );
            } else {
                finish_settled(
                    coordinator,
                    terminals,
                    launched.id,
                    req_id,
                    state.confirmed_cid.as_deref(),
                    true,
                    false,
                    "用户在当前轮完成前插话，归属未确认",
                );
            }
            return true;
        }

        let bg_waiting = scan.has_final_reply && state_str == Some("working");
        handle.set_bg_only_waiting(bg_waiting);
        if bg_waiting && handle.is_bg_only_cancel_requested() {
            handle.set_bg_only_waiting(false);
            let end_row = scan.last_reply_end_row.unwrap_or(scan.segment_end_row);
            finish_settled_with(
                coordinator,
                terminals,
                launched.id,
                req_id,
                state.confirmed_cid.as_deref(),
                false,
                false,
                "",
                Some(json!({"end_row_index": end_row})),
                vec![
                    "取消时仅剩后台任务运行，未中断终端；已按已写出的回复结算，后台任务留给用户"
                        .to_owned(),
                ],
            );
            return true;
        }
        if bg_waiting {
            let since = *state.bg_waiting_since.get_or_insert_with(Instant::now);
            if !state.bg_reminder_emitted && since.elapsed() >= cfg.bg_reminder_delay {
                state.bg_reminder_emitted = true;
                coordinator.set_background_tasks_pending(
                    req_id,
                    BackgroundTasksReminder {
                        code: "background_tasks_pending".to_owned(),
                        since: now_unix_secs(),
                        count: None,
                        tasks: Vec::new(),
                        reason: "Antigravity 已写出回复但后台任务仍在运行；任务继续运行，可继续等待或取消按已有回复结算".to_owned(),
                    },
                );
            }
        } else {
            state.bg_waiting_since = None;
        }
    } else {
        handle.set_bg_only_waiting(false);
        state.bg_waiting_since = None;
    }

    let Some(at) = state.done_since else {
        return false;
    };
    if let Some(scan) = &scan {
        if scan.latest_planner_is_final {
            handle.set_bg_only_waiting(false);
            let end_row = scan.last_reply_end_row.unwrap_or(scan.segment_end_row);
            finish_settled_with(
                coordinator,
                terminals,
                launched.id,
                req_id,
                state.confirmed_cid.as_deref(),
                false,
                false,
                "",
                Some(json!({"end_row_index": end_row})),
                Vec::new(),
            );
            return true;
        }
        if at.elapsed() >= cfg.done_reply_wait {
            handle.set_bg_only_waiting(false);
            if scan.has_final_reply {
                let end_row = scan.last_reply_end_row.unwrap_or(scan.segment_end_row);
                finish_settled_with(
                    coordinator,
                    terminals,
                    launched.id,
                    req_id,
                    state.confirmed_cid.as_deref(),
                    false,
                    false,
                    "",
                    Some(json!({"end_row_index": end_row})),
                    Vec::new(),
                );
            } else {
                finish_settled(
                    coordinator,
                    terminals,
                    launched.id,
                    req_id,
                    state.confirmed_cid.as_deref(),
                    true,
                    false,
                    "Antigravity 已报告 done，但在等待时间内未在转录中读到最终回复",
                );
            }
            return true;
        }
    } else if at.elapsed() >= MISSING_ANCHOR_WAIT {
        handle.set_bg_only_waiting(false);
        finish_settled(
            coordinator,
            terminals,
            launched.id,
            req_id,
            state.confirmed_cid.as_deref(),
            true,
            false,
            "缺少本轮锚点（未在 Antigravity 转录中匹配到本次输入）",
        );
        return true;
    }
    false
}

fn locate_agy_turn(
    home: &Path,
    existing: Option<&str>,
    log_path: Option<&Path>,
    baseline_cids: &[String],
    prev_idx: Option<i64>,
    prompt: &str,
) -> Option<(String, i64)> {
    let candidates: Vec<String> = if let Some(cid) = existing {
        vec![cid.to_owned()]
    } else if let Some(log) = log_path {
        native_logs::agy_log_conversation_ids(log)
            .into_iter()
            .filter(|cid| !baseline_cids.iter().any(|b| b == cid))
            .collect()
    } else {
        Vec::new()
    };
    for cid in candidates {
        let transcript = native_logs::agy_transcript_path(home, &cid);
        if let Some(idx) = native_logs::agy_match_user_input(&transcript, prev_idx, prompt) {
            return Some((cid, idx));
        }
    }
    None
}

pub(super) fn highest_agy_user_index(path: &Path) -> Option<i64> {
    let text = fs::read_to_string(path).ok()?;
    let mut best = None;
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row["type"].as_str() == Some("USER_INPUT")
            && row["source"].as_str() == Some("USER_EXPLICIT")
            && let Some(step) = row["step_index"].as_i64()
        {
            best = Some(best.map_or(step, |cur: i64| cur.max(step)));
        }
    }
    best
}

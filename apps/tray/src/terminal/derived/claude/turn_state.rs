//! Claude derived-turn completion pairing (`Stop` + `turn_duration`), background task tracking,
//! waiting reminder timer, and bg-only cancellation state machine (ADR-0114 Section 4).

use std::collections::{BTreeSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::super::spawn_requests::{BackgroundTasksReminder, now_unix_secs};
use super::super::native_logs::{ClaudeDispatchScan, ClaudeTurnDuration};

pub(in crate::terminal::derived) const SIGNAL_PAIR_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(600)
} else {
    Duration::from_secs(5)
};

pub(in crate::terminal::derived) const BG_WAIT_REMINDER_DELAY: Duration =
    Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug)]
pub(in crate::terminal::derived) struct ClaudeMonitorConfig {
    pub signal_pair_timeout: Duration,
    pub bg_reminder_delay: Duration,
}

impl Default for ClaudeMonitorConfig {
    fn default() -> Self {
        Self {
            signal_pair_timeout: SIGNAL_PAIR_TIMEOUT,
            bg_reminder_delay: BG_WAIT_REMINDER_DELAY,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WakingBackgroundTask {
    pub id: String,
    pub kind: String,
}

impl WakingBackgroundTask {
    pub(super) fn key(&self) -> String {
        if !self.id.is_empty() {
            self.id.clone()
        } else {
            self.kind.clone()
        }
    }

    pub(super) fn label(&self) -> String {
        if !self.id.is_empty() && !self.kind.is_empty() {
            format!("{}:{}", self.kind, self.id)
        } else if !self.id.is_empty() {
            self.id.clone()
        } else {
            self.kind.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StopBackgroundTasks {
    /// Older Claude Code version where `background_tasks` field is absent (or not an array).
    Missing,
    /// `background_tasks` array was present; contains only waking active tasks.
    Present(Vec<WakingBackgroundTask>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StopEvent {
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub background_tasks: StopBackgroundTasks,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HookEvent {
    Stop(StopEvent),
    StopFailure {
        session_id: Option<String>,
        message: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClaudeTurnDecision {
    Continue,
    Settled {
        end_prompt_id: Option<String>,
        end_row_index: Option<usize>,
        warnings: Vec<String>,
    },
    InterruptedByUser,
    Failed(String),
}

pub(super) fn is_waking_task_type(kind: &str) -> bool {
    let normalized = kind.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "shell"
            | "bash"
            | "local_bash"
            | "monitor"
            | "subagent"
            | "agent"
            | "local_agent"
            | "remote_agent"
            | "in_process_teammate"
            | "mcp_task"
            | "mcp"
            | "workflow"
            | "local_workflow"
    )
}

fn is_terminal_task_status(status: Option<&str>) -> bool {
    let Some(status) = status else {
        return false;
    };
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "completed" | "failed" | "killed" | "stopped" | "cancelled" | "canceled" | "done"
    )
}

pub(super) fn parse_hook_event(row: &Value) -> Option<HookEvent> {
    let name = row["hook_event_name"].as_str()?;
    let session_id = row["session_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    match name {
        "Stop" => {
            let prompt_id = row["prompt_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let background_tasks = match row.get("background_tasks") {
                Some(Value::Array(items)) => {
                    let waking = items
                        .iter()
                        .filter_map(|item| {
                            let kind = item["type"].as_str().unwrap_or_default();
                            if !is_waking_task_type(kind)
                                || is_terminal_task_status(item["status"].as_str())
                            {
                                return None;
                            }
                            let id = item["id"].as_str().unwrap_or_default().to_owned();
                            Some(WakingBackgroundTask {
                                id,
                                kind: kind.to_owned(),
                            })
                        })
                        .collect();
                    StopBackgroundTasks::Present(waking)
                }
                _ => StopBackgroundTasks::Missing,
            };
            Some(HookEvent::Stop(StopEvent {
                session_id,
                prompt_id,
                background_tasks,
            }))
        }
        "StopFailure" => {
            let err = row["error"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("Claude StopFailure");
            let detail = row["error_details"]
                .as_str()
                .or_else(|| row["last_assistant_message"].as_str())
                .filter(|s| !s.is_empty());
            let message = match detail {
                Some(d) => format!("{err}: {d}"),
                None => err.to_owned(),
            };
            Some(HookEvent::StopFailure {
                session_id,
                message,
            })
        }
        _ => None,
    }
}

/// Incrementally read complete newline-terminated hook events from `path` starting at `*offset`.
pub(super) fn read_hook_events(path: &Path, offset: &mut u64) -> Vec<HookEvent> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(n) = reader.read_line(&mut line) else {
            break;
        };
        if n == 0 || !line.ends_with('\n') {
            break;
        }
        *offset += n as u64;
        if let Ok(row) = serde_json::from_str::<Value>(&line)
            && let Some(event) = parse_hook_event(&row)
        {
            events.push(event);
        }
    }
    events
}

pub(super) struct ClaudeTurnTracker {
    sid: String,
    cfg: ClaudeMonitorConfig,
    hook_offset: u64,
    seen_durations: usize,
    pending_stops: VecDeque<(Instant, StopEvent)>,
    pending_durations: VecDeque<(Instant, ClaudeTurnDuration)>,
    active_bg_tasks: Vec<WakingBackgroundTask>,
    bg_waiting_since: Option<Instant>,
    bg_task_keys: BTreeSet<String>,
    bg_reminder_emitted: bool,
    warnings: Vec<String>,
    last_paired_end_row: Option<usize>,
    last_paired_prompt_id: Option<String>,
}

impl ClaudeTurnTracker {
    pub(super) fn new(sid: &str, hook_offset: u64, cfg: ClaudeMonitorConfig) -> Self {
        Self {
            sid: sid.to_owned(),
            cfg,
            hook_offset,
            seen_durations: 0,
            pending_stops: VecDeque::new(),
            pending_durations: VecDeque::new(),
            active_bg_tasks: Vec::new(),
            bg_waiting_since: None,
            bg_task_keys: BTreeSet::new(),
            bg_reminder_emitted: false,
            warnings: Vec::new(),
            last_paired_end_row: None,
            last_paired_prompt_id: None,
        }
    }

    fn push_warning(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.iter().any(|w| w == &msg) {
            self.warnings.push(msg);
        }
    }

    /// Poll hook events file. Returns `Some(err_msg)` immediately on `StopFailure`.
    pub(super) fn ingest_hook_file(&mut self, events_path: &Path, now: Instant) -> Option<String> {
        for ev in read_hook_events(events_path, &mut self.hook_offset) {
            match ev {
                HookEvent::StopFailure {
                    session_id,
                    message,
                } => {
                    if session_id.as_deref().is_none_or(|s| s == self.sid) {
                        return Some(message);
                    }
                }
                HookEvent::Stop(stop) => {
                    if stop.session_id.as_deref().is_none_or(|s| s == self.sid) {
                        self.pending_stops.push_back((now, stop));
                    }
                }
            }
        }
        None
    }

    /// Evaluate transcript scan, pair signals, update waiting reminder & bg-only state.
    pub(super) fn step(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        osc_state: Option<&str>,
        bg_only_cancel_requested: bool,
        now: Instant,
    ) -> (ClaudeTurnDecision, bool, Option<BackgroundTasksReminder>) {
        if scan.found_start && scan.turn_durations.len() > self.seen_durations {
            for dur in &scan.turn_durations[self.seen_durations..] {
                self.pending_durations.push_back((now, dur.clone()));
            }
            self.seen_durations = scan.turn_durations.len();
        }

        if scan.interrupted_by_user {
            return (ClaudeTurnDecision::InterruptedByUser, false, None);
        }

        if let Some((_, next_row)) = &scan.superseded_by {
            let decision = self.settle_superseded(start_prompt_id, scan, *next_row);
            return (decision, false, None);
        }

        // Discard Stops guaranteed to belong to prompts prior to this dispatch.
        while self.pending_stops.front().is_some_and(|(_, stop)| {
            stop.prompt_id
                .as_deref()
                .is_some_and(|pid| scan.prior_prompt_ids.contains(pid))
        }) {
            self.pending_stops.pop_front();
        }

        if let Some(decision) = self.pair_signals(start_prompt_id, scan, now) {
            return (decision, false, None);
        }

        if let Some(decision) = self.handle_pair_timeout(start_prompt_id, scan, now) {
            return (decision, false, None);
        }

        let bg_only = scan.has_assistant_reply
            && !self.active_bg_tasks.is_empty()
            && !scan.active_subturn_after_last_duration
            && self.pending_stops.is_empty()
            && self.pending_durations.is_empty()
            && osc_state != Some("blocked");

        if bg_only && bg_only_cancel_requested {
            let decision = self.settle_bg_only_cancel(start_prompt_id, scan);
            return (decision, false, None);
        }

        let reminder = self.check_bg_reminder(scan.has_assistant_reply, now);
        (ClaudeTurnDecision::Continue, bg_only, reminder)
    }

    fn pair_signals(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        now: Instant,
    ) -> Option<ClaudeTurnDecision> {
        let mut ready_to_settle = false;
        while !self.pending_stops.is_empty() && !self.pending_durations.is_empty() {
            let (_, stop) = self.pending_stops.pop_front().expect("checked non-empty");
            let (_, dur) = self
                .pending_durations
                .pop_front()
                .expect("checked non-empty");
            self.last_paired_end_row = Some(dur.row_index + 1);
            self.last_paired_prompt_id = dur.prompt_id.or(stop.prompt_id);
            match stop.background_tasks {
                StopBackgroundTasks::Missing => {
                    self.push_warning(
                        "Claude Stop 钩子未提供 background_tasks 字段，后台任务完成未确认",
                    );
                    self.clear_bg_wait();
                    ready_to_settle = true;
                }
                StopBackgroundTasks::Present(waking) if waking.is_empty() => {
                    self.clear_bg_wait();
                    ready_to_settle = true;
                }
                StopBackgroundTasks::Present(waking) => {
                    ready_to_settle = false;
                    self.update_active_bg_tasks(waking, scan.has_assistant_reply, now);
                }
            }
        }
        if ready_to_settle
            && !scan.active_subturn_after_last_duration
            && self.pending_stops.is_empty()
            && self.pending_durations.is_empty()
        {
            return Some(self.finalize_settled(
                start_prompt_id,
                scan,
                self.last_paired_end_row,
                self.last_paired_prompt_id.clone(),
            ));
        }
        None
    }

    fn handle_pair_timeout(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        now: Instant,
    ) -> Option<ClaudeTurnDecision> {
        if self.pending_stops.is_empty()
            && let Some((at, _)) = self.pending_durations.front()
            && now.saturating_duration_since(*at) >= self.cfg.signal_pair_timeout
        {
            let (_, dur) = self.pending_durations.pop_front().expect("checked");
            self.last_paired_end_row = Some(dur.row_index + 1);
            self.last_paired_prompt_id = dur.prompt_id;
            self.push_warning(
                "未在时限内收到 Claude Stop 钩子，已退回 transcript 结算，后台任务完成未确认",
            );
            if !scan.active_subturn_after_last_duration && self.pending_durations.is_empty() {
                return Some(self.finalize_settled(
                    start_prompt_id,
                    scan,
                    self.last_paired_end_row,
                    self.last_paired_prompt_id.clone(),
                ));
            }
        }

        if self.pending_durations.is_empty()
            && let Some((at, _)) = self.pending_stops.front()
            && now.saturating_duration_since(*at) >= self.cfg.signal_pair_timeout
        {
            let (_, stop) = self.pending_stops.pop_front().expect("checked");
            let end_pid = stop.prompt_id.or_else(|| scan.last_reply_prompt_id.clone());
            match stop.background_tasks {
                StopBackgroundTasks::Missing => {
                    self.push_warning(
                        "未在时限内收到 Claude turn_duration，已退回 transcript 结算，后台任务完成未确认",
                    );
                    self.clear_bg_wait();
                    return Some(self.finalize_settled(
                        start_prompt_id,
                        scan,
                        scan.last_reply_end_row.or(Some(scan.segment_end_row)),
                        end_pid,
                    ));
                }
                StopBackgroundTasks::Present(waking) if waking.is_empty() => {
                    self.push_warning(
                        "未在时限内收到 Claude turn_duration，已按 Stop 与 transcript 结算",
                    );
                    self.clear_bg_wait();
                    return Some(self.finalize_settled(
                        start_prompt_id,
                        scan,
                        scan.last_reply_end_row.or(Some(scan.segment_end_row)),
                        end_pid,
                    ));
                }
                StopBackgroundTasks::Present(waking) => {
                    self.push_warning("未在时限内收到 Claude turn_duration，继续等待后台任务完成");
                    self.update_active_bg_tasks(waking, scan.has_assistant_reply, now);
                }
            }
        }
        None
    }

    fn update_active_bg_tasks(
        &mut self,
        waking: Vec<WakingBackgroundTask>,
        has_reply: bool,
        now: Instant,
    ) {
        let new_keys: BTreeSet<String> = waking.iter().map(WakingBackgroundTask::key).collect();
        let changed = new_keys != self.bg_task_keys;
        self.active_bg_tasks = waking;
        self.bg_task_keys = new_keys;
        if has_reply && (self.bg_waiting_since.is_none() || changed) {
            self.bg_waiting_since = Some(now);
        }
    }

    fn clear_bg_wait(&mut self) {
        self.active_bg_tasks.clear();
        self.bg_task_keys.clear();
        self.bg_waiting_since = None;
    }

    fn check_bg_reminder(
        &mut self,
        has_reply: bool,
        now: Instant,
    ) -> Option<BackgroundTasksReminder> {
        if self.bg_reminder_emitted || !has_reply || self.active_bg_tasks.is_empty() {
            return None;
        }
        let since = *self.bg_waiting_since.get_or_insert(now);
        if now.saturating_duration_since(since) < self.cfg.bg_reminder_delay {
            return None;
        }
        self.bg_reminder_emitted = true;
        let labels: Vec<String> = self
            .active_bg_tasks
            .iter()
            .map(WakingBackgroundTask::label)
            .collect();
        let count = self.active_bg_tasks.len();
        let reason = if labels.is_empty() {
            format!("仍有 {count} 个后台任务运行中；任务继续运行，可继续等待或取消按已有回复结算")
        } else {
            format!(
                "仍有 {count} 个后台任务运行中（{}）；任务继续运行，可继续等待或取消按已有回复结算",
                labels.join(", ")
            )
        };
        Some(BackgroundTasksReminder {
            code: "background_tasks_pending".to_owned(),
            since: now_unix_secs(),
            count: Some(count),
            tasks: labels,
            reason,
        })
    }

    fn settle_superseded(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        next_row: usize,
    ) -> ClaudeTurnDecision {
        if !scan.has_assistant_reply {
            return ClaudeTurnDecision::Failed("用户在当前轮完成前插话，归属未确认".to_owned());
        }
        if !self.active_bg_tasks.is_empty() {
            self.push_warning(
                "用户在后台任务完成前发送了新消息，已按插话前的子回合结算（用户参与，后续后台任务归属未确认）",
            );
        } else {
            self.push_warning("用户在当前轮完成前插话，已按插话前的回复结算（用户参与）");
        }
        let end_row = scan
            .last_reply_end_row
            .map(|r| r.min(next_row))
            .unwrap_or(next_row);
        self.finalize_settled(
            start_prompt_id,
            scan,
            Some(end_row),
            scan.last_reply_prompt_id.clone(),
        )
    }

    fn settle_bg_only_cancel(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
    ) -> ClaudeTurnDecision {
        let labels: Vec<String> = self
            .active_bg_tasks
            .iter()
            .map(WakingBackgroundTask::label)
            .collect();
        let msg = if labels.is_empty() {
            "取消时仅剩后台任务运行，未中断终端；已按已写出的回复结算，后台任务留给用户".to_owned()
        } else {
            format!(
                "取消时仅剩后台任务运行（{}），未中断终端；已按已写出的回复结算，后台任务留给用户",
                labels.join(", ")
            )
        };
        self.push_warning(msg);
        let end_row = self
            .last_paired_end_row
            .or(scan.last_reply_end_row)
            .unwrap_or(scan.segment_end_row);
        let end_pid = self
            .last_paired_prompt_id
            .clone()
            .or_else(|| scan.last_reply_prompt_id.clone());
        self.finalize_settled(start_prompt_id, scan, Some(end_row), end_pid)
    }

    fn finalize_settled(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        end_row: Option<usize>,
        end_pid: Option<String>,
    ) -> ClaudeTurnDecision {
        if scan.user_interjected_queued {
            self.push_warning("用户在当前派发进行中参与了对话");
        }
        ClaudeTurnDecision::Settled {
            end_prompt_id: end_pid.or_else(|| Some(start_prompt_id.to_owned())),
            end_row_index: end_row.or(Some(scan.segment_end_row)),
            warnings: self.warnings.clone(),
        }
    }
}

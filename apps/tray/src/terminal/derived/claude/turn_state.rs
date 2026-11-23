//! Claude derived-turn completion pairing (`Stop` + `turn_duration`), background task tracking,
//! waiting reminder timer, and bg-only cancellation state machine (ADR-0114 Section 4).

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::super::spawn_requests::{BackgroundTasksReminder, now_unix_secs};
use super::super::native_logs::{self, ClaudeDispatchScan, ClaudeTurnDuration};

pub(in crate::terminal::derived) const SIGNAL_PAIR_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(600)
} else {
    Duration::from_secs(5)
};

pub(in crate::terminal::derived) const BG_WAIT_REMINDER_DELAY: Duration =
    Duration::from_secs(30 * 60);

pub(in crate::terminal::derived) const MAX_HOOK_LINE_BYTES: usize = 256 * 1024;
pub(in crate::terminal::derived) const MAX_HOOK_EVENTS_PER_POLL: usize = 256;
pub(in crate::terminal::derived) const MAX_PENDING_PAIR_QUEUE: usize = 64;

/// Background task types reported by Claude Code's `Stop.background_tasks` that wake the main turn.
/// Keep in sync with `_WAKING_TASK_TYPES` and ID fallback order (`id` -> `task_id` -> `tool_use_id`
/// -> `<kind>:<index>`) in `src/agentnave/claude_tui/turn.py`.
pub(super) const WAKING_TASK_TYPES: &[&str] = &[
    "shell",
    "bash",
    "local_bash",
    "monitor",
    "subagent",
    "agent",
    "local_agent",
    "remote_agent",
    "in_process_teammate",
    "mcp",
    "mcp_task",
    "mcp_tool",
    "remote_mcp",
    "workflow",
    "local_workflow",
];

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

fn normalize_task_type(kind: &str) -> String {
    kind.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal::derived) struct WakingBackgroundTask {
    pub id: String,
    pub kind: String,
    pub index: usize,
}

impl WakingBackgroundTask {
    pub(super) fn key(&self) -> String {
        if !self.id.is_empty() {
            self.id.clone()
        } else {
            format!("{}:{}", normalize_task_type(&self.kind), self.index)
        }
    }

    pub(super) fn label(&self) -> String {
        if !self.id.is_empty() && !self.kind.is_empty() {
            format!("{}:{}", self.kind, self.id)
        } else if !self.id.is_empty() {
            self.id.clone()
        } else {
            format!("{}:{}", normalize_task_type(&self.kind), self.index)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal::derived) enum StopBackgroundTasks {
    /// Older Claude Code version where `background_tasks` field is absent (or not an array).
    Missing,
    /// `background_tasks` array was present; contains only waking active tasks.
    Present(Vec<WakingBackgroundTask>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal::derived) struct StopEvent {
    pub session_id: String,
    pub prompt_id: Option<String>,
    pub background_tasks: StopBackgroundTasks,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal::derived) enum HookEvent {
    Stop(StopEvent),
    StopFailure { session_id: String, message: String },
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::terminal::derived) enum ClaudeTurnDecision {
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
    let normalized = normalize_task_type(kind);
    WAKING_TASK_TYPES.contains(&normalized.as_str())
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

pub(in crate::terminal::derived) fn parse_hook_event(row: &Value) -> Option<HookEvent> {
    let name = row["hook_event_name"].as_str()?;
    let session_id = row["session_id"]
        .as_str()
        .filter(|s| !s.is_empty())?
        .to_owned();
    match name {
        "Stop" => {
            let prompt_id = row["prompt_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let background_tasks = match row.get("background_tasks") {
                Some(Value::Array(items)) => {
                    let mut seen = HashSet::new();
                    let waking = items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| {
                            let kind = item["type"].as_str().unwrap_or_default();
                            if !is_waking_task_type(kind)
                                || is_terminal_task_status(item["status"].as_str())
                            {
                                return None;
                            }
                            let id = ["id", "task_id", "tool_use_id"]
                                .iter()
                                .find_map(|field| item[*field].as_str().filter(|s| !s.is_empty()))
                                .unwrap_or_default()
                                .to_owned();
                            let task = WakingBackgroundTask {
                                id,
                                kind: kind.to_owned(),
                                index,
                            };
                            seen.insert(task.key()).then_some(task)
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

pub(in crate::terminal::derived) struct HookReadResult {
    pub events: Vec<HookEvent>,
    pub warnings: Vec<String>,
}

enum HookLine {
    EofOrPartial,
    Line(String),
    Oversized,
}

fn read_next_hook_line(reader: &mut BufReader<File>, offset: &mut u64) -> HookLine {
    let mut buf = Vec::new();
    let Ok(n) = reader
        .by_ref()
        .take((MAX_HOOK_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut buf)
    else {
        return HookLine::EofOrPartial;
    };
    if n == 0 {
        return HookLine::EofOrPartial;
    }
    if buf.len() <= MAX_HOOK_LINE_BYTES {
        if !buf.ends_with(b"\n") {
            return HookLine::EofOrPartial;
        }
        *offset += buf.len() as u64;
        return HookLine::Line(String::from_utf8_lossy(&buf).into_owned());
    }
    let mut total = buf.len() as u64;
    let mut ended = buf.ends_with(b"\n");
    while !ended {
        let Ok(avail) = reader.fill_buf() else {
            return HookLine::EofOrPartial;
        };
        if avail.is_empty() {
            return HookLine::EofOrPartial;
        }
        if let Some(pos) = avail.iter().position(|&b| b == b'\n') {
            let step = pos + 1;
            total += step as u64;
            reader.consume(step);
            ended = true;
        } else {
            let step = avail.len();
            total += step as u64;
            reader.consume(step);
        }
    }
    *offset += total;
    HookLine::Oversized
}

/// Incrementally read bounded newline-terminated hook events from `path` starting at `*offset`.
pub(in crate::terminal::derived) fn read_hook_events(
    path: &Path,
    offset: &mut u64,
) -> HookReadResult {
    let mut out = HookReadResult {
        events: Vec::new(),
        warnings: Vec::new(),
    };
    let Some(mut file) = native_logs::open_regular_readonly(path) else {
        return out;
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return out;
    }
    let mut reader = BufReader::new(file);
    loop {
        match read_next_hook_line(&mut reader, offset) {
            HookLine::EofOrPartial => break,
            HookLine::Oversized => {
                let msg = "Claude hook 单行事件超过上限，已丢弃".to_owned();
                if !out.warnings.contains(&msg) {
                    out.warnings.push(msg);
                }
            }
            HookLine::Line(line) => {
                if out.events.len() >= MAX_HOOK_EVENTS_PER_POLL {
                    let msg = "Claude hook 单轮事件数超过上限，已丢弃多余事件".to_owned();
                    if !out.warnings.contains(&msg) {
                        out.warnings.push(msg);
                    }
                    continue;
                }
                if let Ok(row) = serde_json::from_str::<Value>(&line)
                    && let Some(event) = parse_hook_event(&row)
                {
                    out.events.push(event);
                }
            }
        }
    }
    out
}

pub(in crate::terminal::derived) struct ClaudeTurnTracker {
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
    timed_out_stop_subturn_row: Option<usize>,
    ready_to_settle_after_drain: bool,
}

impl ClaudeTurnTracker {
    pub(in crate::terminal::derived) fn new(
        sid: &str,
        hook_offset: u64,
        cfg: ClaudeMonitorConfig,
    ) -> Self {
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
            timed_out_stop_subturn_row: None,
            ready_to_settle_after_drain: false,
        }
    }

    fn push_warning(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.iter().any(|w| w == &msg) {
            self.warnings.push(msg);
        }
    }

    /// Poll hook events file. Returns `Some(err_msg)` immediately on `StopFailure`.
    pub(in crate::terminal::derived) fn ingest_hook_file(
        &mut self,
        events_path: &Path,
        now: Instant,
    ) -> Option<String> {
        let read = read_hook_events(events_path, &mut self.hook_offset);
        for w in read.warnings {
            self.push_warning(w);
        }
        for ev in read.events {
            match ev {
                HookEvent::StopFailure {
                    session_id,
                    message,
                } => {
                    if session_id == self.sid {
                        return Some(message);
                    }
                }
                HookEvent::Stop(stop) => {
                    if stop.session_id == self.sid {
                        if self.pending_stops.len() >= MAX_PENDING_PAIR_QUEUE {
                            self.push_warning("Claude Stop 待配对队列超过上限，已丢弃");
                        } else {
                            self.pending_stops.push_back((now, stop));
                        }
                    }
                }
            }
        }
        None
    }

    /// Evaluate transcript scan, pair signals, update waiting reminder & bg-only state.
    pub(in crate::terminal::derived) fn step(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        osc_state: Option<&str>,
        bg_only_cancel_requested: bool,
        now: Instant,
    ) -> (ClaudeTurnDecision, bool, Option<BackgroundTasksReminder>) {
        if scan.found_start && scan.turn_durations.len() > self.seen_durations {
            for dur in &scan.turn_durations[self.seen_durations..] {
                if self.timed_out_stop_subturn_row == Some(scan.current_subturn_start_row)
                    && !self.active_bg_tasks.is_empty()
                    && self.pending_stops.is_empty()
                {
                    self.timed_out_stop_subturn_row = None;
                    self.last_paired_end_row = Some(dur.row_index + 1);
                    if let Some(pid) = &dur.prompt_id {
                        self.last_paired_prompt_id = Some(pid.clone());
                    }
                } else if self.pending_durations.len() >= MAX_PENDING_PAIR_QUEUE {
                    self.push_warning("Claude turn_duration 待配对队列超过上限，已丢弃");
                } else {
                    self.pending_durations.push_back((now, dur.clone()));
                    self.ready_to_settle_after_drain = false;
                }
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

        let subturn_idle = !scan.active_subturn_after_last_duration
            || self.timed_out_stop_subturn_row == Some(scan.current_subturn_start_row);
        let bg_only = scan.has_assistant_reply
            && !self.active_bg_tasks.is_empty()
            && subturn_idle
            && self.pending_stops.is_empty()
            && self.pending_durations.is_empty()
            && osc_state != Some("blocked");

        if bg_only && bg_only_cancel_requested {
            let decision = self.settle_bg_only_cancel(start_prompt_id, scan);
            return (decision, false, None);
        }

        let reminder = self.check_bg_reminder(scan.has_assistant_reply && subturn_idle, now);
        (ClaudeTurnDecision::Continue, bg_only, reminder)
    }

    fn pop_matching_pair(&mut self) -> Option<(StopEvent, ClaudeTurnDuration)> {
        let mut match_idx: Option<(usize, usize)> = None;
        for (si, (_, stop)) in self.pending_stops.iter().enumerate() {
            if let Some(sp) = stop.prompt_id.as_deref()
                && let Some(di) = self
                    .pending_durations
                    .iter()
                    .position(|(_, dur)| dur.prompt_id.as_deref() == Some(sp))
            {
                match_idx = Some((si, di));
                break;
            }
        }
        if match_idx.is_none() {
            'outer: for (si, (_, stop)) in self.pending_stops.iter().enumerate() {
                for (di, (_, dur)) in self.pending_durations.iter().enumerate() {
                    if stop.prompt_id.is_none() || dur.prompt_id.is_none() {
                        match_idx = Some((si, di));
                        break 'outer;
                    }
                }
            }
        }
        let (si, di) = match_idx?;
        let (_, stop) = self.pending_stops.remove(si)?;
        let (_, dur) = self.pending_durations.remove(di)?;
        Some((stop, dur))
    }

    fn pair_signals(
        &mut self,
        start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        now: Instant,
    ) -> Option<ClaudeTurnDecision> {
        while let Some((stop, dur)) = self.pop_matching_pair() {
            self.timed_out_stop_subturn_row = None;
            self.last_paired_end_row = Some(dur.row_index + 1);
            self.last_paired_prompt_id = dur.prompt_id.or(stop.prompt_id);
            match stop.background_tasks {
                StopBackgroundTasks::Missing => {
                    self.push_warning(
                        "Claude Stop 钩子未提供 background_tasks 字段，后台任务完成未确认",
                    );
                    self.clear_bg_wait();
                    self.ready_to_settle_after_drain = true;
                }
                StopBackgroundTasks::Present(waking) if waking.is_empty() => {
                    self.clear_bg_wait();
                    self.ready_to_settle_after_drain = true;
                }
                StopBackgroundTasks::Present(waking) => {
                    self.ready_to_settle_after_drain = false;
                    self.update_active_bg_tasks(waking, scan.has_assistant_reply, now);
                }
            }
        }
        if self.ready_to_settle_after_drain
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
        if let Some((at, _)) = self.pending_durations.front()
            && now.saturating_duration_since(*at) >= self.cfg.signal_pair_timeout
        {
            let (_, dur) = self.pending_durations.pop_front().expect("checked");
            self.last_paired_end_row = Some(dur.row_index + 1);
            self.last_paired_prompt_id = dur.prompt_id;
            self.ready_to_settle_after_drain = true;
            self.push_warning(
                "未在时限内收到 Claude Stop 钩子，已退回 transcript 结算，后台任务完成未确认",
            );
            if !scan.active_subturn_after_last_duration
                && self.pending_durations.is_empty()
                && self.pending_stops.is_empty()
                && self.active_bg_tasks.is_empty()
            {
                return Some(self.finalize_settled(
                    start_prompt_id,
                    scan,
                    self.last_paired_end_row,
                    self.last_paired_prompt_id.clone(),
                ));
            }
        }

        while let Some((at, _)) = self.pending_stops.front()
            && now.saturating_duration_since(*at) >= self.cfg.signal_pair_timeout
        {
            let (_, stop) = self.pending_stops.pop_front().expect("checked");
            let foreign_pid = stop.prompt_id.as_deref().is_some_and(|sp| {
                scan.found_start
                    && !scan.owned_prompt_ids.iter().any(|id| id == sp)
                    && !scan.has_unlabeled_task_notification
            });
            if foreign_pid {
                self.push_warning("收到不匹配当前回合 prompt_id 的 Stop 事件，已忽略");
                if self.ready_to_settle_after_drain
                    && self.last_paired_end_row.is_some()
                    && !scan.active_subturn_after_last_duration
                    && self.pending_stops.is_empty()
                    && self.pending_durations.is_empty()
                    && self.active_bg_tasks.is_empty()
                {
                    return Some(self.finalize_settled(
                        start_prompt_id,
                        scan,
                        self.last_paired_end_row,
                        self.last_paired_prompt_id.clone(),
                    ));
                }
                continue;
            }
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
                    self.timed_out_stop_subturn_row = Some(scan.current_subturn_start_row);
                    self.ready_to_settle_after_drain = false;
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
        _start_prompt_id: &str,
        scan: &ClaudeDispatchScan,
        end_row: Option<usize>,
        end_pid: Option<String>,
    ) -> ClaudeTurnDecision {
        if scan.user_interjected_queued {
            self.push_warning("用户在当前派发进行中参与了对话");
        }
        ClaudeTurnDecision::Settled {
            end_prompt_id: end_pid,
            end_row_index: end_row.or(Some(scan.segment_end_row)),
            warnings: self.warnings.clone(),
        }
    }
}

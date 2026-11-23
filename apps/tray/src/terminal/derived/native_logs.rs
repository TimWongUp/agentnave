//! Pure read-only native log/transcript/rollout inspectors for derived terminal turns (ADR-0109, ADR-0114).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::data_files::valid_uuid;

pub(super) fn flat_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn raw_claude_user_text(row: &Value) -> Option<String> {
    if row["type"].as_str() != Some("user")
        || row["isSidechain"].as_bool() == Some(true)
        || row["isMeta"].as_bool() == Some(true)
    {
        return None;
    }
    let content = &row["message"]["content"];
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let blocks = content.as_array()?;
    let mut parts = Vec::new();
    let mut has_prompt_block = false;
    for block in blocks {
        let kind = block["type"].as_str();
        if kind != Some("tool_result") {
            has_prompt_block = true;
            if let Some(text) = block["text"].as_str() {
                parts.push(text);
            }
        }
    }
    has_prompt_block.then(|| parts.join("\n"))
}

pub(super) fn is_claude_task_notification(row: &Value) -> bool {
    if row["type"].as_str() != Some("user") || row["isSidechain"].as_bool() == Some(true) {
        return false;
    }
    if matches!(
        row["origin"]["kind"].as_str(),
        Some("task-notification" | "task_notification")
    ) || matches!(
        row["turnOrigin"].as_str(),
        Some("task_notification" | "task-notification")
    ) {
        return true;
    }
    let Some(text) = raw_claude_user_text(row) else {
        return false;
    };
    let trimmed = text.trim();
    if row["promptSource"].as_str() == Some("system") && trimmed.contains("<task-notification>") {
        return true;
    }
    row["promptSource"].is_null()
        && trimmed.starts_with("<task-notification>")
        && trimmed.contains("</task-notification>")
}

pub(super) fn is_claude_interrupt_row(row: &Value) -> bool {
    let Some(text) = raw_claude_user_text(row) else {
        return false;
    };
    let trimmed = text.trim();
    trimmed.starts_with("[Request interrupted by user") && trimmed.ends_with(']')
}

pub(super) fn is_claude_user_queued_interjection(row: &Value) -> bool {
    if row["isSidechain"].as_bool() == Some(true) {
        return false;
    }
    let attachment = &row["attachment"];
    if attachment["type"].as_str() != Some("queued_command") {
        return false;
    }
    if matches!(
        row["origin"]["kind"].as_str(),
        Some("task-notification" | "task_notification")
    ) {
        return false;
    }
    let Some(prompt) = attachment["prompt"].as_str() else {
        return false;
    };
    let trimmed = prompt.trim();
    !trimmed.is_empty() && !trimmed.starts_with("<task-notification>")
}

fn is_claude_user_prompt(row: &Value) -> Option<String> {
    if is_claude_task_notification(row) || is_claude_interrupt_row(row) {
        return None;
    }
    raw_claude_user_text(row)
}

fn assistant_row_has_text(row: &Value) -> bool {
    if row["type"].as_str() != Some("assistant") || row["isSidechain"].as_bool() == Some(true) {
        return false;
    }
    let Some(blocks) = row["message"]["content"].as_array() else {
        return false;
    };
    blocks.iter().any(|block| {
        block["type"].as_str() == Some("text")
            && block["text"].as_str().is_some_and(|s| !s.trim().is_empty())
    })
}

pub(super) fn claude_homes(account_dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(dir) = account_dir {
        return vec![dir.to_owned()];
    }
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    dirs::home_dir()
        .map(|h| vec![h.join(".claude")])
        .unwrap_or_default()
}

pub(super) fn claude_transcript_path(homes: &[PathBuf], session_id: &str) -> Option<PathBuf> {
    if !valid_uuid(session_id) {
        return None;
    }
    let name = format!("{session_id}.jsonl");
    homes.iter().find_map(|home| {
        fs::read_dir(home.join("projects"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join(&name))
            .find(|path| path.is_file())
    })
}

pub(super) fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

/// Find the `promptId` of the user prompt written to `path` at or after `offset`.
/// Never falls back to another prompt: returns `None` until the expected text matches.
pub(super) fn claude_prompt_anchor(
    path: &Path,
    offset: u64,
    expected_prompt: &str,
) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let _ = file.seek(SeekFrom::Start(offset));
    let reader = BufReader::new(file);
    let expected = flat_whitespace(expected_prompt);
    if expected.is_empty() {
        return None;
    }
    for line in reader.lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(text) = is_claude_user_prompt(&row) else {
            continue;
        };
        let Some(pid) = row["promptId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        if flat_whitespace(unwrap_claude_pasted(&text)) == expected {
            return Some(pid);
        }
    }
    None
}

/// Claude Code 2.1.330 records a pasted prompt as `<pasted_content id="x">…</pasted_content id="x">`.
fn unwrap_claude_pasted(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("<pasted_content id=\"") else {
        return text;
    };
    let Some((id, body)) = rest.split_once("\">") else {
        return text;
    };
    let tagged = format!("</pasted_content id=\"{id}\">");
    body.strip_suffix(tagged.as_str())
        .or_else(|| body.strip_suffix("</pasted_content>"))
        .unwrap_or(text)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClaudeTurnDuration {
    pub row_index: usize,
    pub prompt_id: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ClaudeDispatchScan {
    pub found_start: bool,
    pub prior_prompt_ids: HashSet<String>,
    pub owned_prompt_ids: Vec<String>,
    pub turn_durations: Vec<ClaudeTurnDuration>,
    pub has_assistant_reply: bool,
    pub last_reply_end_row: Option<usize>,
    pub last_reply_prompt_id: Option<String>,
    pub segment_end_row: usize,
    pub interrupted_by_user: bool,
    pub user_interjected_queued: bool,
    pub superseded_by: Option<(String, usize)>,
    pub active_subturn_after_last_duration: bool,
}

fn resolve_parent_prompt_id(
    nodes: &HashMap<String, (Option<String>, Option<String>)>,
    start_parent: Option<&str>,
) -> Option<String> {
    let mut cur = start_parent?;
    let mut visited = HashSet::new();
    while visited.insert(cur) {
        let (parent, pid) = nodes.get(cur)?;
        if let Some(found) = pid {
            return Some(found.clone());
        }
        cur = parent.as_deref()?;
    }
    None
}

/// Scan a Claude JSONL transcript for one dispatch starting at `start_prompt_id`.
/// Includes `<task-notification>` sub-turns up to the next genuine human user prompt.
pub(super) fn claude_dispatch_scan(path: &Path, start_prompt_id: &str) -> ClaudeDispatchScan {
    let mut scan = ClaudeDispatchScan::default();
    let Ok(file) = File::open(path) else {
        return scan;
    };
    let reader = BufReader::new(file);
    let mut nodes: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut current_subturn_pid: Option<String> = None;
    let mut row_idx = 0usize;

    for line in reader.lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if !row.is_object() {
            continue;
        }
        let idx = row_idx;
        row_idx += 1;

        if let Some(uuid) = row["uuid"].as_str().filter(|s| !s.is_empty()) {
            let parent = row["parentUuid"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let pid = row["promptId"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            nodes.insert(uuid.to_owned(), (parent, pid));
        }

        if row["isSidechain"].as_bool() == Some(true) {
            continue;
        }

        let row_pid = row["promptId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        if is_claude_task_notification(&row) {
            if !scan.found_start {
                if let Some(pid) = row_pid {
                    scan.prior_prompt_ids.insert(pid);
                }
                continue;
            }
            if let Some(pid) = row_pid {
                if !scan.owned_prompt_ids.iter().any(|seen| seen == &pid) {
                    scan.owned_prompt_ids.push(pid.clone());
                }
                current_subturn_pid = Some(pid);
            }
            scan.active_subturn_after_last_duration = true;
            scan.interrupted_by_user = false;
            continue;
        }

        if is_claude_interrupt_row(&row) {
            if scan.found_start {
                scan.interrupted_by_user = true;
                scan.active_subturn_after_last_duration = false;
            }
            continue;
        }

        if is_claude_user_prompt(&row).is_some()
            && let Some(pid) = row_pid
        {
            if pid == start_prompt_id {
                scan.found_start = true;
                if !scan.owned_prompt_ids.iter().any(|seen| seen == &pid) {
                    scan.owned_prompt_ids.push(pid.clone());
                }
                current_subturn_pid = Some(pid);
                scan.active_subturn_after_last_duration = true;
                scan.interrupted_by_user = false;
            } else if scan.found_start {
                scan.superseded_by = Some((pid, idx));
                scan.segment_end_row = idx;
                return scan;
            } else {
                scan.prior_prompt_ids.insert(pid);
            }
            continue;
        }

        if !scan.found_start {
            continue;
        }

        if is_claude_user_queued_interjection(&row) {
            scan.user_interjected_queued = true;
        }

        if assistant_row_has_text(&row) {
            scan.has_assistant_reply = true;
            scan.last_reply_end_row = Some(idx + 1);
            scan.last_reply_prompt_id = current_subturn_pid.clone();
            scan.interrupted_by_user = false;
        }

        if row["type"].as_str() == Some("system")
            && row["subtype"].as_str() == Some("turn_duration")
        {
            let resolved_pid = resolve_parent_prompt_id(&nodes, row["parentUuid"].as_str())
                .or_else(|| current_subturn_pid.clone());
            scan.turn_durations.push(ClaudeTurnDuration {
                row_index: idx,
                prompt_id: resolved_pid.clone(),
            });
            scan.active_subturn_after_last_duration = false;
            scan.interrupted_by_user = false;
            if scan.has_assistant_reply {
                scan.last_reply_end_row = Some(idx + 1);
                scan.last_reply_prompt_id = resolved_pid.or_else(|| current_subturn_pid.clone());
            }
        }
    }

    scan.segment_end_row = row_idx;
    scan
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClaudeTurnState {
    Unknown,
    Active,
    Completed,
    Superseded(String),
}

pub(super) fn claude_turn_state(path: &Path, prompt_id: &str) -> ClaudeTurnState {
    let scan = claude_dispatch_scan(path, prompt_id);
    if !scan.found_start {
        ClaudeTurnState::Unknown
    } else if let Some((next_pid, _)) = scan.superseded_by {
        ClaudeTurnState::Superseded(next_pid)
    } else if !scan.turn_durations.is_empty() && !scan.active_subturn_after_last_duration {
        ClaudeTurnState::Completed
    } else {
        ClaudeTurnState::Active
    }
}

#[cfg(unix)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CodexRolloutTurn {
    Unknown,
    Started,
    Completed,
    Interrupted,
    Failed(Option<String>),
}

#[cfg(unix)]
pub(super) fn codex_homes() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    dirs::home_dir()
        .map(|h| vec![h.join(".codex")])
        .unwrap_or_default()
}

#[cfg(unix)]
pub(super) fn codex_rollout_path_for_thread(homes: &[PathBuf], thread_id: &str) -> Option<PathBuf> {
    if thread_id.is_empty() || thread_id.contains(['/', '\\']) {
        return None;
    }
    let suffix = format!("-{thread_id}.jsonl");
    let mut stack: Vec<PathBuf> = homes.iter().map(|h| h.join("sessions")).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
            {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(unix)]
pub(super) fn codex_rollout_turn_state(path: &Path, turn_id: &str) -> CodexRolloutTurn {
    let Ok(file) = File::open(path) else {
        return CodexRolloutTurn::Unknown;
    };
    let reader = BufReader::new(file);
    let mut state = CodexRolloutTurn::Unknown;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if row["type"].as_str() != Some("event_msg") {
            continue;
        }
        let payload = &row["payload"];
        if payload["turn_id"].as_str() != Some(turn_id) {
            continue;
        }
        match payload["type"].as_str() {
            Some("task_started") => {
                if state == CodexRolloutTurn::Unknown {
                    state = CodexRolloutTurn::Started;
                }
            }
            Some("task_complete") => {
                state = CodexRolloutTurn::Completed;
            }
            Some("turn_aborted") => {
                let reason = payload["reason"].as_str().unwrap_or_default();
                state = if reason == "interrupted" {
                    CodexRolloutTurn::Interrupted
                } else {
                    CodexRolloutTurn::Failed((!reason.is_empty()).then(|| reason.to_owned()))
                };
            }
            _ => {}
        }
    }
    state
}

pub(super) fn agy_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("AGENTNAVE_AGY_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    dirs::home_dir().map(|h| h.join(".gemini/antigravity-cli"))
}

pub(super) fn agy_transcript_path(agy_home: &Path, cid: &str) -> PathBuf {
    agy_home
        .join("brain")
        .join(cid)
        .join(".system_generated/logs/transcript_full.jsonl")
}

/// Extract distinct conversation UUIDs announced via `Created conversation <uuid>` in `log_path`.
pub(super) fn agy_log_conversation_ids(log_path: &Path) -> Vec<String> {
    let Ok(meta) = fs::metadata(log_path) else {
        return Vec::new();
    };
    if meta.len() > 16 * 1024 * 1024 {
        return Vec::new();
    }
    let Ok(bytes) = fs::read(log_path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    const MARKER: &str = "Created conversation ";
    let mut out = Vec::new();
    let mut search = text.as_ref();
    while let Some(pos) = search.find(MARKER) {
        let rest = &search[pos + MARKER.len()..];
        if let Some(cand) = rest.get(..36)
            && valid_uuid(cand)
            && !out.iter().any(|seen| seen == cand)
        {
            out.push(cand.to_owned());
        }
        search = rest;
    }
    out
}

fn unwrap_agy_user_request(content: &str) -> &str {
    if let (Some(start), Some(end)) = (
        content.find("<USER_REQUEST>"),
        content.rfind("</USER_REQUEST>"),
    ) && end > start + "<USER_REQUEST>".len()
    {
        return &content[start + "<USER_REQUEST>".len()..end];
    }
    content
}

pub(super) fn agy_match_user_input(
    path: &Path,
    after_index: Option<i64>,
    expected_prompt: &str,
) -> Option<i64> {
    let file = File::open(path).ok()?;
    let expected = flat_whitespace(expected_prompt);
    if expected.is_empty() {
        return None;
    }
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if row["type"].as_str() != Some("USER_INPUT")
            || row["source"].as_str() != Some("USER_EXPLICIT")
        {
            continue;
        }
        let Some(step_index) = row["step_index"].as_i64() else {
            continue;
        };
        if after_index.is_some_and(|prev| step_index <= prev) {
            continue;
        }
        if let Some(content) = row["content"].as_str()
            && flat_whitespace(unwrap_agy_user_request(content)) == expected
        {
            return Some(step_index);
        }
    }
    None
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct AgyDispatchScan {
    pub found_start: bool,
    pub has_final_reply: bool,
    pub latest_planner_is_final: bool,
    pub last_reply_end_row: Option<usize>,
    pub segment_end_row: usize,
    pub superseded_by: Option<(i64, usize)>,
}

pub(super) fn agy_dispatch_scan(path: &Path, user_input_index: i64) -> AgyDispatchScan {
    let mut scan = AgyDispatchScan::default();
    let Ok(file) = File::open(path) else {
        return scan;
    };
    let reader = BufReader::new(file);
    let mut row_idx = 0usize;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if !row.is_object() {
            continue;
        }
        let idx = row_idx;
        row_idx += 1;

        if row["type"].as_str() == Some("USER_INPUT")
            && row["source"].as_str() == Some("USER_EXPLICIT")
        {
            let Some(step_index) = row["step_index"].as_i64() else {
                continue;
            };
            if step_index == user_input_index {
                scan.found_start = true;
                scan.has_final_reply = false;
                scan.latest_planner_is_final = false;
                scan.last_reply_end_row = None;
            } else if scan.found_start && step_index > user_input_index {
                scan.superseded_by = Some((step_index, idx));
                scan.segment_end_row = idx;
                return scan;
            }
            continue;
        }
        if !scan.found_start {
            continue;
        }
        if row["type"].as_str() == Some("PLANNER_RESPONSE") {
            let has_calls = row["tool_calls"]
                .as_array()
                .is_some_and(|arr| !arr.is_empty());
            let has_text = row["content"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty());
            let is_final = !has_calls && has_text;
            scan.latest_planner_is_final = is_final;
            if is_final {
                scan.has_final_reply = true;
                scan.last_reply_end_row = Some(idx + 1);
            }
        }
    }
    scan.segment_end_row = row_idx;
    scan
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AgyTurnState {
    Waiting,
    HasFinalPlannerResponse,
    Superseded(i64),
}

pub(super) fn agy_turn_state(path: &Path, user_input_index: i64) -> AgyTurnState {
    let scan = agy_dispatch_scan(path, user_input_index);
    if let Some((next_idx, _)) = scan.superseded_by {
        AgyTurnState::Superseded(next_idx)
    } else if scan.has_final_reply {
        AgyTurnState::HasFinalPlannerResponse
    } else {
        AgyTurnState::Waiting
    }
}

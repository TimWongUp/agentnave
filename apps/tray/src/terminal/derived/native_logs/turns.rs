//! Read-only full-turn extraction and caching for derived session native logs (ADR-0128).

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::{Value, json};

use super::{
    TranscriptLine, agy_home, agy_transcript_path, claude_homes, claude_transcript_path,
    codex_homes, codex_rollout_path_for_thread, flat_whitespace, is_claude_interrupt_row,
    is_claude_task_notification, is_safe_path_under_root, open_regular_readonly, read_bounded_line,
    unwrap_claude_pasted,
};
use crate::data_files::valid_uuid;

const MAX_CACHE_ENTRIES: usize = 64;
const FORWARDED_CONTEXT_MARKER: &str = "With that context, here's what I need:";
/// What the service puts between its instructions and the task (`core.TASK_SEPARATOR`), flattened.
const TASK_SEPARATOR: &str = "Here's the task:";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(in crate::terminal) struct NativeTurn {
    pub index: usize,
    pub anchor: Value,
    pub user_text: String,
    pub user_time: Option<String>,
    pub assistant_text: Option<String>,
    pub assistant_time: Option<String>,
    pub status: String,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal) struct TaskMatch {
    pub prompt: String,
    pub native_anchor: Option<Value>,
    pub has_forward: bool,
}

#[derive(Clone)]
struct CachedTurns {
    len: u64,
    modified: Option<SystemTime>,
    result: Result<Vec<NativeTurn>, String>,
}

static TURNS_CACHE: LazyLock<Mutex<HashMap<(String, PathBuf), CachedTurns>>> =
    LazyLock::new(Mutex::default);

/// Complete JSON-object rows and whether the file held any non-blank line; lines that are not
/// JSON objects are skipped, and an unfinished trailing line is left for the next read.
fn read_rows(path: &Path) -> Result<(Vec<Value>, bool), String> {
    let file = open_regular_readonly(path)
        .ok_or_else(|| "找不到原生会话日志（可能已被 CLI 清理）".to_owned())?;
    let mut reader = BufReader::new(file);
    let mut rows = Vec::new();
    let mut has_non_empty = false;
    let mut line = String::new();
    loop {
        match read_bounded_line(&mut reader, &mut line) {
            TranscriptLine::Eof => break,
            TranscriptLine::Oversized => has_non_empty = true,
            TranscriptLine::Line => {
                if !line.ends_with('\n') {
                    break;
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                has_non_empty = true;
                if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
                    rows.push(value);
                }
            }
        }
    }
    Ok((rows, has_non_empty))
}

fn non_empty_str(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn is_claude_slash_or_local_command(text: &str) -> bool {
    let trimmed = text.trim_start();
    [
        "<command-name>",
        "<command-message>",
        "<command-args>",
        "<local-command-stdout>",
        "<local-command-stderr>",
        "<local-command-caveat>",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
}

fn claude_human_prompt(row: &Value) -> Option<String> {
    if row["type"].as_str() != Some("user")
        || row["isSidechain"].as_bool() == Some(true)
        || row["isMeta"].as_bool() == Some(true)
        || row["isCompactSummary"].as_bool() == Some(true)
        || row["isVisibleInTranscriptOnly"].as_bool() == Some(true)
        || is_claude_task_notification(row)
        || is_claude_interrupt_row(row)
    {
        return None;
    }
    let content = &row["message"]["content"];
    let raw = if let Some(text) = content.as_str() {
        Some(text.to_owned())
    } else if let Some(blocks) = content.as_array() {
        let mut parts = Vec::new();
        let mut has_prompt_block = false;
        for block in blocks {
            if block["type"].as_str() != Some("tool_result") {
                has_prompt_block = true;
                if let Some(text) = block["text"].as_str() {
                    parts.push(text);
                }
            }
        }
        has_prompt_block.then(|| parts.join("\n"))
    } else {
        None
    }?;
    if is_claude_slash_or_local_command(&raw) {
        return None;
    }
    let unwrapped = unwrap_claude_pasted(&raw).trim().to_owned();
    (!unwrapped.is_empty()).then_some(unwrapped)
}

fn claude_assistant_text(row: &Value) -> Option<String> {
    let blocks = row["message"]["content"].as_array()?;
    let mut text = String::new();
    for block in blocks {
        if block["type"].as_str() == Some("text")
            && let Some(part) = block["text"].as_str()
        {
            text.push_str(part);
        }
    }
    (!text.trim().is_empty()).then_some(text)
}

fn is_recognized_claude_row(row: &Value) -> bool {
    matches!(
        row["type"].as_str(),
        Some(
            "user"
                | "assistant"
                | "system"
                | "attachment"
                | "queue-operation"
                | "custom-title"
                | "ai-title"
                | "last-prompt"
                | "file-history-snapshot"
                | "api-request"
                | "api-request-blob"
                | "api-request-shape"
                | "atis-latch"
                | "data-flow-records"
                | "session-owner"
                | "tools-used"
                | "progress"
                | "summary"
        )
    )
}

struct ClaudeTurnAcc {
    anchor: Value,
    user_text: String,
    user_time: Option<String>,
    replies: Vec<String>,
    current_reply: String,
    assistant_time: Option<String>,
    model: Option<String>,
    interrupted: bool,
}

impl ClaudeTurnAcc {
    fn finish(mut self, index: usize) -> NativeTurn {
        if !self.current_reply.is_empty() {
            self.replies.push(self.current_reply);
        }
        let assistant_text = (!self.replies.is_empty()).then(|| self.replies.join("\n\n"));
        let status = if assistant_text.is_some() {
            "succeeded"
        } else if self.interrupted {
            "interrupted"
        } else {
            "running"
        };
        NativeTurn {
            index,
            anchor: self.anchor,
            user_text: self.user_text,
            user_time: self.user_time,
            assistant_text,
            assistant_time: self.assistant_time,
            status: status.to_owned(),
            model: self.model,
        }
    }
}

pub(super) fn parse_claude_turns(
    rows: &[Value],
    has_non_empty: bool,
) -> Result<Vec<NativeTurn>, String> {
    if has_non_empty && !rows.iter().any(is_recognized_claude_row) {
        return Err("原生会话日志格式无法识别".to_owned());
    }
    let mut turns = Vec::new();
    let mut current: Option<ClaudeTurnAcc> = None;
    for row in rows {
        if row["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let kind = row["type"].as_str();
        if kind == Some("system") && row["subtype"].as_str() == Some("turn_duration") {
            if let Some(acc) = current.as_mut() {
                if !acc.current_reply.is_empty() {
                    acc.replies.push(std::mem::take(&mut acc.current_reply));
                    if let Some(ts) = non_empty_str(&row["timestamp"]) {
                        acc.assistant_time = Some(ts);
                    }
                } else if acc.assistant_time.is_none() {
                    acc.assistant_time = non_empty_str(&row["timestamp"]);
                }
            }
            continue;
        }
        if is_claude_task_notification(row) {
            if let Some(acc) = current.as_mut() {
                if !acc.current_reply.is_empty() {
                    acc.replies.push(std::mem::take(&mut acc.current_reply));
                }
                acc.interrupted = false;
            }
            continue;
        }
        if is_claude_interrupt_row(row) {
            if let Some(acc) = current.as_mut() {
                acc.interrupted = true;
            }
            continue;
        }
        if let Some(user_text) = claude_human_prompt(row) {
            if let Some(prev) = current.take() {
                let idx = turns.len();
                turns.push(prev.finish(idx));
            }
            let anchor = match non_empty_str(&row["promptId"]) {
                Some(pid) => json!({"prompt_id": pid}),
                None => json!({}),
            };
            current = Some(ClaudeTurnAcc {
                anchor,
                user_text,
                user_time: non_empty_str(&row["timestamp"]),
                replies: Vec::new(),
                current_reply: String::new(),
                assistant_time: None,
                model: None,
                interrupted: false,
            });
            continue;
        }
        if kind == Some("assistant")
            && let Some(acc) = current.as_mut()
            && let Some(text) = claude_assistant_text(row)
        {
            acc.current_reply = text;
            acc.interrupted = false;
            if let Some(ts) = non_empty_str(&row["timestamp"]) {
                acc.assistant_time = Some(ts);
            }
            if let Some(model) = non_empty_str(&row["message"]["model"])
                && model != "<synthetic>"
            {
                acc.model = Some(model);
            }
        }
    }
    if let Some(last) = current {
        let idx = turns.len();
        turns.push(last.finish(idx));
    }
    Ok(turns)
}

#[derive(Default)]
struct CodexTurnAcc {
    turn_id: String,
    client_id: Option<String>,
    user_text: Option<String>,
    user_time: Option<String>,
    started_time: Option<String>,
    assistant_text: Option<String>,
    fallback_reply: Option<String>,
    assistant_time: Option<String>,
    status: Option<String>,
    model: Option<String>,
}

fn codex_user_item_text(item: &Value) -> Option<String> {
    let content = &item["content"];
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    let blocks = content.as_array()?;
    let mut parts = Vec::new();
    for block in blocks {
        if matches!(block["type"].as_str(), Some("text" | "input_text") | None)
            && let Some(text) = block["text"].as_str()
        {
            parts.push(text);
        }
    }
    let joined = parts.join("\n");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn is_recognized_codex_row(row: &Value) -> bool {
    matches!(
        row["type"].as_str(),
        Some(
            "session_meta"
                | "turn_context"
                | "event_msg"
                | "response_item"
                | "token_usage_record"
                | "world_state"
                | "compacted"
        )
    )
}

fn apply_codex_event(acc: &mut CodexTurnAcc, payload: &Value, ts: Option<String>) {
    match payload["type"].as_str() {
        Some("task_started") => {
            if acc.started_time.is_none() {
                acc.started_time = ts;
            }
        }
        Some("item_completed") => {
            let item = &payload["item"];
            if item["type"].as_str() == Some("UserMessage") {
                if let Some(text) = codex_user_item_text(item) {
                    acc.user_text = Some(text);
                }
                acc.client_id = non_empty_str(&item["client_id"]);
                acc.user_time = ts.or_else(|| acc.started_time.clone());
            } else if item["type"].as_str() == Some("AgentMessage")
                && let Some(text) = codex_user_item_text(item)
            {
                acc.fallback_reply = Some(text);
                if acc.assistant_time.is_none() {
                    acc.assistant_time = ts;
                }
            }
        }
        Some("user_message") => {
            if acc.user_text.is_none()
                && let Some(text) = non_empty_str(&payload["message"])
            {
                acc.user_text = Some(text);
                acc.user_time = ts.or_else(|| acc.started_time.clone());
            }
        }
        Some("agent_message") => {
            if let Some(text) = non_empty_str(&payload["message"]) {
                acc.fallback_reply = Some(text);
                if acc.assistant_time.is_none() {
                    acc.assistant_time = ts;
                }
            }
        }
        Some("task_complete") => {
            let reply = non_empty_str(&payload["last_agent_message"])
                .or_else(|| acc.fallback_reply.clone());
            acc.assistant_text = reply;
            if let Some(t) = ts {
                acc.assistant_time = Some(t);
            }
            acc.status = Some("succeeded".to_owned());
        }
        Some("turn_aborted") => {
            let reason = payload["reason"].as_str().unwrap_or_default();
            acc.status = Some(if reason == "interrupted" {
                "interrupted".to_owned()
            } else {
                "failed".to_owned()
            });
            if let Some(t) = ts {
                acc.assistant_time = Some(t);
            }
        }
        _ => {}
    }
}

impl CodexTurnAcc {
    /// The finished turn, or `None` when the log never recorded its user message.
    fn finish(self, index: usize) -> Option<NativeTurn> {
        let user_text = self.user_text?;
        let mut anchor = serde_json::Map::new();
        anchor.insert("turn_id".to_owned(), Value::String(self.turn_id));
        if let Some(cid) = self.client_id {
            anchor.insert("client_user_message_id".to_owned(), Value::String(cid));
        }
        let assistant_text = self.assistant_text.or_else(|| {
            matches!(self.status.as_deref(), Some("succeeded")).then_some(self.fallback_reply)?
        });
        let model = if assistant_text.is_some() {
            self.model
        } else {
            None
        };
        Some(NativeTurn {
            index,
            anchor: Value::Object(anchor),
            user_text,
            user_time: self.user_time.or(self.started_time),
            assistant_text,
            assistant_time: self.assistant_time,
            status: self.status.unwrap_or_else(|| "running".to_owned()),
            model,
        })
    }
}

pub(super) fn parse_codex_turns(
    rows: &[Value],
    has_non_empty: bool,
) -> Result<Vec<NativeTurn>, String> {
    if has_non_empty && !rows.iter().any(is_recognized_codex_row) {
        return Err("原生会话日志格式无法识别".to_owned());
    }
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, CodexTurnAcc> = HashMap::new();
    for row in rows {
        let row_type = row["type"].as_str();
        if !matches!(row_type, Some("turn_context" | "event_msg")) {
            continue;
        }
        let payload = &row["payload"];
        let Some(tid) = non_empty_str(&payload["turn_id"]) else {
            continue;
        };
        let acc = by_id.entry(tid.clone()).or_insert_with(|| {
            order.push(tid.clone());
            CodexTurnAcc {
                turn_id: tid,
                ..CodexTurnAcc::default()
            }
        });
        if row_type == Some("turn_context") {
            if let Some(model) = non_empty_str(&payload["model"]) {
                acc.model = Some(model);
            }
        } else {
            apply_codex_event(acc, payload, non_empty_str(&row["timestamp"]));
        }
    }
    let mut turns = Vec::new();
    for tid in order {
        if let Some(acc) = by_id.remove(&tid)
            && let Some(turn) = acc.finish(turns.len())
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}

struct AgyTurnAcc {
    step_index: i64,
    user_text: String,
    user_time: Option<String>,
    replies: Vec<String>,
    current_reply: String,
    assistant_time: Option<String>,
    model: Option<String>,
}

impl AgyTurnAcc {
    fn finish(mut self, index: usize) -> NativeTurn {
        if !self.current_reply.is_empty() {
            self.replies.push(self.current_reply);
        }
        let assistant_text = (!self.replies.is_empty()).then(|| self.replies.join("\n\n"));
        let status = if assistant_text.is_some() {
            "succeeded"
        } else {
            "running"
        };
        NativeTurn {
            index,
            anchor: json!({"user_input_index": self.step_index}),
            user_text: self.user_text,
            user_time: self.user_time,
            assistant_text,
            assistant_time: self.assistant_time,
            status: status.to_owned(),
            model: self.model,
        }
    }
}

fn unwrap_agy_user_text(content: &str) -> String {
    super::unwrap_agy_user_request(content).trim().to_owned()
}

fn is_recognized_agy_row(row: &Value) -> bool {
    matches!(
        row["type"].as_str(),
        Some(
            "USER_INPUT"
                | "PLANNER_RESPONSE"
                | "GENERIC"
                | "SYSTEM_MESSAGE"
                | "TOOL_CALL"
                | "TOOL_RESULT"
        )
    )
}

pub(super) fn parse_antigravity_turns(
    rows: &[Value],
    has_non_empty: bool,
) -> Result<Vec<NativeTurn>, String> {
    if has_non_empty && !rows.iter().any(is_recognized_agy_row) {
        return Err("原生会话日志格式无法识别".to_owned());
    }
    let mut turns = Vec::new();
    let mut current: Option<AgyTurnAcc> = None;
    for row in rows {
        match row["type"].as_str() {
            Some("USER_INPUT") if row["source"].as_str() == Some("USER_EXPLICIT") => {
                let Some(step_index) = row["step_index"].as_i64() else {
                    continue;
                };
                let user_text = row["content"]
                    .as_str()
                    .map(unwrap_agy_user_text)
                    .unwrap_or_default();
                if user_text.is_empty() {
                    continue;
                }
                if let Some(prev) = current.take() {
                    let idx = turns.len();
                    turns.push(prev.finish(idx));
                }
                current = Some(AgyTurnAcc {
                    step_index,
                    user_text,
                    user_time: non_empty_str(&row["created_at"]),
                    replies: Vec::new(),
                    current_reply: String::new(),
                    assistant_time: None,
                    model: None,
                });
            }
            Some("USER_INPUT") => {
                if let Some(acc) = current.as_mut()
                    && !acc.current_reply.is_empty()
                {
                    acc.replies.push(std::mem::take(&mut acc.current_reply));
                }
            }
            Some("PLANNER_RESPONSE") => {
                if let Some(acc) = current.as_mut() {
                    let has_calls = row["tool_calls"]
                        .as_array()
                        .is_some_and(|arr| !arr.is_empty());
                    if has_calls {
                        if !acc.current_reply.is_empty() {
                            acc.replies.push(std::mem::take(&mut acc.current_reply));
                        }
                    } else if let Some(text) = row["content"].as_str()
                        && !text.trim().is_empty()
                    {
                        acc.current_reply = text.to_owned();
                        if let Some(ts) = non_empty_str(&row["created_at"]) {
                            acc.assistant_time = Some(ts);
                        }
                        if let Some(model) = non_empty_str(&row["model"]) {
                            acc.model = Some(model);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(last) = current {
        let idx = turns.len();
        turns.push(last.finish(idx));
    }
    Ok(turns)
}

pub(super) fn read_native_turns(provider: &str, path: &Path) -> Result<Vec<NativeTurn>, String> {
    let file = open_regular_readonly(path)
        .ok_or_else(|| "找不到原生会话日志（可能已被 CLI 清理）".to_owned())?;
    let meta = file
        .metadata()
        .map_err(|_| "找不到原生会话日志（可能已被 CLI 清理）".to_owned())?;
    let key = (provider.to_owned(), path.to_path_buf());
    let len = meta.len();
    let modified = meta.modified().ok();
    drop(file);
    {
        let cache = TURNS_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = cache.get(&key)
            && entry.len == len
            && entry.modified == modified
        {
            return entry.result.clone();
        }
    }
    let result = match provider_log(provider) {
        Some(spec) => {
            read_rows(path).and_then(|(rows, has_non_empty)| (spec.parse)(&rows, has_non_empty))
        }
        None => Err("该 CLI 不支持读取原生会话日志".to_owned()),
    };
    let mut cache = TURNS_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(
        key,
        CachedTurns {
            len,
            modified,
            result: result.clone(),
        },
    );
    result
}

fn claude_anchor_matches(expected: &Value, actual: &Value) -> bool {
    expected["prompt_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .is_some_and(|pid| actual["prompt_id"].as_str() == Some(pid))
}

fn codex_anchor_matches(expected: &Value, actual: &Value) -> bool {
    expected["turn_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .is_some_and(|tid| {
            actual["turn_id"].as_str() == Some(tid)
                && expected["client_user_message_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .is_none_or(|cid| {
                        actual["client_user_message_id"]
                            .as_str()
                            .is_none_or(|actual_cid| actual_cid == cid)
                    })
        })
}

fn antigravity_anchor_matches(expected: &Value, actual: &Value) -> bool {
    expected["user_input_index"]
        .as_i64()
        .is_some_and(|idx| actual["user_input_index"].as_i64() == Some(idx))
}

/// What differs per CLI when reading its native log.
struct ProviderLog {
    parse: fn(&[Value], bool) -> Result<Vec<NativeTurn>, String>,
    anchor_matches: fn(&Value, &Value) -> bool,
}

fn provider_log(provider: &str) -> Option<ProviderLog> {
    Some(match provider {
        "claude" => ProviderLog {
            parse: parse_claude_turns,
            anchor_matches: claude_anchor_matches,
        },
        "codex" => ProviderLog {
            parse: parse_codex_turns,
            anchor_matches: codex_anchor_matches,
        },
        "antigravity" => ProviderLog {
            parse: parse_antigravity_turns,
            anchor_matches: antigravity_anchor_matches,
        },
        _ => return None,
    })
}

fn prompt_matches(turn_text: &str, task: &TaskMatch) -> bool {
    let expected = flat_whitespace(&task.prompt);
    if expected.is_empty() {
        return false;
    }
    // The terminal received the service's instructions, then the separator, then the task.
    let is_task = |text: &str| {
        let flat = flat_whitespace(text);
        flat == expected || flat.ends_with(&format!("{TASK_SEPARATOR} {expected}"))
    };
    if is_task(turn_text) {
        return true;
    }
    if task.has_forward
        && let Some((_, tail)) = turn_text.split_once(FORWARDED_CONTEXT_MARKER)
    {
        return flat_whitespace(tail) == expected;
    }
    false
}

pub(super) fn filter_user_turns(
    turns: &[NativeTurn],
    provider: &str,
    tasks: &[TaskMatch],
) -> Vec<NativeTurn> {
    let anchor_matches = provider_log(provider).map(|spec| spec.anchor_matches);
    let mut claimed = vec![false; turns.len()];
    let mut unmatched_tasks = Vec::new();
    for task in tasks {
        let found =
            task.native_anchor
                .as_ref()
                .zip(anchor_matches)
                .and_then(|(anchor, matches)| {
                    turns
                        .iter()
                        .enumerate()
                        .position(|(i, turn)| !claimed[i] && matches(anchor, &turn.anchor))
                });
        match found {
            Some(pos) => claimed[pos] = true,
            // An anchor that names no turn (e.g. only the session was known) falls back to the text.
            None => unmatched_tasks.push(task),
        }
    }
    for task in unmatched_tasks {
        if let Some(pos) = turns
            .iter()
            .enumerate()
            .position(|(i, turn)| !claimed[i] && prompt_matches(&turn.user_text, task))
        {
            claimed[pos] = true;
        }
    }
    turns
        .iter()
        .enumerate()
        .filter(|(i, _)| !claimed[*i])
        .map(|(_, turn)| turn.clone())
        .collect()
}

fn codex_archived_path(homes: &[PathBuf], thread_id: &str) -> Option<PathBuf> {
    if thread_id.is_empty() || thread_id.contains(['/', '\\']) {
        return None;
    }
    let suffix = format!("-{thread_id}.jsonl");
    for home in homes {
        let dir = home.join("archived_sessions");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
                && is_safe_path_under_root(&path, &dir)
                && open_regular_readonly(&path).is_some()
            {
                return Some(path);
            }
        }
    }
    None
}

pub(in crate::terminal) fn find_transcript_path(
    provider: &str,
    account_dir: Option<&Path>,
    session_id: &str,
) -> Option<PathBuf> {
    if !valid_uuid(session_id) {
        return None;
    }
    match provider {
        "claude" => claude_transcript_path(&claude_homes(account_dir), session_id),
        "codex" => {
            let homes = codex_homes();
            codex_rollout_path_for_thread(&homes, session_id)
                .filter(|path| {
                    homes
                        .iter()
                        .any(|home| is_safe_path_under_root(path, &home.join("sessions")))
                })
                .or_else(|| codex_archived_path(&homes, session_id))
        }
        "antigravity" => {
            let home = agy_home()?;
            let root = home.join("brain");
            let path = agy_transcript_path(&home, session_id);
            (is_safe_path_under_root(&path, &root) && open_regular_readonly(&path).is_some())
                .then_some(path)
        }
        _ => None,
    }
}

pub(in crate::terminal) fn read_user_turns_at_path(
    provider: &str,
    path: &Path,
    tasks: &[TaskMatch],
) -> Result<Vec<NativeTurn>, String> {
    let turns = read_native_turns(provider, path)?;
    Ok(filter_user_turns(&turns, provider, tasks))
}

//! Pure read-only native log/transcript/rollout inspectors for derived terminal turns (ADR-0109).

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::data_files::valid_uuid;

pub(super) fn flat_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_claude_user_prompt(row: &Value) -> Option<String> {
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
pub(super) fn claude_prompt_anchor(
    path: &Path,
    offset: u64,
    expected_prompt: &str,
) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let _ = file.seek(SeekFrom::Start(offset));
    let reader = BufReader::new(file);
    let expected = flat_whitespace(expected_prompt);
    let mut first_prompt_id = None;
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
        if first_prompt_id.is_none() {
            first_prompt_id = Some(pid.clone());
        }
        if !expected.is_empty() && flat_whitespace(&text) == expected {
            return Some(pid);
        }
    }
    first_prompt_id
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClaudeTurnState {
    Unknown,
    Active,
    Completed,
    Superseded(String),
}

pub(super) fn claude_turn_state(path: &Path, prompt_id: &str) -> ClaudeTurnState {
    let Ok(file) = File::open(path) else {
        return ClaudeTurnState::Unknown;
    };
    let reader = BufReader::new(file);
    let mut saw_ours = false;
    let mut completed = false;
    for line in reader.lines().map_while(Result::ok) {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(pid) =
            is_claude_user_prompt(&row).and_then(|_| row["promptId"].as_str().map(str::to_owned))
            && !pid.is_empty()
        {
            if pid == prompt_id {
                saw_ours = true;
                completed = false;
            } else if saw_ours {
                return ClaudeTurnState::Superseded(pid);
            }
        }
        if saw_ours
            && row["isSidechain"].as_bool() != Some(true)
            && row["type"].as_str() == Some("system")
            && row["subtype"].as_str() == Some("turn_duration")
        {
            completed = true;
        }
    }
    if !saw_ours {
        ClaudeTurnState::Unknown
    } else if completed {
        ClaudeTurnState::Completed
    } else {
        ClaudeTurnState::Active
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CodexRolloutTurn {
    Unknown,
    Started,
    Completed,
    Interrupted,
    Failed(Option<String>),
}

pub(super) fn codex_homes() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    dirs::home_dir()
        .map(|h| vec![h.join(".codex")])
        .unwrap_or_default()
}

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
    for (line_idx, line) in reader.lines().map_while(Result::ok).enumerate() {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if row["type"].as_str() != Some("USER_INPUT")
            || row["source"].as_str() != Some("USER_EXPLICIT")
        {
            continue;
        }
        let step_index = row["step_index"]
            .as_i64()
            .unwrap_or_else(|| i64::try_from(line_idx).unwrap_or(0));
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

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AgyTurnState {
    Waiting,
    HasFinalPlannerResponse,
    Superseded(i64),
}

pub(super) fn agy_turn_state(path: &Path, user_input_index: i64) -> AgyTurnState {
    let Ok(file) = File::open(path) else {
        return AgyTurnState::Waiting;
    };
    let reader = BufReader::new(file);
    let mut saw_ours = false;
    let mut answered = false;
    for (line_idx, line) in reader.lines().map_while(Result::ok).enumerate() {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let step_index = row["step_index"]
            .as_i64()
            .unwrap_or_else(|| i64::try_from(line_idx).unwrap_or(0));
        if row["type"].as_str() == Some("USER_INPUT")
            && row["source"].as_str() == Some("USER_EXPLICIT")
        {
            if step_index == user_input_index {
                saw_ours = true;
                answered = false;
            } else if saw_ours && step_index > user_input_index {
                return AgyTurnState::Superseded(step_index);
            }
            continue;
        }
        if !saw_ours {
            continue;
        }
        if row["type"].as_str() == Some("PLANNER_RESPONSE") {
            let has_calls = row["tool_calls"]
                .as_array()
                .is_some_and(|arr| !arr.is_empty());
            let has_text = row["content"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty());
            answered = !has_calls && has_text;
        }
    }
    if answered {
        AgyTurnState::HasFinalPlannerResponse
    } else {
        AgyTurnState::Waiting
    }
}

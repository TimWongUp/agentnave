//! Chat names of the other CLIs, read where magpie reads them: CodeBuddy and WorkBuddy write the
//! same `custom-title` and `ai-title` entries as Claude Code, Grok keeps `generated_title` in a
//! session's `summary.json`, Pi's `/name` is a `session_info` entry, and DeepSeek logs a
//! `session/title` event. Devin and ZCode keep sessions in a shared database or a binary format,
//! so their chats have no name here; Antigravity CLI (`agy`) is read by `super::antigravity`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::title::{self, CliTitle, cached};
use super::{file_name, session_cwd};

/// The hosts whose chat names can be read.
pub(super) const HOSTS: [&str; 5] = ["codebuddy", "workbuddy", "grok", "pi", "deepseek"];

/// How much of a DeepSeek session is decoded when looking for its title, which it logs early.
const DECODED: u64 = 4 << 20;
/// A new chat's record may be created a moment before the terminal's start time is taken.
const CLOCK_SLACK: Duration = Duration::from_secs(2);

/// The chat's name in its CLI, from the session record the workspace saved.
pub(super) fn title_of(host: &str, record: &Path) -> Option<CliTitle> {
    match host {
        "codebuddy" | "workbuddy" => title::claude(record),
        "grok" => grok(&record.parent()?.join("summary.json")),
        "pi" => title::pi(record),
        "deepseek" => deepseek(record),
        _ => None,
    }
}

/// Grok's own title for a session, else the summary it keeps of it.
pub(super) fn grok(summary: &Path) -> Option<CliTitle> {
    cached(summary, |summary, _| {
        let bytes = fs::read(summary)?;
        let title = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|summary| {
                ["generated_title", "session_summary"]
                    .iter()
                    .filter_map(|key| summary[key].as_str())
                    .map(str::trim)
                    .find(|text| !text.is_empty())
                    .map(|text| CliTitle {
                        text: text.to_owned(),
                        custom: false,
                    })
            });
        Ok(title)
    })
}

fn deepseek(record: &Path) -> Option<CliTitle> {
    cached(record, |record, _| {
        let file = fs::File::open(record)?;
        let mut text = Vec::new();
        // A record that fails midway still yields what was decoded before.
        if file_name(record).ends_with(".zstd") || file_name(record).ends_with(".zst") {
            let mut decoder =
                ruzstd::decoding::StreamingDecoder::new(file).map_err(std::io::Error::other)?;
            let _ = (&mut decoder).take(DECODED).read_to_end(&mut text);
        } else {
            file.take(DECODED).read_to_end(&mut text)?;
        }
        Ok(deepseek_entries(&text))
    })
}

/// The latest `session/title` event among DeepSeek's session lines.
fn deepseek_entries(text: &[u8]) -> Option<CliTitle> {
    let mut named = None;
    for line in text.split(|&byte| byte == b'\n') {
        if !line.windows(13).any(|window| window == b"session/title") {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if entry["type"] == "session/title"
            && let Some(title) = entry["data"]["title"].as_str().map(str::trim)
            && !title.is_empty()
        {
            named = Some(title.to_owned());
        }
    }
    named.map(|text| CliTitle {
        text,
        custom: false,
    })
}

/// A session record started by a terminal, whose path stands in for the chat's ID.
pub(super) fn started(host: &str, home: &Path, cwd: &Path, since: SystemTime) -> Option<PathBuf> {
    let folder = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let (root, depth) = match host {
        "codebuddy" | "workbuddy" => ("projects", 1),
        "pi" => ("sessions", 1),
        "grok" => ("sessions", 2),
        _ => return None,
    };
    let mut found: Vec<PathBuf> = Vec::new();
    let mut look = |record: &Path| {
        if created_since(record, since) && cwd_of(host, record).is_some_and(|there| there == folder)
        {
            found.push(record.to_owned());
        }
    };
    // A folder gains an entry only when a session begins in it, so older folders are skipped by
    // their own modification time.
    visit(&home.join(root), depth, since, &mut |record| match host {
        "grok" if file_name(record) == "summary.json" => look(record),
        "pi" | "codebuddy" | "workbuddy" if file_name(record).ends_with(".jsonl") => look(record),
        _ => {}
    });
    (found.len() == 1).then(|| found.remove(0))
}

/// Visits the files `depth` folders under `dir`, entering only folders changed since `since`.
fn visit(dir: &Path, depth: usize, since: SystemTime, each: &mut dyn FnMut(&Path)) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if depth == 0 {
            each(&path);
        } else if fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .is_ok_and(|changed| changed + CLOCK_SLACK >= since)
        {
            visit(&path, depth - 1, since, each);
        }
    }
}

fn created_since(record: &Path, since: SystemTime) -> bool {
    fs::metadata(record)
        .and_then(|meta| meta.created().or(meta.modified()))
        .is_ok_and(|at| at + CLOCK_SLACK >= since)
}

/// The folder a session record says it ran in.
pub(super) fn cwd_of(host: &str, record: &Path) -> Option<PathBuf> {
    let cwd = if host == "grok" {
        let summary: Value = serde_json::from_slice(&fs::read(record).ok()?).ok()?;
        PathBuf::from(summary["info"]["cwd"].as_str()?)
    } else {
        session_cwd(record)?
    };
    Some(fs::canonicalize(&cwd).unwrap_or(cwd))
}

/// Where a host keeps its sessions: the variable that moves it, else the default under home.
pub(super) fn home_of(host: &str) -> Option<PathBuf> {
    let (variable, default) = match host {
        "codebuddy" => ("CODEBUDDY_CONFIG_DIR", ".codebuddy"),
        "workbuddy" => ("WORKBUDDY_CONFIG_DIR", ".workbuddy"),
        "grok" => ("GROK_HOME", ".grok"),
        "pi" => ("PI_CODING_AGENT_DIR", ".pi/agent"),
        "deepseek" => ("DSH_HOME", ".dsh"),
        _ => return None,
    };
    match std::env::var_os(variable) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(default)),
    }
}

//! Read-only PR link extraction from native CLI session logs (ADR-0115).
//!
//! Reuses the tray's existing native session location helpers (`data::boss_app`) and scans each
//! file strictly read-only with bounded memory to find `github.com/<owner>/<repo>/pull/<n>` links.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::cache::Cache;
use super::gh::extract_github_pr_links;
use crate::data::boss_app::Homes;

/// Maximum bytes scanned from a plain-text session log (head + tail window when larger).
const MAX_PLAIN_BYTES: u64 = 16 * 1024 * 1024;
const HEAD_SLICE_BYTES: u64 = 2 * 1024 * 1024;
const TAIL_SLICE_BYTES: u64 = 14 * 1024 * 1024;
/// Maximum decoded bytes from a zstd-compressed rollout/session file.
const MAX_ZSTD_BYTES: u64 = 16 * 1024 * 1024;
/// Upper bound on distinct PR links collected from one session file.
const MAX_LINKS_PER_FILE: usize = 16;

/// Locates native session log files for an open embedded/derived terminal.
pub(crate) fn terminal_log_files(
    homes: &Homes,
    provider: &str,
    native: Option<&str>,
    account_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let Some(native) = native.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    match provider {
        "claude" => {
            let roots = account_dir
                .map(|dir| vec![dir.to_owned()])
                .unwrap_or_else(|| homes.claude.clone());
            if let Some(path) = crate::data::boss_app::claude_transcript_in(&roots, native) {
                files.push(path);
            }
        }
        "codex" => {
            if let Some(path) = codex_rollout_in(&homes.codex, native) {
                files.push(path);
            }
        }
        "antigravity" => {
            for (_, root) in homes.other.iter().filter(|(h, _)| h == "antigravity") {
                push_agy_transcripts(root, native, &mut files);
            }
        }
        "grok" => {
            for (_, home) in homes.other.iter().filter(|(h, _)| h == "grok") {
                push_grok_logs(home, native, &mut files);
            }
        }
        "codebuddy" | "pi" | "deepseek" => {
            let path = PathBuf::from(native);
            if path.is_file() {
                files.push(path);
            }
        }
        _ => {}
    }
    files
}

/// Locates native session log files for a CLI boss workspace.
pub(crate) fn workspace_log_files(
    homes: &Homes,
    host: &str,
    owner_key: &str,
    owner_file: Option<&Path>,
    account_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let effective_homes = match account_dir {
        Some(dir) if host == "claude" => {
            let mut copy = homes.clone();
            copy.claude = vec![dir.to_owned()];
            copy
        }
        _ => homes.clone(),
    };
    let mut files = Vec::new();
    if let Some(path) = owner_file.filter(|p| p.is_file()) {
        if host == "antigravity" {
            if let Some((id, record)) = crate::data::boss_app::native_session_in(
                &effective_homes,
                host,
                owner_key,
                Some(path),
            ) {
                if let Some(root) = agy_root_from_record(&record) {
                    push_agy_transcripts(&root, &id, &mut files);
                } else if record.is_file() && record.extension().is_some_and(|e| e == "jsonl") {
                    files.push(record);
                }
            }
            return files;
        }
        if host == "grok" {
            if let Some(parent) = path.parent() {
                let history = parent.join("chat_history.jsonl");
                if history.is_file() {
                    files.push(history);
                }
            }
            files.push(path.to_owned());
            return files;
        }
        files.push(path.to_owned());
        return files;
    }

    if let Some((id, record)) =
        crate::data::boss_app::native_session_in(&effective_homes, host, owner_key, None)
    {
        if host == "antigravity" {
            if let Some(root) = agy_root_from_record(&record) {
                push_agy_transcripts(&root, &id, &mut files);
            } else if record.is_file() && record.extension().is_some_and(|e| e == "jsonl") {
                files.push(record);
            }
        } else if host == "grok" {
            if let Some(parent) = record.parent() {
                let history = parent.join("chat_history.jsonl");
                if history.is_file() {
                    files.push(history);
                }
            }
            if record.is_file() {
                files.push(record);
            }
        } else if record.is_file() {
            files.push(record);
        }
    }
    files
}

/// Scans `files` using `cache` so unchanged files are never re-read, returning deduplicated
/// `(owner/repo, pr_number)` pairs with the most recently occurring links first.
pub(crate) fn extract_pr_links_from_files(
    cache: &mut Cache,
    files: &[PathBuf],
) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    for path in files {
        let links = cached_file_pr_links(cache, path);
        for (repo, num) in links {
            let key = (repo.to_ascii_lowercase(), num);
            if seen.insert(key) {
                out.push((repo, num));
                if out.len() >= MAX_LINKS_PER_FILE {
                    return out;
                }
            }
        }
    }
    out
}

pub(crate) fn cached_file_pr_links(cache: &mut Cache, path: &Path) -> Vec<(String, u64)> {
    let Ok(meta) = fs::metadata(path) else {
        return Vec::new();
    };
    if !meta.is_file() {
        return Vec::new();
    }
    let len = meta.len();
    let modified = meta.modified().ok();
    if let Some(hit) = cache.get_file_links(path, len, modified) {
        return hit;
    }
    let links = scan_file_pr_links(path, len);
    cache.put_file_links(path.to_owned(), len, modified, links.clone());
    links
}

/// Reads `path` strictly read-only with bounded memory and extracts deduplicated PR links,
/// most-recently mentioned first.
pub(crate) fn scan_file_pr_links(path: &Path, len: u64) -> Vec<(String, u64)> {
    let Ok(bytes) = read_bounded(path, len) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let raw = extract_github_pr_links(&text);
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    let mut ordered = Vec::new();
    for (repo, num) in raw.into_iter().rev() {
        let key = (repo.to_ascii_lowercase(), num);
        if seen.insert(key) {
            ordered.push((repo, num));
            if ordered.len() >= MAX_LINKS_PER_FILE {
                break;
            }
        }
    }
    ordered
}

fn read_bounded(path: &Path, len: u64) -> std::io::Result<Vec<u8>> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    if name.ends_with(".zst") || name.ends_with(".zstd") {
        let mut decoder =
            ruzstd::decoding::StreamingDecoder::new(file).map_err(std::io::Error::other)?;
        let _ = (&mut decoder).take(MAX_ZSTD_BYTES).read_to_end(&mut buf);
        return Ok(buf);
    }
    if len <= MAX_PLAIN_BYTES {
        file.take(MAX_PLAIN_BYTES).read_to_end(&mut buf)?;
        return Ok(buf);
    }
    // Very large transcript: read head + tail so both early and latest PR links are captured.
    (&mut file).take(HEAD_SLICE_BYTES).read_to_end(&mut buf)?;
    buf.push(b'\n');
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_SLICE_BYTES)))?;
    file.take(TAIL_SLICE_BYTES).read_to_end(&mut buf)?;
    Ok(buf)
}

fn codex_rollout_in(homes: &[PathBuf], thread_id: &str) -> Option<PathBuf> {
    if thread_id.is_empty()
        || thread_id.contains(['/', '\\'])
        || thread_id == "."
        || thread_id == ".."
    {
        return None;
    }
    let suffix_plain = format!("-{thread_id}.jsonl");
    let suffix_zst = format!("-{thread_id}.jsonl.zst");
    for home in homes {
        let mut stack = vec![home.join("sessions"), home.join("archived_sessions")];
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
                    .is_some_and(|name| {
                        name.ends_with(&suffix_plain) || name.ends_with(&suffix_zst)
                    })
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn push_agy_transcripts(root: &Path, cid: &str, out: &mut Vec<PathBuf>) {
    if !crate::data_files::valid_uuid(cid) {
        return;
    }
    let logs = root.join("brain").join(cid).join(".system_generated/logs");
    let full = logs.join("transcript_full.jsonl");
    if full.is_file() {
        out.push(full);
    } else {
        let compact = logs.join("transcript.jsonl");
        if compact.is_file() {
            out.push(compact);
        }
    }
}

fn agy_root_from_record(record: &Path) -> Option<PathBuf> {
    if record.file_name().and_then(|n| n.to_str()) == Some("conversation_summaries.db") {
        return record.parent().map(Path::to_owned);
    }
    for ancestor in record.ancestors() {
        if matches!(
            ancestor.file_name().and_then(|n| n.to_str()),
            Some("brain" | "conversations")
        ) {
            return ancestor.parent().map(Path::to_owned);
        }
    }
    None
}

fn push_grok_logs(home: &Path, id: &str, out: &mut Vec<PathBuf>) {
    if id.is_empty() || id.contains(['/', '\\']) || id == "." || id == ".." {
        return;
    }
    for entry in fs::read_dir(home.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let dir = entry.path().join(id);
        let history = dir.join("chat_history.jsonl");
        let summary = dir.join("summary.json");
        if history.is_file() {
            out.push(history);
        }
        if summary.is_file() {
            out.push(summary);
        }
        if !out.is_empty() {
            break;
        }
    }
}

//! The names the CLIs themselves give a chat, read from their session records and never written
//! back (ADR-0103). Claude Code appends `custom-title` (the user named it) and `ai-title` (it
//! named the chat itself) entries to the transcript; Codex lists each chat's `thread_name` in
//! `session_index.jsonl`. Anything unreadable is simply no title.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use super::{WINDOW, codex_rollout_id, file_name, is_uuid, session_cwd};

/// A chat's name in its CLI, and whether the user chose it there (else the CLI wrote it).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CliTitle {
    pub text: String,
    pub custom: bool,
}

/// A new chat's record may be created a moment before the terminal's start time is taken.
const CLOCK_SLACK: Duration = Duration::from_secs(2);

type ClaudeCache = Mutex<HashMap<PathBuf, (u64, Option<CliTitle>)>>;
static CLAUDE: LazyLock<ClaudeCache> = LazyLock::new(Mutex::default);
/// Where the transcript of a Claude session ID was found, so the folders are not listed again.
static FILES: LazyLock<Mutex<HashMap<String, PathBuf>>> = LazyLock::new(Mutex::default);

/// A Codex session index read whole, with the length and time it was read at.
struct Index {
    len: u64,
    modified: Option<SystemTime>,
    names: Arc<HashMap<String, String>>,
}
static CODEX: LazyLock<Mutex<HashMap<PathBuf, Index>>> = LazyLock::new(Mutex::default);

/// The title in a Claude Code transcript: the user's own when there is one, else Claude's. A
/// transcript repeats its title entries as it grows, so the last 256 KiB hold the latest ones;
/// the first 256 KiB are looked at only when those have none.
pub(crate) fn claude(file: &Path) -> Option<CliTitle> {
    let len = fs::metadata(file).ok()?.len();
    let cached = CLAUDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(file)
        .cloned();
    if let Some((seen, title)) = cached
        && seen == len
    {
        return title;
    }
    let title = claude_unread(file, len).ok()?;
    CLAUDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(file.to_owned(), (len, title.clone()));
    title
}

fn claude_unread(file: &Path, len: u64) -> std::io::Result<Option<CliTitle>> {
    let mut file = fs::File::open(file)?;
    let mut chunk = Vec::new();
    file.seek(SeekFrom::Start(len.saturating_sub(WINDOW)))?;
    (&mut file).take(WINDOW).read_to_end(&mut chunk)?;
    let found = claude_entries(&chunk);
    if found.is_some() || len <= WINDOW {
        return Ok(found);
    }
    chunk.clear();
    file.seek(SeekFrom::Start(0))?;
    file.take(WINDOW).read_to_end(&mut chunk)?;
    Ok(claude_entries(&chunk))
}

/// The latest `custom-title` and `ai-title` entries in a stretch of transcript lines.
fn claude_entries(chunk: &[u8]) -> Option<CliTitle> {
    let (mut custom, mut auto) = (None, None);
    for line in chunk.split(|&byte| byte == b'\n') {
        let mentions = |key: &[u8]| line.windows(key.len()).any(|window| window == key);
        if !(mentions(b"customTitle") || mentions(b"aiTitle")) {
            continue;
        }
        // A window that starts mid-line leaves a fragment that does not parse; it is skipped.
        let Ok(entry) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let text = |key: &str| {
            entry[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        match entry["type"].as_str() {
            Some("custom-title") => custom = text("customTitle").or(custom),
            Some("ai-title") => auto = text("aiTitle").or(auto),
            _ => {}
        }
    }
    custom
        .map(|text| CliTitle { text, custom: true })
        .or(auto.map(|text| CliTitle {
            text,
            custom: false,
        }))
}

/// The thread name Codex lists for a session in any of its homes.
pub(crate) fn codex(homes: &[PathBuf], id: &str) -> Option<CliTitle> {
    homes.iter().find_map(|home| {
        let names = codex_index(&home.join("session_index.jsonl"))?;
        let text = names.get(id)?.clone();
        Some(CliTitle {
            text,
            custom: false,
        })
    })
}

/// Every thread name in an index, the latest line of an id winning; read again only when the
/// file has changed.
fn codex_index(path: &Path) -> Option<Arc<HashMap<String, String>>> {
    let meta = fs::metadata(path).ok()?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    let mut cache = CODEX.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(index) = cache.get(path)
        && index.len == len
        && index.modified == modified
    {
        return Some(index.names.clone());
    }
    let text = fs::read_to_string(path).ok()?;
    let mut names = HashMap::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let (Some(id), Some(name)) = (entry["id"].as_str(), entry["thread_name"].as_str())
            && !name.trim().is_empty()
        {
            names.insert(id.to_owned(), name.trim().to_owned());
        }
    }
    let names = Arc::new(names);
    cache.insert(
        path.to_owned(),
        Index {
            len,
            modified,
            names: names.clone(),
        },
    );
    Some(names)
}

/// The chat of a terminal's CLI and its title there, for a terminal that has no workspace to
/// ask: the session `native` names, else for Codex the only rollout opened in `cwd` since the
/// terminal started. The ID is returned so a found chat is not looked for again.
pub(crate) fn of_terminal(
    homes: &super::Homes,
    provider: &str,
    native: Option<&str>,
    cwd: &Path,
    since: SystemTime,
) -> (Option<String>, Option<CliTitle>) {
    match provider {
        "claude" => {
            let title = native
                .and_then(|id| claude_file(&homes.claude, id))
                .and_then(|file| claude(&file));
            (native.map(str::to_owned), title)
        }
        "codex" => {
            let id = native
                .map(str::to_owned)
                .or_else(|| codex_started(&homes.codex, cwd, since));
            let title = id.as_deref().and_then(|id| codex(&homes.codex, id));
            (id, title)
        }
        _ => (None, None),
    }
}

/// The transcript of a Claude Code session whose ID the terminal chose, in whichever project
/// folder Claude filed it.
pub(crate) fn claude_file(homes: &[PathBuf], id: &str) -> Option<PathBuf> {
    if !is_uuid(id) {
        return None;
    }
    let mut known = FILES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(path) = known.get(id).filter(|path| path.is_file()) {
        return Some(path.clone());
    }
    let name = format!("{id}.jsonl");
    let found = homes.iter().find_map(|home| {
        fs::read_dir(home.join("projects"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|folder| folder.path().join(&name))
            .find(|path| path.is_file())
    })?;
    known.insert(id.to_owned(), found.clone());
    Some(found)
}

/// The ID of the Codex session a terminal started in `cwd` has opened, when exactly one rollout
/// there was created since `since`; two terminals in one folder cannot tell theirs apart.
pub(crate) fn codex_started(homes: &[PathBuf], cwd: &Path, since: SystemTime) -> Option<String> {
    let folder = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let mut found: Vec<String> = Vec::new();
    for home in homes {
        recent_rollouts(&home.join("sessions"), since, &mut |path| {
            let name = file_name(path);
            let created = fs::metadata(path)
                .and_then(|meta| meta.created().or(meta.modified()))
                .ok();
            let id = name
                .strip_suffix(".jsonl")
                .and_then(codex_rollout_id)
                .filter(|_| created.is_some_and(|at| at + CLOCK_SLACK >= since))
                .filter(|_| {
                    session_cwd(path)
                        .is_some_and(|there| fs::canonicalize(&there).unwrap_or(there) == folder)
                });
            found.extend(id.map(str::to_owned));
        });
    }
    found.dedup();
    (found.len() == 1).then(|| found.remove(0))
}

/// Visits the rollouts in the day folders (`sessions/YYYY/MM/DD`) from the day before `since`
/// on, which spares a walk through every session the CLI ever saved.
fn recent_rollouts(sessions: &Path, since: SystemTime, each: &mut dyn FnMut(&Path)) {
    use chrono::Datelike;
    let day = chrono::DateTime::<chrono::Local>::from(since).date_naive() - chrono::Days::new(1);
    let (year, month, date) = (day.year(), day.month(), day.day());
    let numbered = |dir: &Path, least: u32| -> Vec<(u32, PathBuf)> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let number: u32 = entry.file_name().to_str()?.parse().ok()?;
                (number >= least).then(|| (number, entry.path()))
            })
            .collect()
    };
    let least = u32::try_from(year).unwrap_or_default();
    for (y, year_dir) in numbered(sessions, least) {
        let months = numbered(&year_dir, if y == least { month } else { 0 });
        for (m, month_dir) in months {
            let first = if y == least && m == month { date } else { 0 };
            for (_, day_dir) in numbered(&month_dir, first) {
                for entry in fs::read_dir(day_dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if file_name(&path).starts_with("rollout-") {
                        each(&path);
                    }
                }
            }
        }
    }
}

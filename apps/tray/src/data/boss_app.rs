//! Which app a Claude or Codex boss chats in. Each desktop app runs the same CLI as the terminal,
//! so the host stays `claude` or `codex` and only the session's own records tell them apart: a
//! Claude transcript's `entrypoint`, a Codex rollout's `originator`, and Claude Desktop's records
//! of the CLI sessions it ran. Claude's follow magpie (`yetone/magpie`), which reads the first
//! `entrypoint` where a boss needs the latest; it reads no `originator`, measured here instead.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const CLAUDE_DESKTOP: &str = "claude-desktop";
pub(crate) const CLAUDE_CLI: &str = "claude-cli";
pub(crate) const CODEX_DESKTOP: &str = "codex-desktop";
pub(crate) const CODEX_CLI: &str = "codex-cli";

/// How much of a transcript is searched at each end; a rollout's first line is its session_meta.
const WINDOW: u64 = 256 * 1024;
/// A Claude Desktop record larger than this is not read, as in magpie.
const RECORD_LIMIT: u64 = 8 << 20;

#[derive(Default)]
pub(crate) struct Homes {
    pub claude: Vec<PathBuf>,
    pub codex: Vec<PathBuf>,
    /// Claude Desktop's data folders, its third-party mode's included.
    pub desktop: Vec<PathBuf>,
}

impl Homes {
    /// Before the hosts page has probed the login shell, only the app's own variables are known.
    pub(super) fn here() -> Self {
        if cfg!(test) {
            return Self::default();
        }
        let home = |variable: &str, fallback: &str| match std::env::var_os(variable) {
            Some(dir) if !dir.is_empty() => vec![PathBuf::from(dir)],
            _ => dirs::home_dir()
                .map(|home| home.join(fallback))
                .into_iter()
                .collect(),
        };
        Self {
            claude: home("CLAUDE_CONFIG_DIR", ".claude"),
            codex: home("CODEX_HOME", ".codex"),
            ..Self::desktop_data()
        }
    }

    pub(crate) fn desktop_data() -> Self {
        let desktop = dirs::config_dir()
            .filter(|_| !cfg!(test))
            .map(|dir| vec![dir.join("Claude"), dir.join("Claude-3p")]);
        Self {
            desktop: desktop.unwrap_or_default(),
            ..Self::default()
        }
    }
}

/// What a sweep of the session folders found, by boss key.
#[derive(Default)]
struct Found {
    transcripts: HashMap<String, PathBuf>,
    desktop: HashSet<String>,
    /// Bosses already looked for, so one whose records are gone costs a single sweep.
    searched: HashSet<String>,
}

pub(super) struct BossApps {
    /// The app a transcript names, by file and its length: transcripts only grow, so an
    /// unchanged length needs no second read.
    files: Mutex<HashMap<PathBuf, (u64, Option<&'static str>)>>,
    found: Mutex<Found>,
}

struct Boss {
    workspace: String,
    key: String,
    host: String,
    file: Option<PathBuf>,
}

impl BossApps {
    pub fn new() -> Self {
        Self {
            files: Mutex::default(),
            found: Mutex::default(),
        }
    }

    /// Every workspace whose boss's app is known; the rest are left out. `homes` is only asked
    /// for when a boss has to be looked for.
    pub fn of(
        &self,
        connection: &rusqlite::Connection,
        homes: impl FnOnce() -> Homes,
    ) -> Vec<(String, &'static str)> {
        let bosses = bosses(connection);
        let mut found = self.found.lock().unwrap_or_else(|p| p.into_inner());
        self.refresh(&mut found, &bosses, homes);
        let mut apps: Vec<_> = bosses
            .iter()
            .filter_map(|boss| {
                let file = boss
                    .file
                    .as_ref()
                    .filter(|file| file.is_file())
                    .or_else(|| found.transcripts.get(&boss.key));
                let named = file.and_then(|file| self.named_by(&boss.host, file));
                let app = named.or_else(|| {
                    (boss.host == "claude" && found.desktop.contains(&boss.key))
                        .then_some(CLAUDE_DESKTOP)
                })?;
                Some((boss.workspace.clone(), app))
            })
            .collect();
        apps.sort_unstable();
        apps
    }

    /// Sweep again only for a boss not looked for yet, or whose transcript has since moved.
    fn refresh(&self, found: &mut Found, bosses: &[Boss], homes: impl FnOnce() -> Homes) {
        let mut missing = Vec::new();
        for boss in bosses {
            if boss.file.as_ref().is_some_and(|file| file.is_file()) {
                continue;
            }
            if let Some(known) = found.transcripts.get(&boss.key) {
                if known.is_file() {
                    continue;
                }
                found.searched.remove(&boss.key);
            }
            missing.push(boss.key.clone());
        }
        if missing.iter().all(|key| found.searched.contains(key)) {
            return;
        }
        let searched = std::mem::take(&mut found.searched);
        *found = sweep(&homes());
        found.searched = searched;
        found.searched.extend(missing);
    }

    fn named_by(&self, host: &str, file: &Path) -> Option<&'static str> {
        let len = fs::metadata(file).ok()?.len();
        let lock = || self.files.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(&(seen, app)) = lock().get(file)
            && seen == len
        {
            return app;
        }
        // Read without the lock; a transcript read whole that names no app is remembered too, a
        // failed read is tried again next time.
        let app = match host {
            "claude" => claude_entrypoint(file).ok()?.as_deref().map(claude_app),
            _ => codex_originator(file).ok()?.as_deref().and_then(codex_app),
        };
        lock().insert(file.to_owned(), (len, app));
        app
    }
}

/// The current boss of every workspace, and of older chats saved before workspaces were
/// recorded, whose group id is their boss's key.
fn bosses(connection: &rusqlite::Connection) -> Vec<Boss> {
    let rows = |sql: &str| -> Vec<(String, String, String, Option<String>)> {
        connection
            .prepare(sql)
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })?
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut bosses = HashMap::new();
    let older = rows(
        "SELECT manager_session, manager_session, host, NULL FROM conversations \
         WHERE manager_session IS NOT NULL AND host IN ('claude', 'codex')",
    );
    let recorded = rows(
        "SELECT id, owner, owner_host, owner_file FROM workspaces \
         WHERE owner_host IN ('claude', 'codex')",
    );
    let replaced: HashSet<String> = recorded.iter().map(|row| row.0.clone()).collect();
    for (workspace, key, host, file) in older
        .into_iter()
        .filter(|row| !replaced.contains(&row.0))
        .chain(recorded)
    {
        bosses.entry(workspace.clone()).or_insert(Boss {
            workspace,
            key,
            host,
            file: file.map(PathBuf::from),
        });
    }
    bosses.into_values().collect()
}

fn sweep(homes: &Homes) -> Found {
    let mut found = Found::default();
    for claude in &homes.claude {
        files_at(&claude.join("projects"), 1, &mut |path| {
            let name = file_name(path);
            if let Some(id) = name.strip_suffix(".jsonl").filter(|id| is_uuid(id)) {
                keep_newest(&mut found.transcripts, boss_key("claude", id), path);
            }
        });
    }
    for codex in &homes.codex {
        let mut rollout = |path: &Path| {
            let name = file_name(path);
            let stem = name
                .strip_suffix(".jsonl.zst")
                .or_else(|| name.strip_suffix(".jsonl"));
            if let Some(id) = stem
                .filter(|stem| stem.starts_with("rollout-") && stem.len() > 36)
                .map(|stem| &stem[stem.len() - 36..])
                .filter(|id| is_uuid(id))
            {
                keep_newest(&mut found.transcripts, boss_key("codex", id), path);
            }
        };
        files_at(&codex.join("sessions"), 3, &mut rollout);
        files_at(&codex.join("archived_sessions"), 0, &mut rollout);
    }
    for data in &homes.desktop {
        for kind in ["claude-code-sessions", "local-agent-mode-sessions"] {
            files_at(&data.join(kind), 2, &mut |path| {
                let name = file_name(path);
                if name.starts_with("local_")
                    && name.ends_with(".json")
                    && fs::metadata(path).is_ok_and(|file| file.len() <= RECORD_LIMIT)
                    && let Ok(text) = fs::read_to_string(path)
                    && let Ok(record) = serde_json::from_str::<Value>(&text)
                {
                    found.desktop.extend(
                        desktop_session_ids(&record)
                            .into_iter()
                            .map(|id| boss_key("claude", id)),
                    );
                }
            });
        }
    }
    found
}

/// The CLI sessions Claude Desktop counts as one of its chats.
fn desktop_session_ids(record: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = [
        "cliSessionId",
        "unarchivedCliSessionId",
        "preClearCliSessionId",
    ]
    .iter()
    .filter_map(|key| record[key].as_str())
    .collect();
    ids.extend(
        record["priorCliSessionIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    );
    if record["cliSessionId"].as_str().is_none()
        && let Some(id) = record["sessionId"]
            .as_str()
            .and_then(|id| id.strip_prefix("local_"))
    {
        ids.push(id);
    }
    ids
}

fn files_at(dir: &Path, depth: usize, each: &mut dyn FnMut(&Path)) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if depth > 0 {
            files_at(&path, depth - 1, each);
        } else {
            each(&path);
        }
    }
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

/// A session a chat moved between folders leaves copies; the last one written is the live one.
fn keep_newest(transcripts: &mut HashMap<String, PathBuf>, key: String, path: &Path) {
    let written = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
    if transcripts
        .get(&key)
        .is_none_or(|known| written(known) < written(path))
    {
        transcripts.insert(key, path.to_owned());
    }
}

fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(at, c)| match at {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

/// A boss chat's key: the service's `_boss`, which hashes `json.dumps([host, session])`.
fn boss_key(host: &str, session: &str) -> String {
    let digest = Sha256::digest(format!("[\"{host}\", \"{session}\"]"));
    format!("s-{}", &format!("{digest:x}")[..32])
}

fn claude_app(entrypoint: &str) -> &'static str {
    if matches!(entrypoint, "claude-desktop" | "claude-desktop-3p") {
        CLAUDE_DESKTOP
    } else {
        CLAUDE_CLI
    }
}

/// Any other originator (an editor extension, an SDK) is neither, and stays unnamed.
fn codex_app(originator: &str) -> Option<&'static str> {
    if originator.to_ascii_lowercase().contains("desktop") {
        return Some(CODEX_DESKTOP);
    }
    matches!(
        originator,
        "codex_cli_rs" | "codex-tui" | "codex_exec" | "codex-cli"
    )
    .then_some(CODEX_CLI)
}

fn value_of(chunk: &[u8], key: &[u8], latest: bool) -> Option<String> {
    let mut places = chunk.windows(key.len());
    let at = if latest {
        places.rposition(|w| w == key)
    } else {
        places.position(|w| w == key)
    }? + key.len();
    let end = chunk[at..].iter().position(|&b| b == b'"')? + at;
    String::from_utf8(chunk[at..end].to_vec()).ok()
}

/// The latest `entrypoint` a Claude Code transcript records ("cli", "claude-desktop", ...): a
/// chat resumed in the other app keeps writing the same file. Its last 256 KiB are searched
/// first, then its first; None when it names none there.
fn claude_entrypoint(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::{Seek, SeekFrom};
    const KEY: &[u8] = b"\"entrypoint\":\"";
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut chunk = Vec::new();
    file.seek(SeekFrom::Start(len.saturating_sub(WINDOW)))?;
    file.by_ref().take(WINDOW).read_to_end(&mut chunk)?;
    if let Some(found) = value_of(&chunk, KEY, true) {
        return Ok(Some(found));
    }
    if len <= WINDOW {
        return Ok(None);
    }
    chunk.clear();
    file.seek(SeekFrom::Start(0))?;
    file.take(WINDOW).read_to_end(&mut chunk)?;
    Ok(value_of(&chunk, KEY, true))
}

/// The `originator` in a Codex rollout's first line; Codex packs archived rollouts with zstd.
fn codex_originator(path: &Path) -> std::io::Result<Option<String>> {
    let file = fs::File::open(path)?;
    let mut chunk = Vec::new();
    if file_name(path).ends_with(".zst") {
        ruzstd::decoding::StreamingDecoder::new(file)
            .map_err(std::io::Error::other)?
            .take(WINDOW)
            .read_to_end(&mut chunk)?;
    } else {
        file.take(WINDOW).read_to_end(&mut chunk)?;
    }
    Ok(value_of(&chunk, b"\"originator\":\"", false))
}

#[cfg(test)]
mod tests;

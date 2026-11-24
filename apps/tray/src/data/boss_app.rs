//! Which app a Claude or Codex boss chats in. Each desktop app runs the same CLI as the terminal,
//! so the host stays `claude` or `codex` and only the session's own records tell them apart: a
//! Claude transcript's `entrypoint`, a Codex rollout's `originator`, and Claude Desktop's records
//! of the CLI sessions it ran. A boss shows where it chats now, so Claude's latest `entrypoint`
//! counts, not its first; the Codex `originator` values were measured here.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const CLAUDE_DESKTOP: &str = "claude-desktop";
pub(crate) const CLAUDE_CLI: &str = "claude-cli";
pub(crate) const CODEX_DESKTOP: &str = "codex-desktop";
pub(crate) const CODEX_CLI: &str = "codex-cli";
pub(crate) const ANTIGRAVITY_DESKTOP: &str = "antigravity-desktop";
pub(crate) const ANTIGRAVITY_CLI: &str = "antigravity-cli";

/// How much of a transcript is searched at each end; a rollout's first line is its session_meta.
const WINDOW: u64 = 256 * 1024;
/// A Claude Desktop record larger than this is not read.
const RECORD_LIMIT: u64 = 8 << 20;
/// Retry missing transcripts periodically so sessions created slightly after workspace creation resolve.
const NEGATIVE_TTL: Duration = Duration::from_secs(15);

mod antigravity;
mod cwd;
mod other;
mod title;
pub(crate) use title::CliTitle;

/// Locate a Claude Code session transcript by UUID within `homes`.
pub(crate) fn claude_transcript_in(homes: &[PathBuf], id: &str) -> Option<PathBuf> {
    title::claude_file(homes, id)
}

/// A terminal's chat, whether its session record was located, and its CLI's title for it.
pub(crate) fn terminal_title(
    provider: &str,
    native: Option<&str>,
    cwd: &Path,
    since: std::time::SystemTime,
    exclude: &HashSet<String>,
    claude_home: Option<&Path>,
) -> (Option<String>, bool, Option<CliTitle>) {
    let homes = Homes::here().with_claude(claude_home);
    title::of_terminal_located(&homes, provider, native, cwd, since, exclude)
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Homes {
    pub claude: Vec<PathBuf>,
    pub codex: Vec<PathBuf>,
    /// Claude Desktop's data folders, its third-party mode's included.
    pub desktop: Vec<PathBuf>,
    /// Where the other CLIs whose chat names can be read keep their sessions, by host.
    pub other: Vec<(String, PathBuf)>,
}

impl Homes {
    /// These homes, with Claude's replaced by a named account's folder.
    fn with_claude(mut self, folder: Option<&Path>) -> Self {
        if let Some(folder) = folder {
            self.claude = vec![folder.to_owned()];
        }
        self
    }

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
        let mut other: Vec<(String, PathBuf)> = other::HOSTS
            .iter()
            .filter_map(|host| Some(((*host).to_owned(), other::home_of(host)?)))
            .collect();
        if let Some(path) = antigravity::home() {
            other.push(("antigravity".to_owned(), path));
        }
        Self {
            claude: home("CLAUDE_CONFIG_DIR", ".claude"),
            codex: home("CODEX_HOME", ".codex"),
            other,
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
    homes: Option<Homes>,
    transcripts: HashMap<String, PathBuf>,
    other_transcripts: HashMap<String, PathBuf>,
    desktop: HashSet<String>,
    desktop_cwds: HashMap<String, String>,
    /// Antigravity bosses with no saved record, found by key in the CLI's or the desktop app's data.
    antigravity: HashMap<String, &'static str>,
    /// Bosses already looked for with when they missed, so gone records do not sweep on every poll
    /// while newly created transcripts still resolve once `NEGATIVE_TTL` passes.
    searched: HashMap<String, Instant>,
}

type CachedCwd = (u64, Option<SystemTime>, Option<String>);

pub(super) struct BossApps {
    /// The app a transcript names, by file and its length: transcripts only grow, so an
    /// unchanged length needs no second read and appended tails only scan new bytes.
    files: Mutex<HashMap<PathBuf, (u64, Option<&'static str>)>>,
    cwds: Mutex<HashMap<PathBuf, CachedCwd>>,
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
            cwds: Mutex::default(),
            found: Mutex::default(),
        }
    }

    /// Working directory of each workspace's original boss session, read from its session records.
    pub fn cwds(
        &self,
        connection: &rusqlite::Connection,
        homes: impl FnOnce() -> Homes,
    ) -> HashMap<String, String> {
        cwd::of(self, connection, homes)
    }

    /// Every workspace whose boss's app is known; the rest are left out. Resolve `homes`
    /// when a boss has no saved transcript, so newly probed roots can invalidate a miss.
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
                if boss.host == "antigravity" {
                    // The service records an agy chat's file from desktop, CLI, or T3 Code ACP.
                    // T3 Code ACP sessions are neither Antigravity Desktop nor resumable CLI chats.
                    let app = match boss.file.as_deref() {
                        Some(file) if antigravity::is_acp_record(file) => return None,
                        Some(file) if antigravity::is_desktop_record(file) => ANTIGRAVITY_DESKTOP,
                        Some(_) => ANTIGRAVITY_CLI,
                        None => *found.antigravity.get(&boss.key)?,
                    };
                    return Some((boss.workspace.clone(), app));
                }
                let file = transcript(boss, &found);
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

    /// The CLI's own name for each workspace's boss chat, for the workspaces whose CLI keeps one
    /// where the App can read it (Claude Code and Codex); the rest are left out.
    pub fn titles(
        &self,
        connection: &rusqlite::Connection,
        homes: impl FnOnce() -> Homes,
    ) -> Vec<(String, CliTitle)> {
        let bosses = bosses(connection);
        let homes = homes();
        let mut found = self.found.lock().unwrap_or_else(|p| p.into_inner());
        self.refresh(&mut found, &bosses, || homes.clone());
        let mut titles: Vec<_> = bosses
            .iter()
            .filter_map(|boss| {
                if boss.host == "antigravity" {
                    return None;
                }
                let file = transcript(boss, &found)?;
                let title = match boss.host.as_str() {
                    "claude" => title::claude(file),
                    _ => title::codex(&homes.codex, &rollout_id(file)?),
                }?;
                Some((boss.workspace.clone(), title))
            })
            .collect();
        titles.extend(other_titles(connection, &homes));
        titles.sort_unstable();
        titles
    }

    /// Sweep again only for a boss not looked for yet, or whose transcript has since moved.
    fn refresh(&self, found: &mut Found, bosses: &[Boss], homes: impl FnOnce() -> Homes) {
        let mut missing = Vec::new();
        for boss in bosses {
            if boss.host == "antigravity" {
                if boss.file.is_some() || found.antigravity.contains_key(&boss.key) {
                    continue;
                }
                missing.push(boss.key.clone());
                continue;
            }
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
        if missing.is_empty() {
            return;
        }
        let homes = homes();
        let roots_changed = found.homes.as_ref() != Some(&homes);
        let now = Instant::now();
        if !roots_changed
            && missing.iter().all(|key| {
                found
                    .searched
                    .get(key)
                    .is_some_and(|at| now.duration_since(*at) < NEGATIVE_TTL)
            })
        {
            return;
        }
        let mut searched = if roots_changed {
            HashMap::new()
        } else {
            let mut kept = std::mem::take(&mut found.searched);
            kept.retain(|_, at| now.duration_since(*at) < NEGATIVE_TTL);
            kept
        };
        let mut antigravity_apps = std::mem::take(&mut found.antigravity);
        let other_transcripts = if roots_changed {
            HashMap::new()
        } else {
            std::mem::take(&mut found.other_transcripts)
        };
        let cli_roots = antigravity::roots(&homes);
        let desktop_roots = antigravity::desktop_roots(&cli_roots);
        for boss in bosses.iter().filter(|boss| {
            boss.host == "antigravity" && boss.file.is_none() && missing.contains(&boss.key)
        }) {
            let app = if antigravity::find_by_key(&cli_roots, &boss.key).is_some() {
                ANTIGRAVITY_CLI
            } else if antigravity::find_by_key(&desktop_roots, &boss.key).is_some() {
                ANTIGRAVITY_DESKTOP
            } else {
                continue;
            };
            antigravity_apps.insert(boss.key.clone(), app);
        }
        for key in missing {
            searched.insert(key, now);
        }
        *found = sweep(&homes);
        found.antigravity = antigravity_apps;
        found.other_transcripts = other_transcripts;
        found.homes = Some(homes);
        found.searched = searched;
    }

    fn named_by(&self, host: &str, file: &Path) -> Option<&'static str> {
        let len = fs::metadata(file).ok()?.len();
        let lock = || self.files.lock().unwrap_or_else(|p| p.into_inner());
        let cached = lock().get(file).copied();
        if let Some((seen, app)) = cached {
            if seen == len {
                return app;
            }
            // Codex's originator sits on line 1 and never changes as the rollout grows.
            if host != "claude" && app.is_some() && len > seen {
                lock().insert(file.to_owned(), (len, app));
                return app;
            }
            // For an active Claude transcript that already named an app, scan only appended bytes.
            if host == "claude"
                && len > seen
                && let Some(previous) = app
                && let Ok(delta) = claude_entrypoint_since(file, seen, len)
            {
                let next = Some(delta.as_deref().map_or(previous, claude_app));
                lock().insert(file.to_owned(), (len, next));
                return next;
            }
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

/// The names of the other CLIs' chats, from the session record each workspace saved (or from a
/// sweep / summary DB for Grok and Antigravity CLI when `owner_file` is absent).
fn other_titles(connection: &rusqlite::Connection, homes: &Homes) -> Vec<(String, CliTitle)> {
    let hosts = other::HOSTS.map(|host| format!("'{host}'")).join(", ");
    let sql = format!(
        "SELECT id, owner_host, owner_file FROM workspaces \
         WHERE owner_host IN ({hosts}) AND owner_file IS NOT NULL"
    );
    let rows: Vec<(String, String, String)> = connection
        .prepare(&sql)
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .unwrap_or_default();
    let mut titles: Vec<(String, CliTitle)> = rows
        .into_iter()
        .filter_map(|(workspace, host, file)| {
            Some((workspace, other::title_of(&host, Path::new(&file))?))
        })
        .collect();

    let missing_grok: Vec<(String, String)> = connection
        .prepare(
            "SELECT id, owner FROM workspaces WHERE owner_host = 'grok' AND owner_file IS NULL",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .unwrap_or_default();
    if !missing_grok.is_empty() {
        let swept = sweep(homes);
        for (workspace, owner) in missing_grok {
            if let Some(record) = swept.transcripts.get(&owner)
                && let Some(title) = other::title_of("grok", record)
            {
                titles.push((workspace, title));
            }
        }
    }

    let agy_rows: Vec<(String, String, Option<String>)> = connection
        .prepare("SELECT id, owner, owner_file FROM workspaces WHERE owner_host = 'antigravity'")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .unwrap_or_default();
    if !agy_rows.is_empty() {
        let roots = antigravity::roots(homes);
        for (workspace, owner, file) in agy_rows {
            if let Some(title) =
                antigravity::title_for_workspace(&roots, &owner, file.as_deref().map(Path::new))
            {
                titles.push((workspace, title));
            }
        }
    }
    titles
}

/// The session record a boss chat is known by: the one its workspace saved, else the one a sweep
/// of the session folders found.
fn transcript<'a>(boss: &'a Boss, found: &'a Found) -> Option<&'a PathBuf> {
    boss.file
        .as_ref()
        .filter(|file| file.is_file())
        .or_else(|| found.transcripts.get(&boss.key))
}

/// The ID a Codex rollout's file name carries.
fn rollout_id(path: &Path) -> Option<String> {
    let name = file_name(path);
    let stem = name
        .strip_suffix(".jsonl.zst")
        .or_else(|| name.strip_suffix(".jsonl"))?;
    codex_rollout_id(stem).map(str::to_owned)
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
         WHERE owner_host IN ('claude', 'codex', 'antigravity')",
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
            if let Some(id) = stem.and_then(codex_rollout_id) {
                keep_newest(&mut found.transcripts, boss_key("codex", id), path);
            }
        };
        files_at(&codex.join("sessions"), 3, &mut rollout);
        files_at(&codex.join("archived_sessions"), 0, &mut rollout);
    }
    for (host, home) in &homes.other {
        if host == "grok" {
            files_at(&home.join("sessions"), 2, &mut |path| {
                if file_name(path) == "summary.json"
                    && let Some(id) = path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .filter(|id| is_uuid(id))
                {
                    keep_newest(&mut found.transcripts, boss_key("grok", id), path);
                }
            });
        }
    }
    let (desktop, desktop_cwds) = desktop_keys(homes);
    found.desktop = desktop;
    found.desktop_cwds = desktop_cwds;
    found
}

/// The keys of the Claude chats Claude Desktop's own records count as its chats, and any startup
/// directory each record names.
fn desktop_keys(homes: &Homes) -> (HashSet<String>, HashMap<String, String>) {
    let mut keys = HashSet::new();
    let mut cwds = HashMap::new();
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
                    let cwd = ["originCwd", "cwd"]
                        .iter()
                        .filter_map(|field| record[*field].as_str())
                        .map(str::trim)
                        .find(|dir| !dir.is_empty() && Path::new(dir).is_absolute())
                        .map(str::to_owned);
                    for id in desktop_session_ids(&record) {
                        let key = boss_key("claude", id);
                        if let Some(cwd) = &cwd {
                            cwds.entry(key.clone()).or_insert_with(|| cwd.clone());
                        }
                        keys.insert(key);
                    }
                }
            });
        }
    }
    (keys, cwds)
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

fn codex_rollout_id(stem: &str) -> Option<&str> {
    let rest = stem.strip_prefix("rollout-")?;
    let tail = rest
        .get(rest.len().checked_sub(36)?..)
        .filter(|id| is_uuid(id))?;
    if let Some(earlier) = rest[..rest.len() - 36]
        .strip_suffix('_')
        .and_then(|s| s.get(s.len().checked_sub(36)?..))
        .filter(|id| is_uuid(id))
    {
        return Some(earlier);
    }
    Some(tail)
}

/// The native ID of the boss chat with this key and its session record, to continue it in a
/// terminal (ADR-0103): from the record its workspace names, else from a sweep of the session
/// folders. Only an ID whose key matches is returned, so a file name with more than one ID cannot
/// pick another chat.
#[cfg(test)]
pub(crate) fn native_session(
    host: &str,
    key: &str,
    file: Option<&Path>,
) -> Option<(String, PathBuf)> {
    native_session_as(host, key, file, None)
}

/// The same for a chat of a named Claude account, whose records are in that account's folder.
pub(crate) fn native_session_as(
    host: &str,
    key: &str,
    file: Option<&Path>,
    claude_home: Option<&Path>,
) -> Option<(String, PathBuf)> {
    native_session_in(&Homes::here().with_claude(claude_home), host, key, file)
}

/// Locate a desktop record for handover without allowing it to be resumed. A chat counts as the
/// desktop app's by the same records `BossApps::of` reads: a Claude transcript's latest
/// `entrypoint`, else Claude Desktop's own records; an Antigravity record's folder, else, with no
/// record saved, a chat found by key only in the desktop app's data.
pub(crate) fn desktop_session(
    host: &str,
    key: &str,
    file: Option<&Path>,
    home: Option<&Path>,
) -> Option<(String, PathBuf)> {
    desktop_session_in(&Homes::here().with_claude(home), host, key, file)
}

pub(crate) fn desktop_session_in(
    homes: &Homes,
    host: &str,
    key: &str,
    file: Option<&Path>,
) -> Option<(String, PathBuf)> {
    if host == "antigravity" {
        let Some(path) = file else {
            let cli_roots = antigravity::roots(homes);
            if antigravity::find_by_key(&cli_roots, key).is_some() {
                return None;
            }
            return antigravity::find_by_key(&antigravity::desktop_roots(&cli_roots), key);
        };
        if !antigravity::is_desktop_record(path) {
            return None;
        }
        if let Some(id) = antigravity::id_from_path(path) {
            return (boss_key(host, &id) == key).then(|| (id, path.to_owned()));
        }
        let root = antigravity::root_from_record(path)?;
        return antigravity::find_by_key(&[root], key);
    }
    if host != "claude" {
        return None;
    }
    let (id, path) = native_session_in(homes, host, key, file)?;
    let app = match claude_entrypoint(&path).ok()? {
        Some(entrypoint) => claude_app(&entrypoint),
        None if desktop_keys(homes).0.contains(key) => CLAUDE_DESKTOP,
        None => CLAUDE_CLI,
    };
    (app == CLAUDE_DESKTOP).then_some((id, path))
}

pub(crate) fn native_session_in(
    homes: &Homes,
    host: &str,
    key: &str,
    file: Option<&Path>,
) -> Option<(String, PathBuf)> {
    if host == "antigravity" {
        if let Some(path) = file {
            if antigravity::is_desktop_record(path) || antigravity::is_acp_record(path) {
                return None;
            }
            if let Some(id) = antigravity::id_from_path(path)
                && boss_key("antigravity", &id) == key
            {
                return Some((id, path.to_owned()));
            }
        }
        return antigravity::find_by_key(&antigravity::roots(homes), key);
    }
    let id_of = |path: &Path| -> Option<(String, PathBuf)> {
        let name = file_name(path);
        let id = match host {
            "claude" => name.strip_suffix(".jsonl").filter(|id| is_uuid(id)),
            "codex" => name
                .strip_suffix(".jsonl.zst")
                .or_else(|| name.strip_suffix(".jsonl"))
                .and_then(codex_rollout_id),
            "grok" if matches!(name, "chat_history.jsonl" | "summary.json") => path
                .parent()
                .and_then(Path::file_name)
                .and_then(|dir| dir.to_str())
                .filter(|id| is_uuid(id)),
            _ => None,
        }?;
        (boss_key(host, id) == key).then(|| (id.to_owned(), path.to_owned()))
    };
    file.and_then(id_of).or_else(|| {
        sweep(homes)
            .transcripts
            .get(key)
            .and_then(|path| id_of(path))
    })
}

/// The folder a boss chat ran in, read according to the CLI's session format.
pub(crate) fn session_cwd_of(host: &str, record: &Path, id: &str) -> Option<PathBuf> {
    match host {
        "grok" => {
            let summary = if file_name(record) == "summary.json" {
                record.to_owned()
            } else {
                record.parent()?.join("summary.json")
            };
            other::cwd_of("grok", &summary)
        }
        "antigravity" => {
            let root = antigravity::root_from_record(record).or_else(antigravity::home)?;
            antigravity::cwd_for_id(&root, id)
        }
        _ => session_cwd(record),
    }
}

fn read_head(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut chunk = Vec::new();
    let name = file_name(path);
    if name.ends_with(".zst") || name.ends_with(".zstd") {
        let mut decoder =
            ruzstd::decoding::StreamingDecoder::new(file).map_err(std::io::Error::other)?;
        let _ = (&mut decoder).take(limit).read_to_end(&mut chunk);
    } else {
        file.take(limit).read_to_end(&mut chunk)?;
    }
    Ok(chunk)
}

/// The folder a boss chat ran in, from the start of its session record: Claude Code and Codex
/// both write it there, and a CLI finds the chat to continue by it.
pub(crate) fn session_cwd(path: &Path) -> Option<PathBuf> {
    let chunk = read_head(path, WINDOW).ok()?;
    json_string_field(&chunk, b"\"cwd\":").map(PathBuf::from)
}

fn json_string_field(chunk: &[u8], key: &[u8]) -> Option<String> {
    let mut offset = 0;
    while let Some(pos) = chunk[offset..]
        .windows(key.len())
        .position(|window| window == key)
    {
        let mut cursor = offset + pos + key.len();
        while cursor < chunk.len() && chunk[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if chunk.get(cursor) == Some(&b'"')
            && let Some(Ok(value)) = serde_json::Deserializer::from_slice(&chunk[cursor..])
                .into_iter::<String>()
                .next()
        {
            return Some(value);
        }
        offset += pos + key.len();
    }
    None
}

/// A boss chat's key: the service's `_boss`, which hashes `json.dumps([host, session])`.
pub(crate) fn boss_key(host: &str, session: &str) -> String {
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

fn claude_entrypoint_since(path: &Path, seen: u64, len: u64) -> std::io::Result<Option<String>> {
    use std::io::{Seek, SeekFrom};
    const KEY: &[u8] = b"\"entrypoint\":\"";
    let start = seen.saturating_sub(64).max(len.saturating_sub(WINDOW));
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut chunk = Vec::new();
    file.take(len.saturating_sub(start))
        .read_to_end(&mut chunk)?;
    Ok(value_of(&chunk, KEY, true))
}

/// The `originator` in a Codex rollout's first line; Codex packs archived rollouts with zstd.
fn codex_originator(path: &Path) -> std::io::Result<Option<String>> {
    let chunk = read_head(path, WINDOW)?;
    Ok(value_of(&chunk, b"\"originator\":\"", false))
}

#[cfg(test)]
mod tests;

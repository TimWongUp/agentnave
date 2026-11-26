//! Automatic discovery of local agent log directories and multi-account homes (ADR-0129).
//!
//! Claude & Codex `$HOME` multi-account discovery and Claude Desktop / Kimi / MiniMax / OpenCode
//! layouts follow yetone/magpie `internal/appdir/claude_homes.go` and `internal/sessions/` (MIT).
//! T3 Code `userdata/settings.json` is read strictly read-only for `config.homePath` only; no
//! credentials, tokens, or auth fields are ever read.

use crate::data::boss_app::Homes;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) type RootEntry = (&'static str, PathBuf, usize);

pub(super) fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return home.join(rest);
    }
    PathBuf::from(trimmed)
}

pub(super) fn canon_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn push_unique(
    dirs: &mut Vec<RootEntry>,
    seen: &mut HashSet<(&'static str, PathBuf)>,
    provider: &'static str,
    path: PathBuf,
    depth: usize,
) {
    if seen.insert((provider, canon_key(&path))) {
        dirs.push((provider, path, depth));
    }
}

fn live_env(key: &str) -> Option<String> {
    std::env::var_os(key)
        .map(|v| v.to_string_lossy().trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn env_dir(
    env: &dyn Fn(&str) -> Option<String>,
    key: &str,
    home: &Path,
    fallback: PathBuf,
) -> PathBuf {
    env(key).map_or(fallback, |raw| expand_tilde(&raw, home))
}

/// Resolve `T3CODE_HOME` (default `~/.t3`).
pub(super) fn t3_home_in(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env_dir(env, "T3CODE_HOME", home, home.join(".t3"))
}

/// Strictly read-only extraction of `config.homePath` for `claudeAgent` and `codex` instances
/// from `<t3_home>/userdata/settings.json`. Never reads tokens, secrets, or auth fields.
pub(super) fn read_t3_instance_homes(t3_home: &Path, home: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut claude = Vec::new();
    let mut codex = Vec::new();
    let Some(doc) = super::read_small_json(&t3_home.join("userdata/settings.json")) else {
        return (claude, codex);
    };
    for container in ["providerInstances", "providers"] {
        let Some(map) = doc.get(container).and_then(Value::as_object) else {
            continue;
        };
        for (key, entry) in map {
            let driver = entry
                .get("driver")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(raw_home) = entry
                .get("config")
                .and_then(|cfg| cfg.get("homePath"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let path = expand_tilde(raw_home, home);
            if driver == "claudeAgent" || key.contains("claudeAgent") {
                claude.push(path);
            } else if driver == "codex"
                || key == "codex"
                || key.starts_with("codex_")
                || key.starts_with("codex-")
                || key.contains("codex")
            {
                codex.push(path);
            }
        }
    }
    (claude, codex)
}

/// Scan `search_roots` for directories whose lowercased name (ignoring a leading `.`) starts with
/// `prefix`, accepting either the directory itself or one level beneath it when `predicate` holds.
/// Reference: yetone/magpie `internal/appdir/claude_homes.go` (MIT).
fn scan_prefixed_homes(
    search_roots: &[PathBuf],
    prefix: &str,
    predicate: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut visited_roots = HashSet::new();
    for root in search_roots {
        if root.as_os_str().is_empty() || !visited_roots.insert(canon_key(root)) {
            continue;
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for dir in entries {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let trimmed = name.strip_prefix('.').unwrap_or(&name);
            if !trimmed.starts_with(prefix) || !dir.is_dir() {
                continue;
            }
            if predicate(&dir) {
                found.push(dir);
                continue;
            }
            let mut subs: Vec<PathBuf> = fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect();
            subs.sort();
            for sub in subs {
                if sub.is_dir() && predicate(&sub) {
                    found.push(sub);
                }
            }
        }
    }
    found
}

fn default_desktop_dirs(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let base = home.join("Library/Application Support");
        vec![base.join("Claude"), base.join("Claude-3p")]
    } else if cfg!(windows) {
        let mut out = Vec::new();
        if let Some(appdata) = env("APPDATA").map(PathBuf::from).or_else(dirs::data_dir) {
            out.push(appdata.join("Claude"));
            out.push(appdata.join("Claude-3p"));
        }
        if let Some(local) = env("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(dirs::data_local_dir)
        {
            out.push(local.join("Claude-3p"));
        }
        out
    } else {
        let base = env("XDG_CONFIG_HOME")
            .map(|v| expand_tilde(&v, home))
            .unwrap_or_else(|| home.join(".config"));
        vec![base.join("Claude"), base.join("Claude-3p")]
    }
}

/// Discover Claude Desktop / Cowork `local-agent-mode-sessions/*/*/local_*/.claude/projects`.
/// Reference: yetone/magpie `internal/sessions/calls.go` (MIT).
fn discover_claude_desktop_projects(desktop_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen_bases = HashSet::new();
    for base in desktop_dirs {
        if !seen_bases.insert(canon_key(base)) {
            continue;
        }
        let sessions = base.join("local-agent-mode-sessions");
        if !sessions.is_dir() {
            continue;
        }
        for acct in sorted_subdirs(&sessions) {
            for org in sorted_subdirs(&acct) {
                for local in sorted_subdirs(&org) {
                    let is_local = local
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("local_"));
                    if !is_local {
                        continue;
                    }
                    let projects = local.join(".claude/projects");
                    if projects.is_dir() {
                        out.push(projects);
                    }
                }
            }
        }
    }
    out
}

fn sorted_subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn discover_t3_antigravity_brains(t3_home: &Path) -> Vec<PathBuf> {
    let root = t3_home.join("userdata/providers/antigravity");
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for profile in sorted_subdirs(&root) {
        let brain = profile.join("antigravity-acp/brain");
        if brain.is_dir() {
            out.push(brain);
        }
    }
    out
}

fn add_claude_roots(
    home: &Path,
    homes: &Homes,
    t3_claude: Vec<PathBuf>,
    env: &dyn Fn(&str) -> Option<String>,
    dirs: &mut Vec<RootEntry>,
    seen: &mut HashSet<(&'static str, PathBuf)>,
) {
    let explicit_env = env("CLAUDE_CONFIG_DIR").map(|raw| expand_tilde(&raw, home));
    let mut search_roots = vec![home.to_path_buf()];
    if let Some(parent) = explicit_env.as_deref().and_then(Path::parent) {
        search_roots.push(parent.to_path_buf());
    }
    for root in &homes.claude {
        push_unique(dirs, seen, "claude", root.join("projects"), 4);
    }
    if let Some(root) = explicit_env {
        push_unique(dirs, seen, "claude", root.join("projects"), 4);
    }
    let default_claude = home.join(".claude");
    if homes.claude.is_empty() || default_claude.join("projects").is_dir() {
        push_unique(dirs, seen, "claude", default_claude.join("projects"), 4);
    }
    for root in scan_prefixed_homes(&search_roots, "claude", |d| d.join("projects").is_dir()) {
        push_unique(dirs, seen, "claude", root.join("projects"), 4);
    }
    for root in t3_claude {
        push_unique(dirs, seen, "claude", root.join("projects"), 4);
    }
}

fn add_codex_roots(
    home: &Path,
    homes: &Homes,
    t3_codex: Vec<PathBuf>,
    env: &dyn Fn(&str) -> Option<String>,
    dirs: &mut Vec<RootEntry>,
    seen: &mut HashSet<(&'static str, PathBuf)>,
) {
    let explicit_env = env("CODEX_HOME").map(|raw| expand_tilde(&raw, home));
    let mut search_roots = vec![home.to_path_buf()];
    if let Some(parent) = explicit_env.as_deref().and_then(Path::parent) {
        search_roots.push(parent.to_path_buf());
    }
    let mut codex_homes: Vec<PathBuf> = homes.codex.clone();
    if let Some(root) = explicit_env {
        codex_homes.push(root);
    }
    let default_codex = home.join(".codex");
    if homes.codex.is_empty()
        || default_codex.join("sessions").is_dir()
        || default_codex.join("archived_sessions").is_dir()
    {
        codex_homes.push(default_codex);
    }
    codex_homes.extend(scan_prefixed_homes(&search_roots, "codex", |d| {
        d.join("auth.json").is_file() || d.join("sessions").is_dir()
    }));
    codex_homes.extend(t3_codex);
    for root in codex_homes {
        push_unique(dirs, seen, "codex", root.join("sessions"), 5);
        push_unique(dirs, seen, "codex", root.join("archived_sessions"), 2);
    }
}

fn add_other_jsonl_roots(
    home: &Path,
    t3_home: &Path,
    homes: &Homes,
    env: &dyn Fn(&str) -> Option<String>,
    dirs: &mut Vec<RootEntry>,
    seen: &mut HashSet<(&'static str, PathBuf)>,
) {
    for (provider, var, default, sub, depth) in [
        ("pi", "PI_CODING_AGENT_DIR", ".pi/agent", "sessions", 2),
        (
            "codebuddy",
            "CODEBUDDY_CONFIG_DIR",
            ".codebuddy",
            "projects",
            4,
        ),
        (
            "workbuddy",
            "WORKBUDDY_CONFIG_DIR",
            ".workbuddy",
            "projects",
            4,
        ),
        ("deepseek", "DSH_HOME", ".dsh", "sessions", 3),
        ("grok", "GROK_HOME", ".grok", "sessions", 3),
    ] {
        let found: Vec<PathBuf> = homes
            .other
            .iter()
            .filter(|(host, _)| host == provider)
            .map(|(_, path)| path.clone())
            .collect();
        let mut bases = if found.is_empty() {
            vec![env_dir(env, var, home, home.join(default))]
        } else {
            found
        };
        if let Some(raw) = env(var) {
            bases.push(expand_tilde(&raw, home));
        }
        if provider == "workbuddy" {
            let wb_default = home.join(".workbuddy");
            if wb_default.join("projects").is_dir() {
                bases.push(wb_default);
            }
            let wb_ai = home.join(".workbuddy-ai");
            if wb_ai.join("projects").is_dir() {
                bases.push(wb_ai);
            }
        }
        for root in bases {
            push_unique(dirs, seen, provider, root.join(sub), depth);
        }
    }

    for (_, root) in homes
        .other
        .iter()
        .filter(|(host, _)| matches!(host.as_str(), "antigravity" | "antigravity-acp"))
    {
        push_unique(dirs, seen, "antigravity", root.join("brain"), 5);
    }
    push_unique(
        dirs,
        seen,
        "antigravity",
        env_dir(
            env,
            "ANTIGRAVITY_CLI_HOME",
            home,
            home.join(".gemini/antigravity-cli"),
        )
        .join("brain"),
        5,
    );
    push_unique(
        dirs,
        seen,
        "antigravity",
        home.join(".gemini/antigravity/brain"),
        5,
    );
    let insiders = home.join(".gemini/antigravity-insiders/brain");
    if insiders.is_dir() {
        push_unique(dirs, seen, "antigravity", insiders, 5);
    }
    for brain in discover_t3_antigravity_brains(t3_home) {
        push_unique(dirs, seen, "antigravity", brain, 5);
    }

    let kimi_sessions =
        env_dir(env, "KIMI_CODE_HOME", home, home.join(".kimi-code")).join("sessions");
    if kimi_sessions.is_dir() {
        push_unique(dirs, seen, "kimi", kimi_sessions, 5);
    }
    let minimax_sessions =
        env_dir(env, "MINIMAX_DATA_DIR", home, home.join(".minimax")).join("v2/sessions");
    if minimax_sessions.is_dir() {
        push_unique(dirs, seen, "minimax", minimax_sessions, 6);
    }
}

/// Pure, injectable root discovery for tests and live scanning.
pub(super) fn roots_in(
    home: &Path,
    t3_home: &Path,
    desktop_dirs: &[PathBuf],
    homes: &Homes,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<RootEntry> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    let (t3_claude, t3_codex) = read_t3_instance_homes(t3_home, home);
    add_claude_roots(home, homes, t3_claude, env, &mut dirs, &mut seen);
    for projects in discover_claude_desktop_projects(desktop_dirs) {
        push_unique(&mut dirs, &mut seen, "claude-desktop", projects, 4);
    }
    add_codex_roots(home, homes, t3_codex, env, &mut dirs, &mut seen);
    add_other_jsonl_roots(home, t3_home, homes, env, &mut dirs, &mut seen);
    dirs
}

/// Live root discovery on this machine.
pub(super) fn roots(homes: &Homes) -> Vec<RootEntry> {
    let home = dirs::home_dir().unwrap_or_default();
    let t3_home = t3_home_in(&home, &live_env);
    let mut desktop_dirs = homes.desktop.clone();
    desktop_dirs.extend(default_desktop_dirs(&home, &live_env));
    roots_in(&home, &t3_home, &desktop_dirs, homes, &live_env)
}

/// OpenCode SQLite database path: `$OPENCODE_DB` (relative to data dir unless absolute),
/// else `$XDG_DATA_HOME/opencode/opencode.db` or `~/.local/share/opencode/opencode.db`.
/// Reference: yetone/magpie `internal/sessions/opencode.go` (MIT).
pub(super) fn opencode_db_in(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_DATA_HOME")
        .map(|v| expand_tilde(&v, home).join("opencode"))
        .unwrap_or_else(|| home.join(".local/share/opencode"));
    if let Some(raw) = env("OPENCODE_DB").filter(|v| v != ":memory:") {
        let p = expand_tilde(&raw, home);
        if p.is_absolute() {
            return p;
        }
        return base.join(p);
    }
    base.join("opencode.db")
}

pub(super) fn opencode_db() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    opencode_db_in(&home, &live_env)
}

/// Hermes SQLite state databases: `$HERMES_HOME/state.db` (default `~/.hermes/state.db`)
/// plus `profiles/*/state.db`, deduplicated by canonical path.
pub(super) fn hermes_dbs_in(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let base = env_dir(env, "HERMES_HOME", home, home.join(".hermes"));
    let mut candidates = vec![base.join("state.db")];
    let profiles = base.join("profiles");
    if profiles.is_dir() {
        for dir in sorted_subdirs(&profiles) {
            let db = dir.join("state.db");
            if db.is_file() {
                candidates.push(db);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for db in candidates {
        if db.is_file() && seen.insert(canon_key(&db)) {
            out.push(db);
        }
    }
    out
}

pub(super) fn hermes_dbs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    hermes_dbs_in(&home, &live_env)
}

/// Devin CLI SQLite database path (optional; skipped when absent).
pub(super) fn devin_db_in(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env_dir(env, "DEVIN_HOME", home, home.join(".devin")).join("sessions.db")
}

pub(super) fn devin_db() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    devin_db_in(&home, &live_env)
}

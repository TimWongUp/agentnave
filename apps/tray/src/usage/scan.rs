use super::{Event, buckets, db, discover, frames, hermes, now, parsers, query, zcode};
use crate::data::boss_app::Homes;
use crate::projects::{OTHER, Project};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_LINE: u64 = 4 * 1024 * 1024;
const DAY: i64 = buckets::DAY;

type Found = Vec<(&'static str, PathBuf, SystemTime)>;

fn recent(modified: SystemTime, now_ms: i64) -> bool {
    modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
        >= now_ms.saturating_sub(buckets::RETENTION_WINDOW_MS)
}

/// Per-scan resolver caching `cwd -> Project` so each unique directory is only stat'd once,
/// layered with AgentNave read-only worker/boss attribution windows.
pub(super) struct ProjectResolver<'a> {
    cwd_cache: HashMap<String, Project>,
    windows: &'a [query::Window],
    bosses: &'a [query::Boss],
    classify: fn(&str) -> Project,
}

impl<'a> ProjectResolver<'a> {
    pub(super) fn new(windows: &'a [query::Window], bosses: &'a [query::Boss]) -> Self {
        Self {
            cwd_cache: HashMap::new(),
            windows,
            bosses,
            classify: crate::projects::classify_usage_dir,
        }
    }

    #[cfg(test)]
    pub(super) fn with_classifier(
        windows: &'a [query::Window],
        bosses: &'a [query::Boss],
        classify: fn(&str) -> Project,
    ) -> Self {
        Self {
            cwd_cache: HashMap::new(),
            windows,
            bosses,
            classify,
        }
    }

    fn classify_cwd(&mut self, cwd: &str) -> Project {
        let trimmed = cwd.trim();
        if trimmed.is_empty() {
            return Project::other();
        }
        if let Some(cached) = self.cwd_cache.get(trimmed) {
            return cached.clone();
        }
        let project = (self.classify)(trimmed);
        self.cwd_cache.insert(trimmed.to_owned(), project.clone());
        project
    }

    pub(super) fn resolve(&mut self, e: &Event) -> (Project, &'static str) {
        if let Some((proj, worker)) = query::assign(e, self.windows, self.bosses) {
            let role = if worker { "worker" } else { "boss" };
            if proj.key != OTHER {
                return (proj.clone(), role);
            }
            return (self.classify_cwd(&e.cwd), role);
        }
        (self.classify_cwd(&e.cwd), "native")
    }
}

/// DeepSeek Harness keeps older format files beside the newest one: only the highest
/// `session[.vN].jsonl[.zstd]` of a session folder is read, the later written of two kinds.
fn latest_deepseek(dir: &Path, now_ms: i64, out: &mut Found) {
    let mut best: Option<(u32, SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("session") else {
            continue;
        };
        let Some(rest) = rest
            .strip_suffix(".jsonl.zstd")
            .or_else(|| rest.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let version = match rest.strip_prefix(".v") {
            Some(n) => n.parse().ok(),
            None => rest.is_empty().then_some(0),
        };
        let (Some(version), Ok(modified)) = (version, entry.metadata().and_then(|m| m.modified()))
        else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(v, m, _)| (version, modified) > (*v, *m))
        {
            best = Some((version, modified, entry.path()));
        }
    }
    if let Some((_, modified, path)) = best
        && recent(modified, now_ms)
    {
        out.push(("deepseek", path, modified));
    }
}

#[cfg(test)]
pub(super) fn discover(dir: &Path, provider: &'static str, depth: usize, out: &mut Found) {
    discover_at(dir, provider, depth, now(), out);
}

fn discover_at(dir: &Path, provider: &'static str, depth: usize, now_ms: i64, out: &mut Found) {
    if depth == 0 {
        return;
    }
    if provider == "deepseek" && depth == 1 {
        return latest_deepseek(dir, now_ms, out);
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            discover_at(&path, provider, depth - 1, now_ms, out);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let valid = match provider {
            "claude" | "claude-desktop" | "pi" | "codebuddy" | "workbuddy" => {
                name.ends_with(".jsonl")
            }
            "grok" => name == "updates.jsonl",
            "codex" => {
                (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
                    && (name.starts_with("rollout-")
                        || path
                            .components()
                            .any(|c| c.as_os_str() == "archived_sessions"))
            }
            "kimi" => name == "wire.jsonl",
            "minimax" => name.starts_with("messages") && name.ends_with(".jsonl"),
            _ => name == "transcript_full.jsonl",
        };
        if valid
            && let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            && recent(modified, now_ms)
        {
            out.push((provider, path, modified));
        }
    }
}

fn antigravity_root_of(transcript: &Path) -> Option<PathBuf> {
    for ancestor in transcript.ancestors() {
        if ancestor.file_name().and_then(|n| n.to_str()) == Some("brain") {
            return ancestor.parent().map(discover::canon_key);
        }
    }
    None
}

fn load_antigravity_cwds(
    roots: &[discover::RootEntry],
) -> HashMap<PathBuf, HashMap<String, String>> {
    let mut map = HashMap::new();
    for (provider, brain_dir, _) in roots {
        if *provider != "antigravity" {
            continue;
        }
        let Some(parent) = brain_dir.parent() else {
            continue;
        };
        let key = discover::canon_key(parent);
        if map.contains_key(&key) {
            continue;
        }
        let loaded = parsers::antigravity::load_cwds(parent);
        map.insert(key, loaded);
    }
    map
}

pub(super) fn run_with(
    root: &Path,
    homes: &Homes,
    windows: &[query::Window],
    bosses: &[query::Boss],
) -> Result<(), Box<dyn std::error::Error>> {
    let scan_now = now();
    let discovered_roots = discover::roots(homes);
    let agy_cwds = load_antigravity_cwds(&discovered_roots);
    let mut files = Vec::new();
    for (provider, path, depth) in &discovered_roots {
        discover_at(path, provider, *depth, scan_now, &mut files);
    }
    files.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    let mut conn = db::open(root)?;
    let mut resolver = ProjectResolver::new(windows, bosses);
    for (provider, path, _) in files {
        let cwds = if provider == "antigravity" {
            antigravity_root_of(&path).and_then(|key| agy_cwds.get(&key))
        } else {
            None
        };
        if let Err(e) = file_with(&mut conn, provider, &path, cwds, &mut resolver, scan_now) {
            eprintln!("用量日志 {}：{e}", path.display());
        }
    }
    let mut resolve_fn = |e: &Event| resolver.resolve(e);
    let zcode_db = zcode::database();
    if let Err(e) = zcode::scan_provider(
        &mut conn,
        &zcode_db,
        "zcode",
        Duration::from_secs(2),
        scan_now,
        &mut resolve_fn,
    ) {
        eprintln!("用量日志 {}：{e}", zcode_db.display());
    }
    let opencode_db = discover::opencode_db();
    if let Err(e) = zcode::scan_provider(
        &mut conn,
        &opencode_db,
        "opencode",
        Duration::from_secs(2),
        scan_now,
        &mut resolve_fn,
    ) {
        eprintln!("用量日志 {}：{e}", opencode_db.display());
    }
    for hermes_db in discover::hermes_dbs() {
        if let Err(e) = hermes::scan_with(
            &mut conn,
            &hermes_db,
            Duration::from_secs(2),
            scan_now,
            &mut resolve_fn,
        ) {
            eprintln!("用量日志 {}：{e}", hermes_db.display());
        }
    }
    let devin_db = discover::devin_db();
    if let Err(e) = zcode::scan_devin(&mut conn, &devin_db, scan_now, &mut resolve_fn) {
        eprintln!("用量日志 {}：{e}", devin_db.display());
    }
    if let Err(e) = db::prune(&mut conn, scan_now) {
        eprintln!("用量库清理失败：{e}");
    }
    Ok(())
}

fn head(path: &Path, len: u64) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(len.min(4096))
        .read_to_end(&mut bytes)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn reader(path: &Path, offset: u64) -> io::Result<Box<dyn Read>> {
    let mut f = File::open(path)?;
    if path.extension().is_some_and(|x| x == "zst" || x == "zstd") {
        let mut frames = frames::Frames::new(f);
        io::copy(&mut frames.by_ref().take(offset), &mut io::sink())?;
        Ok(Box::new(frames))
    } else {
        f.seek(SeekFrom::Start(offset))?;
        Ok(Box::new(f))
    }
}

#[cfg(test)]
pub(super) fn file(
    conn: &mut rusqlite::Connection,
    provider: &str,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut resolver = ProjectResolver::new(&[], &[]);
    file_with(conn, provider, path, None, &mut resolver, now())
}

pub(super) fn file_with(
    conn: &mut rusqlite::Connection,
    provider: &str,
    path: &Path,
    agy_cwds: Option<&HashMap<String, String>>,
    resolver: &mut ProjectResolver<'_>,
    now_ms: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let meta = fs::metadata(path)?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    let name = path.to_string_lossy();
    let mut c = db::cursor(conn, &name)?.unwrap_or_default();
    if c.size == size && c.mtime == mtime {
        return Ok(());
    }
    let changed_head = !c.head.is_empty() && head(path, c.size as u64)? != c.head;
    if size < c.size || changed_head {
        c = db::Cursor::default();
    }
    let mut state = serde_json::from_str(&c.state).unwrap_or_else(|_| parsers::initial());
    let input = reader(path, c.offset as u64)?;
    let retention_floor = now_ms.saturating_sub(buckets::RETENTION_WINDOW_MS);
    let future_cap = now_ms.saturating_add(DAY);
    let tx = conn.transaction()?;
    let offset = lines(input, c.offset as u64, |line| {
        for e in parsers::parse_with(provider, path, line, &mut state, agy_cwds) {
            if e.ts >= retention_floor && e.ts <= future_cap {
                let (project, role_kind) = resolver.resolve(&e);
                db::save_event(&tx, &e, &project, role_kind, now_ms)?;
            }
        }
        Ok(())
    })?;
    c.size = size;
    c.mtime = mtime;
    c.offset = offset as i64;
    c.head = head(path, size as u64)?;
    c.state = state.to_string();
    db::save_cursor(&tx, &name, provider, &c)?;
    tx.commit()?;
    Ok(())
}

fn lines(
    input: impl Read,
    mut offset: u64,
    mut apply: impl FnMut(&[u8]) -> rusqlite::Result<()>,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut input = BufReader::new(input);
    loop {
        let mut line = Vec::new();
        let count = input
            .by_ref()
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        if count as u64 > MAX_LINE {
            let mut skipped = count as u64;
            if line.last() != Some(&b'\n') {
                let mut discard = Vec::new();
                loop {
                    discard.clear();
                    let n = input
                        .by_ref()
                        .take(65_536)
                        .read_until(b'\n', &mut discard)?;
                    if n == 0 {
                        return Ok(offset);
                    }
                    skipped += n as u64;
                    if discard.last() == Some(&b'\n') {
                        break;
                    }
                }
            }
            offset += skipped;
            continue;
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        apply(&line)?;
        offset += count as u64;
    }
    Ok(offset)
}

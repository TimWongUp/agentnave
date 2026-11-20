use super::{db, frames, now, parsers, zcode};
use crate::data::boss_app::Homes;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LINE: u64 = 4 * 1024 * 1024;
const DAY: i64 = 86_400_000;

fn env_dir(key: &str, fallback: PathBuf) -> PathBuf {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map_or(fallback, PathBuf::from)
}

fn push_unique(
    dirs: &mut Vec<(&'static str, PathBuf, usize)>,
    seen: &mut HashSet<(&'static str, PathBuf)>,
    provider: &'static str,
    path: PathBuf,
    depth: usize,
) {
    if seen.insert((provider, path.clone())) {
        dirs.push((provider, path, depth));
    }
}

pub(super) fn roots(homes: &Homes) -> Vec<(&'static str, PathBuf, usize)> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    let claude = if homes.claude.is_empty() {
        vec![env_dir("CLAUDE_CONFIG_DIR", home.join(".claude"))]
    } else {
        homes.claude.clone()
    };
    for root in claude {
        push_unique(&mut dirs, &mut seen, "claude", root.join("projects"), 4);
    }

    let codex = if homes.codex.is_empty() {
        vec![env_dir("CODEX_HOME", home.join(".codex"))]
    } else {
        homes.codex.clone()
    };
    for root in codex {
        push_unique(&mut dirs, &mut seen, "codex", root.join("sessions"), 5);
        push_unique(
            &mut dirs,
            &mut seen,
            "codex",
            root.join("archived_sessions"),
            2,
        );
    }

    for (provider, var, default, sub, depth) in [
        ("pi", "PI_CODING_AGENT_DIR", ".pi/agent", "sessions", 2),
        (
            "codebuddy",
            "CODEBUDDY_CONFIG_DIR",
            ".codebuddy",
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
        let bases = if found.is_empty() {
            vec![env_dir(var, home.join(default))]
        } else {
            found
        };
        for root in bases {
            push_unique(&mut dirs, &mut seen, provider, root.join(sub), depth);
        }
    }

    for (_, root) in homes.other.iter().filter(|(host, _)| host == "antigravity") {
        push_unique(&mut dirs, &mut seen, "antigravity", root.join("brain"), 5);
    }
    push_unique(
        &mut dirs,
        &mut seen,
        "antigravity",
        env_dir("ANTIGRAVITY_CLI_HOME", home.join(".gemini/antigravity-cli")).join("brain"),
        5,
    );
    push_unique(
        &mut dirs,
        &mut seen,
        "antigravity",
        home.join(".gemini/antigravity/brain"),
        5,
    );

    dirs
}

type Found = Vec<(&'static str, PathBuf, SystemTime)>;

fn recent(modified: SystemTime) -> bool {
    modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
        >= now() - 31 * DAY
}

/// DeepSeek Harness keeps older format files beside the newest one: only the highest
/// `session[.vN].jsonl[.zstd]` of a session folder is read, the later written of two kinds.
fn latest_deepseek(dir: &Path, out: &mut Found) {
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
        && recent(modified)
    {
        out.push(("deepseek", path, modified));
    }
}

pub(super) fn discover(dir: &Path, provider: &'static str, depth: usize, out: &mut Found) {
    if depth == 0 {
        return;
    }
    if provider == "deepseek" && depth == 1 {
        return latest_deepseek(dir, out);
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            discover(&path, provider, depth - 1, out);
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let valid = match provider {
            "claude" | "pi" | "codebuddy" => name.ends_with(".jsonl"),
            "grok" => name == "updates.jsonl",
            "codex" => {
                (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
                    && (name.starts_with("rollout-")
                        || path
                            .components()
                            .any(|c| c.as_os_str() == "archived_sessions"))
            }
            _ => name == "transcript_full.jsonl",
        };
        if valid
            && let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            && recent(modified)
        {
            out.push((provider, path, modified));
        }
    }
}

pub(super) fn run(root: &Path, homes: &Homes) -> Result<(), Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for (provider, path, depth) in roots(homes) {
        discover(&path, provider, depth, &mut files);
    }
    files.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    let mut conn = db::open(root)?;
    for (provider, path, _) in files {
        if let Err(e) = file(&mut conn, provider, &path) {
            eprintln!("用量日志 {}：{e}", path.display());
        }
    }
    let database = zcode::database();
    if let Err(e) = zcode::scan(&mut conn, &database) {
        eprintln!("用量日志 {}：{e}", database.display());
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

pub(super) fn file(
    conn: &mut rusqlite::Connection,
    provider: &str,
    path: &Path,
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
    let tx = conn.transaction()?;
    let offset = lines(input, c.offset as u64, |line| {
        for e in parsers::parse(provider, path, line, &mut state) {
            if e.ts >= now() - 31 * DAY && e.ts <= now() + DAY {
                db::save(&tx, &e)?;
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

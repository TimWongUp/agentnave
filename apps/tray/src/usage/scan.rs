use super::{db, frames, now, parsers, zcode};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LINE: u64 = 4 * 1024 * 1024;
const DAY: i64 = 86_400_000;

fn roots() -> Vec<(&'static str, PathBuf, usize)> {
    let home = dirs::home_dir().unwrap_or_default();
    let env = |key: &str, fallback: PathBuf| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map_or(fallback, PathBuf::from)
    };
    let claude = env("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let codex = env("CODEX_HOME", home.join(".codex"));
    let agy = env("ANTIGRAVITY_CLI_HOME", home.join(".gemini/antigravity-cli"));
    let pi = env("PI_CODING_AGENT_DIR", home.join(".pi/agent"));
    let codebuddy = env("CODEBUDDY_CONFIG_DIR", home.join(".codebuddy"));
    let deepseek = env("DSH_HOME", home.join(".dsh"));
    let grok = env("GROK_HOME", home.join(".grok"));
    let desktop_agy = home.join(".gemini/antigravity");
    let mut dirs = vec![
        ("claude", claude.join("projects"), 4),
        ("codex", codex.join("sessions"), 5),
        ("codex", codex.join("archived_sessions"), 2),
        ("antigravity", agy.join("brain"), 5),
        ("pi", pi.join("sessions"), 2),
        ("codebuddy", codebuddy.join("projects"), 4),
        ("deepseek", deepseek.join("sessions"), 3),
        ("grok", grok.join("sessions"), 3),
    ];
    if desktop_agy != agy {
        dirs.push(("antigravity", desktop_agy.join("brain"), 5));
    }
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

pub(super) fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for (provider, path, depth) in roots() {
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

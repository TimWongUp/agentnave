//! Antigravity CLI (`agy`) session metadata, read strictly read-only from its own SQLite
//! summary database (`~/.gemini/antigravity-cli/conversation_summaries.db`). Desktop App records
//! under `~/.gemini/antigravity/` are rejected for terminal resume because the desktop app has no
//! terminal resume entry.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::Value;

use super::title::CliTitle;
use super::{boss_key, file_name, is_uuid};

/// A new chat's record may be created a moment before the terminal's start time is taken.
const CLOCK_SLACK: Duration = Duration::from_secs(2);
const BUSY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct SummaryRow {
    title: String,
    workspace_uris: String,
    last_modified_time: String,
}

type Stamp = (u64, Option<SystemTime>);

struct CachedSummaries {
    db: Stamp,
    wal: Option<Stamp>,
    rows: Arc<HashMap<String, SummaryRow>>,
}

static SUMMARIES: LazyLock<Mutex<HashMap<PathBuf, CachedSummaries>>> =
    LazyLock::new(Mutex::default);

pub(super) fn home() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".gemini/antigravity-cli"))
}

pub(super) fn roots(homes: &super::Homes) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = homes
        .other
        .iter()
        .filter(|(host, _)| host == "antigravity")
        .map(|(_, path)| path.clone())
        .collect();
    if found.is_empty()
        && !cfg!(test)
        && let Some(default) = home()
    {
        found.push(default);
    }
    found
}

/// Whether `path` belongs to the Antigravity Desktop App (`~/.gemini/antigravity/`) rather than
/// `antigravity-cli`.
pub(super) fn is_desktop_record(path: &Path) -> bool {
    let names: Vec<&str> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    if names.last().copied() == Some("conversation_summaries.db")
        && let Some(parent) = names.iter().rev().nth(1).copied()
    {
        return matches!(parent, "antigravity" | "antigravity-insiders");
    }
    names.windows(2).any(|pair| {
        matches!(pair[0], "antigravity" | "antigravity-insiders")
            && matches!(pair[1], "brain" | "conversations")
    })
}

pub(super) fn id_from_path(path: &Path) -> Option<String> {
    if path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some("conversations")
        && path.extension().and_then(|ext| ext.to_str()) == Some("db")
    {
        let stem = path.file_stem()?.to_str()?;
        return is_uuid(stem).then(|| stem.to_owned());
    }
    let mut components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str());
    while let Some(part) = components.next() {
        if part == "brain" {
            let id = components.next()?;
            return is_uuid(id).then(|| id.to_owned());
        }
    }
    None
}

pub(super) fn root_from_record(record: &Path) -> Option<PathBuf> {
    if file_name(record) == "conversation_summaries.db" {
        return record.parent().map(Path::to_owned);
    }
    for ancestor in record.ancestors() {
        if matches!(file_name(ancestor), "brain" | "conversations") {
            return ancestor.parent().map(Path::to_owned);
        }
    }
    None
}

pub(super) fn title_for_id(root: &Path, id: &str) -> Option<CliTitle> {
    let rows = summaries(root)?;
    non_empty_title(rows.get(id)?)
}

pub(super) fn cwd_for_id(root: &Path, id: &str) -> Option<PathBuf> {
    let rows = summaries(root)?;
    first_cwd(&rows.get(id)?.workspace_uris)
}

pub(super) fn find_by_key(roots: &[PathBuf], key: &str) -> Option<(String, PathBuf)> {
    for root in roots {
        if let Some(rows) = summaries(root) {
            for id in rows.keys() {
                if is_uuid(id) && boss_key("antigravity", id) == key {
                    return Some((id.clone(), root.join("conversation_summaries.db")));
                }
            }
        }
        for entry in fs::read_dir(root.join("conversations"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("db")
                && let Some(id) = path.file_stem().and_then(|stem| stem.to_str())
                && is_uuid(id)
                && boss_key("antigravity", id) == key
            {
                return Some((id.to_owned(), path));
            }
        }
        for entry in fs::read_dir(root.join("brain"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let id = file_name(&path);
            if path.is_dir() && is_uuid(id) && boss_key("antigravity", id) == key {
                return Some((
                    id.to_owned(),
                    path.join(".system_generated/logs/transcript_full.jsonl"),
                ));
            }
        }
    }
    None
}

pub(super) fn title_for_workspace(
    roots: &[PathBuf],
    owner: &str,
    owner_file: Option<&Path>,
) -> Option<CliTitle> {
    if let Some(path) = owner_file {
        if is_desktop_record(path) {
            return None;
        }
        if let Some(id) = id_from_path(path) {
            if let Some(root) = root_from_record(path)
                && let Some(title) = title_for_id(&root, &id)
            {
                return Some(title);
            }
            return roots.iter().find_map(|root| title_for_id(root, &id));
        }
    }
    let (id, record) = find_by_key(roots, owner)?;
    if let Some(root) = root_from_record(&record)
        && let Some(title) = title_for_id(&root, &id)
    {
        return Some(title);
    }
    roots.iter().find_map(|root| title_for_id(root, &id))
}

pub(super) fn terminal_title(
    roots: &[PathBuf],
    native: Option<&str>,
    cwd: &Path,
    since: SystemTime,
    exclude: &HashSet<String>,
    before: Option<SystemTime>,
) -> (Option<String>, bool, Option<CliTitle>) {
    if let Some(id) = native {
        let mut located = false;
        for root in roots {
            let in_summary = summaries(root).is_some_and(|rows| rows.contains_key(id));
            let has_files = root
                .join("conversations")
                .join(format!("{id}.db"))
                .is_file()
                || root.join("brain").join(id).is_dir();
            if in_summary || has_files {
                located = true;
                if let Some(title) = title_for_id(root, id) {
                    return (Some(id.to_owned()), true, Some(title));
                }
            }
        }
        return (Some(id.to_owned()), located, None);
    }
    let folder = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let mut found: Vec<(PathBuf, String)> = Vec::new();
    for root in roots {
        let Some(rows) = summaries(root) else {
            continue;
        };
        for (id, row) in rows.iter() {
            if !is_uuid(id) || exclude.contains(id) {
                continue;
            }
            let Some(there) = first_cwd(&row.workspace_uris) else {
                continue;
            };
            let canonical = fs::canonicalize(&there).unwrap_or(there);
            if canonical == folder && started_between(root, id, row, since, before) {
                found.push((root.clone(), id.clone()));
            }
        }
    }
    if found.len() != 1 {
        return (None, false, None);
    }
    let (root, id) = found.remove(0);
    let title = title_for_id(&root, &id);
    (Some(id), true, title)
}

fn non_empty_title(row: &SummaryRow) -> Option<CliTitle> {
    let text = row.title.trim();
    (!text.is_empty()).then(|| CliTitle {
        text: text.to_owned(),
        custom: false,
    })
}

fn started_between(
    root: &Path,
    id: &str,
    row: &SummaryRow,
    since: SystemTime,
    before: Option<SystemTime>,
) -> bool {
    let within = |at: SystemTime| {
        at + CLOCK_SLACK >= since && before.is_none_or(|peer| at + CLOCK_SLACK < peer)
    };
    let conv_db = root.join("conversations").join(format!("{id}.db"));
    if let Ok(meta) = fs::metadata(conv_db)
        && let Ok(at) = meta.created().or_else(|_| meta.modified())
    {
        return within(at);
    }
    parse_time(&row.last_modified_time).is_some_and(within)
}

fn parse_time(raw: &str) -> Option<SystemTime> {
    let trimmed = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(SystemTime::from(dt));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%d %H:%M:%S%:z"] {
        if let Ok(dt) = DateTime::parse_from_str(trimmed, format) {
            return Some(SystemTime::from(dt));
        }
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Some(SystemTime::from(
                DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc),
            ));
        }
    }
    None
}

fn first_cwd(workspace_uris: &str) -> Option<PathBuf> {
    let Value::Array(uris) = serde_json::from_str::<Value>(workspace_uris).ok()? else {
        return None;
    };
    uris.iter()
        .filter_map(Value::as_str)
        .find_map(decode_file_uri)
}

pub(super) fn decode_file_uri(uri: &str) -> Option<PathBuf> {
    decode_file_uri_for(uri, cfg!(windows))
}

pub(super) fn decode_file_uri_for(uri: &str, windows: bool) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let (host, raw_path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, ""),
    };
    if windows {
        let host_decoded = percent_decode(host)?;
        if let Some(drive) = windows_drive(&host_decoded) {
            let tail = percent_decode(raw_path)?;
            return Some(PathBuf::from(windows_drive_path(drive, &tail)));
        }
        let decoded = percent_decode(raw_path)?;
        if !host_decoded.is_empty() && !host_decoded.eq_ignore_ascii_case("localhost") {
            return (!decoded.is_empty()).then(|| {
                PathBuf::from(format!("\\\\{host_decoded}{}", decoded.replace('/', "\\")))
            });
        }
        if let Some(rest_after_slash) = decoded.strip_prefix('/')
            && let Some(drive) = windows_drive_prefix(rest_after_slash)
        {
            return Some(PathBuf::from(windows_drive_path(
                drive,
                &rest_after_slash[2..],
            )));
        }
        return (!decoded.is_empty()).then(|| PathBuf::from(decoded.replace('/', "\\")));
    }
    if raw_path.is_empty() || (!host.is_empty() && !host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let decoded = percent_decode(raw_path)?;
    (!decoded.is_empty()).then(|| PathBuf::from(decoded))
}

fn windows_drive(text: &str) -> Option<char> {
    let bytes = text.as_bytes();
    (bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        .then(|| (bytes[0] as char).to_ascii_uppercase())
}

fn windows_drive_prefix(text: &str) -> Option<char> {
    let bytes = text.as_bytes();
    (bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/'))
        .then(|| (bytes[0] as char).to_ascii_uppercase())
}

fn windows_drive_path(drive: char, tail: &str) -> String {
    if tail.is_empty() || tail == "/" {
        format!("{drive}:\\")
    } else {
        format!("{drive}:{}", tail.replace('/', "\\"))
    }
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

fn summaries(root: &Path) -> Option<Arc<HashMap<String, SummaryRow>>> {
    let db_path = root.join("conversation_summaries.db");
    let db_stamp = stamp(&db_path)?;
    let wal_stamp = stamp(&db_path.with_file_name("conversation_summaries.db-wal"));
    {
        let cache = SUMMARIES.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = cache.get(&db_path)
            && entry.db == db_stamp
            && entry.wal == wal_stamp
        {
            return Some(entry.rows.clone());
        }
    }
    let rows = read_summaries(&db_path)?;
    let rows = Arc::new(rows);
    SUMMARIES.lock().unwrap_or_else(|p| p.into_inner()).insert(
        db_path,
        CachedSummaries {
            db: db_stamp,
            wal: wal_stamp,
            rows: rows.clone(),
        },
    );
    Some(rows)
}

fn read_summaries(db_path: &Path) -> Option<HashMap<String, SummaryRow>> {
    let conn = open_read_only(db_path)?;
    let mut stmt = conn
        .prepare(
            "SELECT conversation_id, title, workspace_uris, last_modified_time \
             FROM conversation_summaries",
        )
        .ok()?;
    let mapped = stmt
        .query_map([], |row| {
            let id = col_string(row, 0);
            if id.is_empty() {
                return Ok(None);
            }
            Ok(Some((
                id,
                SummaryRow {
                    title: col_string(row, 1),
                    workspace_uris: col_string(row, 2),
                    last_modified_time: col_string(row, 3),
                },
            )))
        })
        .ok()?;
    let mut out = HashMap::new();
    for entry in mapped.flatten().flatten() {
        out.insert(entry.0, entry.1);
    }
    Some(out)
}

fn col_string(row: &rusqlite::Row<'_>, index: usize) -> String {
    match row.get_ref(index) {
        Ok(ValueRef::Text(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Ok(ValueRef::Integer(number)) => number.to_string(),
        Ok(ValueRef::Real(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn open_read_only(db_path: &Path) -> Option<Connection> {
    if !db_path.is_file() {
        return None;
    }
    let wal = db_path.with_file_name("conversation_summaries.db-wal");
    let shm = db_path.with_file_name("conversation_summaries.db-shm");
    let conn = if wal.exists() && shm.exists() {
        Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?
    } else {
        let uri = immutable_uri(db_path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI;
        Connection::open_with_flags(uri, flags).ok()?
    };
    let _ = conn.busy_timeout(BUSY_TIMEOUT);
    Some(conn)
}

fn immutable_uri(path: &Path) -> Option<String> {
    let raw = path.to_str()?;
    let normalized = if cfg!(windows) {
        format!("/{}", raw.replace('\\', "/").trim_start_matches('/'))
    } else {
        raw.to_owned()
    };
    let mut encoded = String::with_capacity(normalized.len() + 20);
    encoded.push_str("file://");
    for ch in normalized.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3F"),
            '#' => encoded.push_str("%23"),
            _ => encoded.push(ch),
        }
    }
    encoded.push_str("?immutable=1");
    Some(encoded)
}

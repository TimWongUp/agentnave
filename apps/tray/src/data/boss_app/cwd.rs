//! Where each workspace's original boss session ran, read-only from the host's session records.
//! When no boss directory can be read (missing record, older/retired host, or NULL cwd), callers
//! fall back to the earliest surviving dispatch directory.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Instant, SystemTime};

use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::Value;

use super::{
    Boss, BossApps, Found, Homes, NEGATIVE_TTL, antigravity, boss_key, file_name, is_uuid,
    session_cwd,
};

type Stamp = (u64, Option<SystemTime>);

struct CachedDbCwds {
    db: Stamp,
    wal: Option<Stamp>,
    by_key: Arc<HashMap<String, String>>,
}

static DB_CWDS: LazyLock<Mutex<HashMap<(&'static str, PathBuf), CachedDbCwds>>> =
    LazyLock::new(Mutex::default);

const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Working directory of each workspace's creating boss session when its record names one.
pub(super) fn of(
    apps: &BossApps,
    connection: &Connection,
    homes: impl FnOnce() -> Homes,
) -> HashMap<String, String> {
    let bosses = bosses(connection);
    if bosses.is_empty() {
        return HashMap::new();
    }
    let homes = homes();
    let mut found = apps.found.lock().unwrap_or_else(|p| p.into_inner());
    refresh(&mut found, &bosses, &homes);

    let mut out = HashMap::new();
    for boss in &bosses {
        if let Some(cwd) = boss_cwd(apps, boss, &found, &homes) {
            out.insert(boss.workspace.clone(), cwd);
        }
    }
    out
}

/// The creating boss of each workspace (and older conversations before `workspaces`), skipping
/// legacy handovers whose original boss was overwritten without saving `previous_host`.
fn bosses(connection: &Connection) -> Vec<Boss> {
    let mut bosses = HashMap::new();
    let has_origin = has_column(connection, "workspaces", "origin");
    let has_previous = has_column(connection, "workspaces", "previous_host")
        && has_column(connection, "workspaces", "previous_file");

    let mut replaced = HashSet::new();
    if let Ok(mut stmt) = connection.prepare("SELECT id FROM workspaces")
        && let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0))
    {
        for id in rows.flatten() {
            replaced.insert(id);
        }
    }

    // An older schema cannot tell the creating boss from one that took over, so its workspaces
    // read no boss directory and fall back to their earliest dispatch.
    let ws_sql = "SELECT id, \
         CASE WHEN previous_host IS NOT NULL THEN coalesce(origin, id) ELSE owner END, \
         CASE WHEN previous_host IS NOT NULL THEN previous_host ELSE owner_host END, \
         CASE WHEN previous_host IS NOT NULL THEN previous_file ELSE owner_file END \
         FROM workspaces \
         WHERE (previous_host IS NOT NULL AND previous_host != '') \
            OR (owner = coalesce(origin, id) AND owner_host IS NOT NULL)";
    let rows = if has_origin && has_previous {
        query_boss_rows(connection, ws_sql)
    } else {
        Vec::new()
    };
    for (workspace, key, host, file) in rows {
        bosses.entry(workspace.clone()).or_insert(Boss {
            workspace,
            key,
            host,
            file: file.map(PathBuf::from),
        });
    }

    let older_sql = "SELECT DISTINCT manager_session, manager_session, host, NULL \
                     FROM conversations \
                     WHERE manager_session IS NOT NULL AND host IS NOT NULL";
    for (workspace, key, host, file) in query_boss_rows(connection, older_sql) {
        if !replaced.contains(&workspace) {
            bosses.entry(workspace.clone()).or_insert(Boss {
                workspace,
                key,
                host,
                file: file.map(PathBuf::from),
            });
        }
    }
    bosses.into_values().collect()
}

fn has_column(connection: &Connection, table: &str, column: &str) -> bool {
    connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .is_ok_and(|cols| cols.iter().any(|name| name == column))
}

fn query_boss_rows(
    connection: &Connection,
    sql: &str,
) -> Vec<(String, String, String, Option<String>)> {
    connection
        .prepare(sql)
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect()
        })
        .unwrap_or_default()
}

fn is_other_transcript_host(host: &str) -> bool {
    matches!(host, "codebuddy" | "workbuddy" | "pi" | "deepseek")
}

fn is_swept_host(host: &str) -> bool {
    matches!(host, "claude" | "codex" | "grok")
}

fn refresh(found: &mut Found, bosses: &[Boss], homes: &Homes) {
    let mut missing_swept = false;
    let mut missing_other = HashSet::new();
    let mut missing_keys = Vec::new();
    for boss in bosses {
        let host = boss.host.as_str();
        if !is_swept_host(host) && !is_other_transcript_host(host) {
            continue;
        }
        if boss.file.as_ref().is_some_and(|file| file.is_file()) {
            continue;
        }
        let known = if is_other_transcript_host(host) {
            found.other_transcripts.get(&boss.key)
        } else {
            found.transcripts.get(&boss.key)
        };
        if let Some(path) = known {
            if path.is_file() {
                continue;
            }
            found.searched.remove(&boss.key);
        }
        if is_other_transcript_host(host) {
            missing_other.insert(boss.host.clone());
        } else {
            missing_swept = true;
        }
        missing_keys.push(boss.key.clone());
    }
    if missing_keys.is_empty() {
        return;
    }
    let roots_changed = found.homes.as_ref() != Some(homes);
    let now = Instant::now();
    if !roots_changed
        && missing_keys.iter().all(|key| {
            found
                .searched
                .get(key)
                .is_some_and(|at| now.duration_since(*at) < NEGATIVE_TTL)
        })
    {
        return;
    }
    if roots_changed {
        let antigravity_apps = std::mem::take(&mut found.antigravity);
        *found = super::sweep(homes);
        found.antigravity = antigravity_apps;
        found.homes = Some(homes.clone());
    } else {
        found
            .searched
            .retain(|_, at| now.duration_since(*at) < NEGATIVE_TTL);
        if missing_swept {
            let swept = super::sweep(homes);
            found.transcripts = swept.transcripts;
            found.desktop = swept.desktop;
            found.desktop_cwds = swept.desktop_cwds;
        }
    }
    if !missing_other.is_empty() {
        sweep_other(homes, &missing_other, &mut found.other_transcripts);
    }
    for key in missing_keys {
        found.searched.insert(key, now);
    }
}

fn sweep_other(homes: &Homes, wanted: &HashSet<String>, out: &mut HashMap<String, PathBuf>) {
    for (host, home) in &homes.other {
        if !wanted.contains(host) {
            continue;
        }
        match host.as_str() {
            "codebuddy" | "workbuddy" => {
                super::files_at(&home.join("projects"), 1, &mut |path| {
                    let name = file_name(path);
                    if let Some(id) = name.strip_suffix(".jsonl").filter(|id| is_uuid(id)) {
                        super::keep_newest(out, boss_key(host, id), path);
                    }
                });
            }
            "pi" => {
                super::files_at(&home.join("sessions"), 1, &mut |path| {
                    let name = file_name(path);
                    if let Some(stem) = name.strip_suffix(".jsonl")
                        && let Some((_, id)) = stem.rsplit_once('_')
                        && is_uuid(id)
                    {
                        super::keep_newest(out, boss_key("pi", id), path);
                    }
                });
            }
            "deepseek" => {
                super::files_at(&home.join("sessions"), 2, &mut |path| {
                    let name = file_name(path);
                    if name.starts_with("session")
                        && (name.ends_with(".jsonl")
                            || name.ends_with(".jsonl.zstd")
                            || name.ends_with(".jsonl.zst"))
                        && let Some(id) = path
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|dir| dir.to_str())
                            .filter(|id| !id.is_empty())
                    {
                        super::keep_newest(out, boss_key("deepseek", id), path);
                    }
                });
            }
            _ => {}
        }
    }
}

fn boss_cwd(apps: &BossApps, boss: &Boss, found: &Found, homes: &Homes) -> Option<String> {
    match boss.host.as_str() {
        "claude" => {
            let from_transcript = super::transcript(boss, found)
                .and_then(|file| cached_file_cwd(apps, file, session_cwd));
            from_transcript.or_else(|| {
                found
                    .desktop_cwds
                    .get(&boss.key)
                    .and_then(|raw| normalize_cwd(Path::new(raw)))
            })
        }
        "codex" => {
            super::transcript(boss, found).and_then(|file| cached_file_cwd(apps, file, session_cwd))
        }
        "grok" => super::transcript(boss, found).and_then(|record| {
            let summary = if file_name(record) == "summary.json" {
                record.to_owned()
            } else {
                record.parent()?.join("summary.json")
            };
            cached_file_cwd(apps, &summary, grok_summary_cwd)
        }),
        host if is_other_transcript_host(host) => boss
            .file
            .as_ref()
            .filter(|file| file.is_file())
            .or_else(|| found.other_transcripts.get(&boss.key))
            .and_then(|file| cached_file_cwd(apps, file, session_cwd)),
        "antigravity" => antigravity_cwd(boss, homes),
        "zcode" => zcode_cwd(boss, homes),
        "hermes" => hermes_cwd(boss, homes),
        _ => None,
    }
}

fn normalize_cwd(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let trimmed = text.trim();
    (!trimmed.is_empty() && Path::new(trimmed).is_absolute()).then(|| trimmed.to_owned())
}

fn cached_file_cwd(
    apps: &BossApps,
    file: &Path,
    read: fn(&Path) -> Option<PathBuf>,
) -> Option<String> {
    let meta = fs::metadata(file).ok()?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    if let Some((seen_len, seen_mod, cached)) = apps
        .cwds
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(file)
        .cloned()
        && seen_len == len
        && seen_mod == modified
    {
        return cached;
    }
    let resolved = read(file).and_then(|path| normalize_cwd(&path));
    apps.cwds
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(file.to_owned(), (len, modified, resolved.clone()));
    resolved
}

fn grok_summary_cwd(summary: &Path) -> Option<PathBuf> {
    let value: Value = serde_json::from_slice(&fs::read(summary).ok()?).ok()?;
    value["info"]["cwd"].as_str().map(PathBuf::from)
}

fn antigravity_cwd(boss: &Boss, homes: &Homes) -> Option<String> {
    let query = |root: &Path, key: &str| -> Option<String> {
        let db = root.join("conversation_summaries.db");
        db_cwds(
            "antigravity",
            &db,
            "SELECT conversation_id, workspace_uris FROM conversation_summaries \
             WHERE workspace_uris IS NOT NULL AND workspace_uris != '' AND workspace_uris != '[]'",
            antigravity::first_cwd,
        )
        .and_then(|map| map.get(key).cloned())
    };
    if let Some(record) = boss.file.as_deref()
        && let Some(root) = antigravity::root_from_record(record)
    {
        let key = antigravity::id_from_path(record)
            .map(|id| boss_key("antigravity", &id))
            .unwrap_or_else(|| boss.key.clone());
        if let Some(cwd) = query(&root, &key) {
            return Some(cwd);
        }
    }
    let cli_roots = antigravity::roots(homes);
    let desktop_roots = antigravity::desktop_roots(&cli_roots);
    let acp_roots = antigravity::acp_roots(homes);
    cli_roots
        .iter()
        .chain(desktop_roots.iter())
        .chain(acp_roots.iter())
        .find_map(|root| query(root, &boss.key))
}

fn zcode_cwd(boss: &Boss, homes: &Homes) -> Option<String> {
    let db = boss
        .file
        .as_deref()
        .filter(|path| path.is_file())
        .map(Path::to_owned)
        .or_else(|| other_home(homes, "zcode").map(|root| root.join("cli/db/db.sqlite")))
        .or_else(|| (!cfg!(test)).then(crate::usage::zcode::database))?;
    db_cwds(
        "zcode",
        &db,
        "SELECT id, directory FROM session WHERE directory IS NOT NULL AND directory != ''",
        |raw| Some(PathBuf::from(raw)),
    )
    .and_then(|map| map.get(&boss.key).cloned())
}

fn hermes_cwd(boss: &Boss, homes: &Homes) -> Option<String> {
    let db = boss
        .file
        .as_deref()
        .filter(|path| path.is_file())
        .map(Path::to_owned)
        .or_else(|| other_home(homes, "hermes").map(|root| root.join("state.db")))
        .or_else(|| (!cfg!(test)).then(default_hermes_db).flatten())?;
    db_cwds(
        "hermes",
        &db,
        "SELECT id, cwd FROM sessions WHERE cwd IS NOT NULL AND cwd != ''",
        |raw| Some(PathBuf::from(raw)),
    )
    .and_then(|map| map.get(&boss.key).cloned())
}

fn other_home(homes: &Homes, target: &str) -> Option<PathBuf> {
    homes
        .other
        .iter()
        .find(|(host, _)| host == target)
        .map(|(_, path)| path.clone())
}

fn default_hermes_db() -> Option<PathBuf> {
    let root = match std::env::var_os("HERMES_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = dirs::home_dir()?;
            #[cfg(windows)]
            if let Some(installed) = dirs::data_local_dir()
                .map(|local| local.join("hermes"))
                .filter(|dir| dir.is_dir())
            {
                installed
            } else {
                home.join(".hermes")
            }
            #[cfg(not(windows))]
            home.join(".hermes")
        }
    };
    let active = fs::read_to_string(root.join("active_profile"))
        .ok()
        .map(|raw| raw.trim().trim_start_matches('\u{feff}').to_owned())
        .filter(|name| {
            name != "default"
                && !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && name.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        })
        .map(|name| root.join("profiles").join(&name))
        .filter(|profile| {
            profile.is_dir()
                && !root
                    .join("profiles/.deleted")
                    .join(profile.file_name().unwrap_or_default())
                    .exists()
        });
    Some(active.unwrap_or(root).join("state.db"))
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

fn db_cwds(
    host: &'static str,
    db_path: &Path,
    sql: &str,
    parse: fn(&str) -> Option<PathBuf>,
) -> Option<Arc<HashMap<String, String>>> {
    let db_stamp = stamp(db_path)?;
    let mut wal_name = db_path.as_os_str().to_owned();
    wal_name.push("-wal");
    let wal_stamp = stamp(Path::new(&wal_name));
    let cache_key = (host, db_path.to_owned());
    {
        let cache = DB_CWDS.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = cache.get(&cache_key)
            && entry.db == db_stamp
            && entry.wal == wal_stamp
        {
            return Some(entry.by_key.clone());
        }
    }
    // A failed read (locked, corrupt or another schema) is remembered as empty until the files
    // change, so the bosses of one host do not each reopen it on every poll.
    let map = Arc::new(read_db_cwds(host, db_path, sql, parse).unwrap_or_default());
    DB_CWDS.lock().unwrap_or_else(|p| p.into_inner()).insert(
        cache_key,
        CachedDbCwds {
            db: db_stamp,
            wal: wal_stamp,
            by_key: map.clone(),
        },
    );
    Some(map)
}

fn read_db_cwds(
    host: &str,
    db_path: &Path,
    sql: &str,
    parse: fn(&str) -> Option<PathBuf>,
) -> Option<HashMap<String, String>> {
    let conn = open_read_only(db_path)?;
    let mut stmt = conn.prepare(sql).ok()?;
    let mapped = stmt
        .query_map([], |row| Ok((col_string(row, 0), col_string(row, 1))))
        .ok()?;
    let mut out = HashMap::new();
    for (id, raw) in mapped.flatten() {
        if id.is_empty() {
            continue;
        }
        if let Some(cwd) = parse(&raw).and_then(|path| normalize_cwd(&path)) {
            out.insert(boss_key(host, &id), cwd);
        }
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
    let mut wal_name = db_path.as_os_str().to_owned();
    wal_name.push("-wal");
    let conn = if Path::new(&wal_name).exists() {
        Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?
    } else {
        let uri = immutable_uri(db_path)?;
        Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .ok()
        .or_else(|| {
            Connection::open_with_flags(
                db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()
        })?
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
            ' ' => encoded.push_str("%20"),
            _ => encoded.push(ch),
        }
    }
    encoded.push_str("?immutable=1");
    Some(encoded)
}

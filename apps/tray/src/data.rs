//! Studio snapshots (ADR-0008) and the tray-only management routes (ADR-0012).
mod uninstall;

mod assets;
mod orphaned;
mod request;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::data_files::valid_id;
use crate::response::Response;
use boss_app::BossApps;

mod alerts;
pub(crate) mod snapshot;
use snapshot::{Item, Overview, Snapshot, call_counts};
pub(crate) mod boss_app;

/// Studio rewrites live files every 1.5s; older ones were left by a server that crashed.
const LIVE_FRESH: Duration = Duration::from_secs(10);
const VERSION: u64 = 1;
/// Live files of this version carry only reply tails, laid over the saved views.
const LIVE_TAILS: u64 = 2;
/// Route answers are reused while their inputs stay the same, at most this long, since the
/// stage's two-hour window moves with the clock.
const ROUTE_REUSE: Duration = Duration::from_secs(30);
include!(concat!(env!("OUT_DIR"), "/office_art.rs"));

pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; media-src 'self' blob: http://127.0.0.1:*; base-uri 'none'; frame-src 'self'; frame-ancestors 'self'";
/// The terminal page alone also allows inline styles, which xterm.js needs (ADR-0103).
pub const TERMINAL_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; media-src 'self' blob: http://127.0.0.1:*; base-uri 'none'; frame-src 'self'; frame-ancestors 'self'";

/// (mtime, size, content hash, parsed value) of one JSON file.
type Cached = (SystemTime, u64, u64, Option<Arc<Value>>);

/// (view revision, parsed view) of one conversation.
type CachedView = (i64, Arc<Value>);

/// What identifies a route answer: snapshots, stage top-5 IDs (for single-group routes),
/// whether the data root exists, the reuse window, running limit, handovers, boss apps, CLI titles and the titles the user renamed.
type RouteKey = (
    Vec<crate::stage::SourceKey>,
    Vec<String>,
    bool,
    u64,
    usize,
    Vec<(String, crate::stage::Handover)>,
    Vec<(String, &'static str)>,
    (Vec<(String, boss_app::CliTitle)>, BTreeSet<String>),
    Vec<crate::isolated::Dir>,
    Vec<(String, crate::projects::Project)>,
    Vec<crate::stage::WorkspaceLink>,
    Vec<crate::projects::ManualProject>,
);

/// A route answer with the views it was computed from, held so their addresses stay unique.
type RouteAnswer = (RouteKey, Vec<Arc<Value>>, Response);

/// One server's publication of running tasks' and seats' reply tails and reported models.
struct Live {
    payload: Arc<Value>,
    updated_at: Option<String>,
}

/// A live view built once per input: the saved views and live payload it came from (held so
/// their addresses stay unique), the live time, and the result.
type Merged = (Arc<Value>, Arc<Value>, Option<String>, Arc<Value>);

pub struct Reader {
    uninstalling: std::sync::atomic::AtomicBool,
    root: PathBuf,
    projects_dir: PathBuf,
    hosts: Mutex<Option<crate::hosts::Hosts>>,
    boss_homes: Mutex<boss_app::Homes>,
    tools: Arc<Mutex<Option<crate::tool_dirs::ToolDirs>>>,
    /// Parsed views by conversation, keyed on the row's revision, so polling parses only
    /// views that changed.
    summaries: Mutex<HashMap<String, CachedView>>,
    /// Display-only recovery, reused by revision only after a fresh ownership probe.
    orphaned: Mutex<HashMap<String, CachedView>>,
    /// Parsed live files, keyed the same way; several routes poll within one publication.
    live_files: Mutex<HashMap<PathBuf, Cached>>,
    /// Live views by conversation, so unchanged inputs keep one Arc for the route caches.
    merged: Mutex<HashMap<String, Merged>>,
    routes: Mutex<HashMap<String, RouteAnswer>>,
    alerts: Mutex<alerts::Cache>,
    boss_apps: BossApps,
}

impl Reader {
    pub fn new(root: PathBuf) -> Self {
        let projects_dir = if cfg!(test) {
            root.join("tray-config")
        } else {
            crate::preferences::dir()
        };
        Self {
            uninstalling: std::sync::atomic::AtomicBool::new(false),
            root,
            projects_dir,
            hosts: Mutex::new(None),
            boss_homes: Mutex::new(boss_app::Homes::here()),
            tools: Arc::default(),
            summaries: Mutex::new(HashMap::new()),
            orphaned: Mutex::new(HashMap::new()),
            live_files: Mutex::new(HashMap::new()),
            merged: Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
            alerts: Mutex::new(HashMap::new()),
            boss_apps: BossApps::new(),
        }
    }

    pub fn overview(&self) -> Overview {
        self.scan().0
    }

    /// Share parsed saved views across all polling routes; live files are always reread.
    fn scan(&self) -> (Overview, Vec<Snapshot>) {
        let database = crate::db::read(&self.root);
        self.scan_with(&database)
    }

    pub(super) fn scan_with(
        &self,
        database: &Result<Option<rusqlite::Connection>, String>,
    ) -> (Overview, Vec<Snapshot>) {
        let mut overview = Overview::default();
        let rows = match database {
            Ok(Some(connection)) => view_rows(connection),
            Ok(None) => {
                overview.missing = true;
                Ok(Vec::new())
            }
            Err(error) => Err(error.clone()),
        };
        let rows = rows.unwrap_or_else(|error| {
            overview.scan_error = Some(error.clone());
            overview
                .unavailable
                .push((crate::db::path(&self.root).display().to_string(), error));
            Vec::new()
        });
        let mut live = self.live();
        let mut merged = self.merged.lock().unwrap_or_else(|p| p.into_inner());
        let mut shown = HashSet::new();
        let mut cache = self.summaries.lock().unwrap_or_else(|p| p.into_inner());
        let mut seen = HashSet::new();
        let mut snapshots = Vec::new();
        let mut items = Vec::new();
        for (id, rev, written_ms) in rows {
            if !valid_id(&id) {
                continue;
            }
            seen.insert(id.clone());
            let saved = match cache.get(&id) {
                Some((cached, value)) if *cached == rev => Some(value.clone()),
                _ => {
                    let value = database
                        .as_ref()
                        .ok()
                        .and_then(Option::as_ref)
                        .and_then(|connection| view_body(connection, &id))
                        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                        .filter(|value| version(value) == Some(VERSION))
                        .filter(|value| summary(value).is_some_and(|item| item.id == id))
                        .map(Arc::new);
                    if let Some(value) = &value {
                        cache.insert(id.clone(), (rev, value.clone()));
                    } else {
                        cache.remove(&id);
                    }
                    value
                }
            };
            let current = live.remove(&id).and_then(|live| {
                let view = live_view(&mut merged, &id, saved.as_ref(), &live)?;
                shown.insert(id.clone());
                Some((view, live.updated_at))
            });
            let is_live = current.is_some();
            let modified = if is_live {
                SystemTime::now()
            } else {
                UNIX_EPOCH + Duration::from_millis(u64::try_from(written_ms).unwrap_or(0))
            };
            let (view, live_updated) = match current {
                Some((view, updated_at)) => (Some(view), updated_at),
                None => (
                    saved.map(|saved| {
                        database.as_ref().ok().and_then(Option::as_ref).map_or_else(
                            || saved.clone(),
                            |connection| orphaned::view(self, connection, &id, &saved),
                        )
                    }),
                    None,
                ),
            };
            if let Some(view) = view
                && let Some(item) = summary(&view).filter(|item| item.id == id)
            {
                items.push((modified, item.clone()));
                snapshots.push(Snapshot {
                    item,
                    view,
                    live: is_live,
                    live_updated,
                });
            } else {
                overview.unreadable.insert(id.clone());
                overview.unavailable.push((
                    format!("{}#{id}", crate::db::path(&self.root).display()),
                    "视图快照无法读取或版本不受支持".to_owned(),
                ));
            }
        }
        merged.retain(|id, _| shown.contains(id));
        cache.retain(|id, _| seen.contains(id));
        self.orphaned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|id, _| seen.contains(id));
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        overview.items = items.into_iter().map(|(_, item)| item).collect();
        overview.unavailable.sort();
        for snapshot in &snapshots {
            let (running, review) = call_counts(snapshot);
            overview.running += running;
            overview.review += review;
        }
        (overview, snapshots)
    }

    /// Stage and chat answers, reused while relevant snapshots and the reuse window are unchanged.
    fn route(&self, path: &str) -> Response {
        let database = crate::db::read(&self.root);
        let (overview, snapshots) = self.scan_with(&database);
        let window = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / ROUTE_REUSE.as_secs();
        let limit = crate::settings::max_running(&self.root) as usize;
        let (handovers, links, apps, titles) = match &database {
            Ok(Some(connection)) => (
                handovers(connection),
                links(connection),
                self.boss_apps.of(connection, || self.boss_homes()),
                self.boss_apps.titles(connection, || self.boss_homes()),
            ),
            _ => (None, None, Vec::new(), Vec::new()),
        };
        // Records of directories are only forgotten when every workspace was read.
        let complete = !overview.missing
            && overview.scan_error.is_none()
            && overview.unreadable.is_empty()
            && handovers.is_some()
            && links.is_some();
        let handovers = handovers.unwrap_or_default();
        let links = links.unwrap_or_default();
        let handover_map: HashMap<_, _> = handovers.iter().cloned().collect();
        let app_map: HashMap<_, _> = apps.iter().cloned().collect();
        let title_map: HashMap<_, _> = titles.iter().cloned().collect();
        let renamed = crate::manage::renamed_titles(&self.projects_dir);
        let data_exists = self.root.is_dir();
        let mut dirs = crate::isolated::read(&self.root);
        let mut projects = crate::projects::lookup(
            &crate::stage::origin_cwds(&snapshots, &handover_map),
            complete,
        );
        let manual_projects = if path == "/app/chats" {
            crate::projects::manual::list(&self.projects_dir)
        } else {
            Vec::new()
        };
        let (sources, stage_ids, key_handovers, key_apps, key_titles, key_links, views) =
            if let Some(group_id) = crate::stage::requested_group(path) {
                dirs.retain(|dir| dir.workspace == group_id);
                projects.retain(|id, _| id == group_id);
                let rel_links: Vec<_> = links
                    .iter()
                    .filter(|l| {
                        l.source == group_id
                            || l.target.as_deref() == Some(group_id)
                            || (l.target.is_none() && l.target_boss == group_id)
                    })
                    .cloned()
                    .collect();
                let mut watched = HashSet::from([group_id.to_owned()]);
                for l in &rel_links {
                    watched.insert(l.source.clone());
                    if let Some(target) = &l.target {
                        watched.insert(target.clone());
                    }
                }
                let stage_ids = crate::stage::stage_ids_of(&snapshots, &handover_map);
                let mut sources = Vec::new();
                let mut views = Vec::new();
                for s in &snapshots {
                    if crate::stage::chat_group(&s.item).is_some_and(|id| watched.contains(&id)) {
                        sources.push(crate::stage::source_key(s));
                        views.push(s.view.clone());
                    }
                }
                let h = handovers
                    .into_iter()
                    .filter(|(id, _)| watched.contains(id))
                    .collect();
                let a = apps
                    .into_iter()
                    .filter(|(id, _)| watched.contains(id))
                    .collect();
                let t = titles
                    .into_iter()
                    .filter(|(id, _)| watched.contains(id))
                    .collect();
                (sources, stage_ids, h, a, t, rel_links, views)
            } else {
                let sources = snapshots.iter().map(crate::stage::source_key).collect();
                let views = snapshots.iter().map(|s| s.view.clone()).collect();
                (
                    sources,
                    Vec::new(),
                    handovers,
                    apps,
                    titles,
                    links.clone(),
                    views,
                )
            };
        let key = (
            sources,
            stage_ids,
            data_exists,
            window,
            limit,
            key_handovers,
            key_apps,
            (key_titles, renamed.clone()),
            dirs.clone(),
            projects.clone().into_iter().collect(),
            key_links,
            manual_projects.clone(),
        );
        let mut routes = self.routes.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((saved, _, response)) = routes.get(path)
            && *saved == key
        {
            return response.clone();
        }
        let response = crate::stage::respond(
            path,
            snapshots,
            data_exists,
            limit,
            &handover_map,
            &app_map,
            &crate::stage::Places {
                dirs: &dirs,
                projects: &projects,
                links: &links,
                manual_projects: &manual_projects,
                titles: &title_map,
                renamed: &renamed,
            },
        );
        // Focus routes vary; keep the few most recently asked for, not every group ever opened.
        if routes.len() >= 8 && !routes.contains_key(path) {
            routes.clear();
        }
        routes.insert(path.to_owned(), (key, views, response.clone()));
        response
    }

    /// Running work from servers that wrote within LIVE_FRESH.
    fn live(&self) -> HashMap<String, Live> {
        let mut cache = self.live_files.lock().unwrap_or_else(|p| p.into_inner());
        let mut seen = HashSet::new();
        let mut found: Vec<(SystemTime, HashMap<String, Live>)> = Vec::new();
        let now = SystemTime::now();
        for entry in fs::read_dir(self.root.join("live"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let Some(meta) = entry.metadata().ok() else {
                continue;
            };
            let Some(modified) = meta.modified().ok() else {
                continue;
            };
            // Small clock skew counts as fresh; far-future mtimes after a backward clock jump do not.
            if path.extension().is_none_or(|ext| ext != "json") || !live_fresh(modified, now) {
                continue;
            }
            seen.insert(path.clone());
            let Some(payload) =
                cached(&mut cache, &path, &meta, |v| version(v) == Some(LIVE_TAILS))
            else {
                continue;
            };
            let Some(Value::Object(conversations)) = payload.get("conversations") else {
                continue;
            };
            let ids: Vec<String> = conversations.keys().cloned().collect();
            // Content changes only when necessary; heartbeats refresh its mtime.
            let updated_at = Some(crate::time_format::iso(
                modified
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros() as i64,
            ));
            found.push((
                modified,
                ids.into_iter()
                    .map(|id| {
                        let live = Live {
                            payload: payload.clone(),
                            updated_at: updated_at.clone(),
                        };
                        (id, live)
                    })
                    .collect(),
            ));
        }
        cache.retain(|path, _| seen.contains(path));
        found.sort_by_key(|(modified, _)| *modified);
        found
            .into_iter()
            .flat_map(|(_, conversations)| conversations)
            .collect()
    }
}

fn live_fresh(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age <= LIVE_FRESH)
        || modified
            .duration_since(now)
            .is_ok_and(|ahead| ahead <= LIVE_FRESH)
}

/// The action header and JSON body of a write, for a route that has already checked its method.
/// A page in the interface language the user picked.
fn page(html: &[u8]) -> Response {
    let general = crate::preferences::general::current();
    Response::ok(
        "text/html; charset=utf-8",
        crate::preferences::localize(html, &general),
    )
}

fn live_view(
    merged: &mut HashMap<String, Merged>,
    id: &str,
    saved: Option<&Arc<Value>>,
    live: &Live,
) -> Option<Arc<Value>> {
    let view = live.payload.get("conversations")?.get(id)?;
    let base = saved?.clone();
    if let Some((held, payload, at, out)) = merged.get(id)
        && Arc::ptr_eq(payload, &live.payload)
        && *at == live.updated_at
        && Arc::ptr_eq(held, &base)
    {
        return Some(out.clone());
    }
    let out = Arc::new(lay_tails(&base, view, live.updated_at.as_deref()));
    let entry = (
        base,
        live.payload.clone(),
        live.updated_at.clone(),
        out.clone(),
    );
    merged.insert(id.to_owned(), entry);
    Some(out)
}

/// A parsed JSON file from the cache, read again only when its mtime or size changed and
/// parsed again only when its content did: live heartbeats refresh the mtime alone.
fn cached(
    cache: &mut HashMap<PathBuf, Cached>,
    path: &Path,
    meta: &fs::Metadata,
    accept: impl Fn(&Value) -> bool,
) -> Option<Arc<Value>> {
    let modified = meta.modified().unwrap_or(UNIX_EPOCH);
    let len = meta.len();
    if let Some((m, l, _, value)) = cache.get(path)
        && *m == modified
        && *l == len
    {
        return value.clone();
    }
    let bytes = fs::read(path).ok();
    let hash = bytes.as_deref().map_or(0, |bytes| {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    });
    if let Some((m, l, h, value)) = cache.get_mut(path)
        && bytes.is_some()
        && *l == len
        && *h == hash
    {
        *m = modified;
        return value.clone();
    }
    let value = bytes
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|value| accept(value))
        .map(Arc::new);
    cache.insert(path.to_path_buf(), (modified, len, hash, value.clone()));
    value
}

/// Every saved view's conversation, revision and write time (milliseconds since the epoch).
fn view_rows(connection: &rusqlite::Connection) -> Result<Vec<(String, i64, i64)>, String> {
    let mut statement = connection
        .prepare("SELECT conversation_id, rev, written_ms FROM views")
        .map_err(|e| e.to_string())?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .and_then(Iterator::collect)
        .map_err(|e| e.to_string())
}

/// Workspaces waiting for, or led by, another boss (ADR-0033), including one handed back to
/// its creator.
/// Handed-over workspaces; None when they could not be read.
fn handovers(connection: &rusqlite::Connection) -> Option<Vec<(String, crate::stage::Handover)>> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .ok()?;
    let origin = if version >= 3 {
        "coalesce(origin, id)"
    } else {
        "id"
    };
    let linked = if version >= 4 {
        " OR EXISTS (SELECT 1 FROM workspace_links WHERE source_workspace = workspaces.id)"
    } else {
        ""
    };
    let rows = connection
        .prepare(&format!(
            "SELECT id, owner != {origin} OR previous_host IS NOT NULL, owner_host, pending, \
             note, released, previous_host, title, cwd \
             FROM workspaces \
             WHERE pending = 1 OR owner != {origin} OR previous_host IS NOT NULL{linked} \
             ORDER BY id",
        ))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        crate::stage::Handover {
                            changed: row.get(1)?,
                            host: row.get(2)?,
                            pending: row.get(3)?,
                            note: row.get(4)?,
                            released: row.get(5)?,
                            previous_host: row.get(6)?,
                            title: row.get(7)?,
                            cwd: row.get(8)?,
                        },
                    ))
                })?
                .collect()
        });
    rows.ok()
}

fn links(connection: &rusqlite::Connection) -> Option<Vec<crate::stage::WorkspaceLink>> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .ok()?;
    if version < 4 {
        return Some(Vec::new());
    }
    connection
        .prepare(
            "SELECT source_workspace, target_boss, target_workspace, target_host, claimed_at \
             FROM workspace_links ORDER BY claimed_at ASC, source_workspace ASC, target_boss ASC",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok(crate::stage::WorkspaceLink {
                        source: row.get(0)?,
                        target_boss: row.get(1)?,
                        target: row.get(2)?,
                        target_host: row.get(3)?,
                        claimed_at: row.get(4)?,
                    })
                })?
                .collect()
        })
        .ok()
}

fn view_body(connection: &rusqlite::Connection, id: &str) -> Option<String> {
    connection
        .query_row(
            "SELECT body FROM views WHERE conversation_id = ?1",
            [id],
            |row| row.get(0),
        )
        .ok()
}

fn version(value: &Value) -> Option<u64> {
    value.get("version").and_then(Value::as_u64)
}

/// What the CLI is doing now, when its reply last grew and its latest tool names; each is
/// absent from older servers.
pub(crate) fn copy_progress(target: &mut Value, source: &Value) {
    for key in ["doing", "doing_since", "output_at"] {
        if let Some(value) = source[key].as_str() {
            target[key] = json!(value);
        }
    }
    if source["tools"].is_array() {
        target["tools"] = source["tools"].clone();
    }
}

/// The saved views with running work's reply tails, reported models and elapsed time laid
/// over them. Only work the saved views still show as running takes a tail.
fn lay_tails(saved: &Value, tails: &Value, updated_at: Option<&str>) -> Value {
    fn lay(target: &mut Value, tail: &Value) {
        target["output"] = tail["output"].clone();
        if let Some(model) = tail["reported_model"].as_str() {
            target["reported_model"] = json!(model);
        }
        // A reported model can rename the member while it runs; absent from older servers.
        if tail["identity"].is_object() {
            target["identity"] = tail["identity"].clone();
        }
        copy_progress(target, tail);
    }
    let now = updated_at.and_then(crate::time_format::timestamp);
    let mut view = saved.clone();
    if let Some(tasks) = view
        .pointer_mut("/director/tasks")
        .and_then(Value::as_array_mut)
    {
        for task in tasks {
            let Some(tail) = task["id"].as_str().and_then(|id| tails["tasks"].get(id)) else {
                continue;
            };
            if task["status"] != "running" {
                continue;
            }
            lay(task, tail);
            if let (Some(now), Some(start)) = (
                now,
                task["time"]
                    .as_str()
                    .and_then(crate::time_format::timestamp),
            ) {
                task["elapsed_ms"] = json!((now - start).max(0) / 1000);
            }
        }
    }
    if let Some(seats) = view
        .pointer_mut("/director/round/seats")
        .and_then(Value::as_array_mut)
    {
        for seat in seats {
            let Some(tail) = seat["seat_id"]
                .as_str()
                .and_then(|id| tails["seats"].get(id))
            else {
                continue;
            };
            if seat["status"] == "running" {
                lay(seat, tail);
            }
        }
    }
    view
}

fn summary(snapshot: &Value) -> Option<Item> {
    if snapshot.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return None;
    }
    let summary = snapshot.get("summary")?;
    let text = |key: &str| summary.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Item {
        id: text("id")?,
        title: text("title").unwrap_or_default(),
        mode: text("mode").unwrap_or_default(),
        host: text("host"),
        manager_session: text("manager_session"),
        status: text("status").unwrap_or_default(),
        updated: text("updated").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests;

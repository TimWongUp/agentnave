//! Studio snapshots (ADR-0008) and the tray-only management routes (ADR-0012).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use boss_app::BossApps;

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

const PANEL_HTML: &[u8] = include_bytes!("../ui/panel.html");
const PANEL_JS: &[u8] = include_bytes!("../ui/panel.js");
const PANEL_CSS: &[u8] = include_bytes!("../ui/panel.css");
const MANAGE_CSS: &[u8] = include_bytes!("../ui/manage.css");
const TO_OFFICE: &[u8] =
    br#"<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=/chat/">"#;
const SETTINGS_HTML: &[u8] = include_bytes!("../ui/settings.html");
const SETTINGS_JS: &[u8] = include_bytes!("../ui/settings.js");
const SETTINGS_CSS: &[u8] = include_bytes!("../ui/settings.css");
const CHAT_HTML: &[u8] = include_bytes!("../ui/chat.html");
const CHAT_JS: &[u8] = include_bytes!("../ui/chat.js");
const CHAT_CSS: &[u8] = include_bytes!("../ui/chat.css");
const COMMON_JS: &[u8] = concat!(
    "\"use strict\";\nconst AGENTNAVE_ART_VERSION = \"",
    env!("CARGO_PKG_VERSION"),
    "\";\n",
    include_str!("../ui/common.js")
)
.as_bytes();
const HOSTS_JS: &[u8] = include_bytes!("../ui/hosts.js");
const HOSTS_CSS: &[u8] = include_bytes!("../ui/hosts.css");
const ONBOARDING_CSS: &[u8] = include_bytes!("../ui/onboarding.css");
const MOCK_JS: &[u8] = include_bytes!("../ui/mock.js");

pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'";

/// Same rule as the Studio MCP server: `AGENTNAVE_DATA_DIR`, else `~/.agentnave`.
pub fn data_root() -> PathBuf {
    match std::env::var_os("AGENTNAVE_DATA_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir().unwrap_or_default().join(".agentnave"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub host: Option<String>,
    pub manager_session: Option<String>,
    pub status: String,
    /// Last activity as Studio recorded it (RFC 3339); empty in older snapshots.
    pub updated: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Overview {
    /// Most recently changed first.
    pub items: Vec<Item>,
    pub unavailable: Vec<(String, String)>,
    pub missing: bool,
    pub running: usize,
    pub review: usize,
}

impl Overview {
    pub fn count(&self, status: &str) -> usize {
        match status {
            "running" => self.running,
            "review" => self.review,
            _ => self
                .items
                .iter()
                .filter(|item| item.status == status)
                .count(),
        }
    }
}

#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub cache_control: &'static str,
}

impl Response {
    pub(crate) fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            cache_control: "no-store",
            status: 200,
            content_type,
            body,
        }
    }

    pub fn json(value: &serde_json::Value) -> Self {
        Self::ok(
            "application/json; charset=utf-8",
            value.to_string().into_bytes(),
        )
    }

    fn not_found() -> Self {
        Self {
            cache_control: "no-store",
            status: 404,
            content_type: "text/plain; charset=utf-8",
            body: b"not found".to_vec(),
        }
    }

    pub fn error(status: u16, message: &str) -> Self {
        let mut response = Self::json(&json!({"error": message}));
        response.status = status;
        response
    }
}

/// (mtime, size, content hash, parsed value) of one JSON file.
type Cached = (SystemTime, u64, u64, Option<Arc<Value>>);

/// (view revision, parsed view) of one conversation.
type CachedView = (i64, Option<Arc<Value>>);

/// What identifies a route answer: each snapshot's view, whether the data root exists, and
/// the reuse window.
// Snapshots, whether the data root exists, the reuse window and the running limit.
type RouteKey = (
    Vec<crate::stage::SourceKey>,
    bool,
    u64,
    usize,
    Vec<(String, crate::stage::Handover)>,
    Vec<(String, &'static str)>,
);

/// A route answer with the views it was computed from, held so their addresses stay unique.
type RouteAnswer = (RouteKey, Vec<Arc<Value>>, Response);

/// One server's publication for a conversation: version 1 holds its whole projection,
/// version 2 (`tails`) only running tasks' and seats' reply tails and reported models.
struct Live {
    payload: Arc<Value>,
    tails: bool,
    updated_at: Option<String>,
}

/// A live view built once per input: the saved views and live payload it came from (held so
/// their addresses stay unique), the live time, and the result.
type Merged = (Option<Arc<Value>>, Arc<Value>, Option<String>, Arc<Value>);

pub(crate) struct Snapshot {
    pub item: Item,
    pub view: Arc<Value>,
    pub live: bool,
    pub live_updated: Option<String>,
}

pub struct Reader {
    root: PathBuf,
    hosts: Mutex<Option<crate::hosts::Hosts>>,
    tools: Arc<Mutex<Option<crate::hosts::ToolDirs>>>,
    /// Parsed views by conversation, keyed on the row's revision, so polling parses only
    /// views that changed.
    summaries: Mutex<HashMap<String, CachedView>>,
    /// Parsed live files, keyed the same way; several routes poll within one publication.
    live_files: Mutex<HashMap<PathBuf, Cached>>,
    /// Live views by conversation, so unchanged inputs keep one Arc for the route caches.
    merged: Mutex<HashMap<String, Merged>>,
    routes: Mutex<HashMap<String, RouteAnswer>>,
    boss_apps: BossApps,
}

impl Reader {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            hosts: Mutex::new(None),
            tools: Arc::default(),
            summaries: Mutex::new(HashMap::new()),
            live_files: Mutex::new(HashMap::new()),
            merged: Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
            boss_apps: BossApps::new(),
        }
    }

    /// Writes require an explicit POST and action header; navigation cannot mutate files.
    pub fn request(&self, method: &str, path: &str, action: Option<&str>, body: &[u8]) -> Response {
        if path == "/app/hosts"
            || path.starts_with("/app/hosts/")
            || path == "/app/onboarding"
            || path == "/app/onboarding/dismiss"
            || path == "/app/onboarding/reopen"
        {
            return self.host_request(method, path, action, body);
        }
        if path == "/app/skills" {
            if method != "GET" {
                return Response::error(405, "此路由只接受 GET");
            }
            return Response::json(&crate::hosts::skill_catalog());
        }
        if let Some(family) = path.strip_prefix("/app/characters/") {
            return crate::characters::request(&self.root, family, method, action, body);
        }
        if path == "/app/review" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate_review,
                    crate::members::save_review,
                )
            });
        }
        if path == "/app/permissions" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate_permissions,
                    crate::members::save_permissions,
                )
            });
        }
        if path == "/app/cli-options" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate_cli_options,
                    crate::members::save_cli_options,
                )
            });
        }
        if path == "/app/members" {
            return post(method, action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::members::validate,
                    crate::members::save,
                )
            });
        }
        if path == "/app/settings" {
            if method == "GET" {
                return Response::json(&crate::settings::state(&self.root));
            }
            if method != "POST" {
                return Response::error(405, "此路由只接受 GET 或 POST");
            }
            return with_payload(action, body, |payload| {
                save(
                    &self.root,
                    &payload,
                    crate::settings::validate,
                    crate::settings::save,
                )
            });
        }
        if path == "/app/orchestration" {
            if method == "GET" {
                return self.orchestration();
            }
            if method != "POST" {
                return Response::error(405, "此路由只接受 GET 或 POST");
            }
            return with_payload(action, body, |payload| self.save_orchestration(&payload));
        }
        if path == "/app/rules" || path == "/app/rules/open" {
            let home = dirs::home_dir().unwrap_or_default();
            let apps = self
                .hosts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map(crate::hosts::Hosts::rule_apps)
                .unwrap_or_default();
            if path == "/app/rules" {
                if method != "GET" {
                    return Response::error(405, "此路由只接受 GET");
                }
                return Response::json(&self.rule_files());
            }
            return post(method, action, body, |payload| {
                match crate::rules::open(&apps, &home, payload["path"].as_str().unwrap_or_default())
                {
                    Ok(result) => Response::json(&result),
                    Err(error) => Response::error(400, &error),
                }
            });
        }
        if path == "/app/conversations/rename" {
            return post(method, action, body, |payload| {
                let (Some(id), Some(title)) = (
                    payload["id"].as_str().filter(|id| valid_id(id)),
                    payload["title"].as_str(),
                ) else {
                    return Response::error(400, "需要有效的会话 ID 与标题");
                };
                Response::json(&crate::manage::rename(&self.root, id, title))
            });
        }
        if path == "/app/workspaces/handover" {
            return post(method, action, body, |payload| {
                let Some(group) = payload["group"].as_str().filter(|id| valid_id(id)) else {
                    return Response::error(400, "需要有效的工作区 ID");
                };
                Response::json(&crate::manage::handover(&self.root, group))
            });
        }
        if path == "/app/workspaces/forget" {
            return post(method, action, body, |payload| {
                let Some(group) = payload["group"].as_str().filter(|id| valid_id(id)) else {
                    return Response::error(400, "需要有效的工作区 ID");
                };
                Response::json(&crate::manage::forget(&self.root, group))
            });
        }
        if path == "/app/conversations/delete" {
            return post(method, action, body, |payload| {
                self.delete_conversations(&payload)
            });
        }
        if method != "GET" {
            return Response::error(405, "此路由只接受 GET");
        }
        self.respond(path)
    }

    fn orchestration(&self) -> Response {
        let (document, error) = match crate::members::read_orchestration(&self.root) {
            Ok(document) => (document, Value::Null),
            Err(error) => (json!({"version": 1, "preset": "off"}), json!(error)),
        };
        let rules = self.rule_files();
        Response::json(&json!({"document": document, "error": error,
            "presets": crate::rules::presets(), "files": rules["files"], "bosses": rules["bosses"]}))
    }

    fn save_orchestration(&self, payload: &Value) -> Response {
        // The modes each row runs on are worked out here, never taken from the page.
        let mut document = payload.clone();
        if let Some(fields) = document.as_object_mut() {
            fields.remove("effective_apps");
        }
        if let Err(error) = crate::members::validate_orchestration(&document) {
            return Response::error(400, &error);
        }
        let guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        document["effective_apps"] = guard
            .as_ref()
            .map(|hosts| hosts.effective_apps(&document))
            .unwrap_or_else(|| json!({}));
        if let Err(error) = crate::members::save_orchestration(&self.root, &document) {
            return Response::error(500, &error);
        }
        let failures = guard
            .as_ref()
            .map(|hosts| self.sync_rules(hosts, &[]))
            .unwrap_or_default();
        drop(guard);
        let rules = self.rule_files();
        Response::json(&json!({"saved": true, "failures": failures,
            "files": rules["files"], "bosses": rules["bosses"]}))
    }

    fn delete_conversations(&self, payload: &Value) -> Response {
        let Some(ids) = payload.get("ids").and_then(Value::as_array).filter(|ids| {
            !ids.is_empty() && ids.iter().all(|id| id.as_str().is_some_and(valid_id))
        }) else {
            return Response::error(400, "ids 必须为非空的有效会话 ID 数组");
        };
        let cutoff = match payload.get("updated_before") {
            None => None,
            Some(value) => match value.as_i64().filter(|cutoff| *cutoff > 0) {
                Some(cutoff) => Some(cutoff),
                None => return Response::error(400, "updated_before 必须是有效的毫秒时间"),
            },
        };
        let results: Vec<Value> = match cutoff {
            Some(cutoff) => crate::manage::cleanup(
                &self.root,
                &ids.iter()
                    .map(|id| id.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>(),
                cutoff,
            ),
            None => ids
                .iter()
                .map(|id| crate::manage::delete(&self.root, id.as_str().unwrap()))
                .collect(),
        };
        Response::json(&json!({"results": results}))
    }

    /// This phase only runs the bounded shell probe, never reads host configuration.
    pub(crate) fn probe_tools(&self, context: &crate::hosts::Context) -> crate::hosts::Shell {
        let (shell, dirs) = crate::hosts::probe_tools(context);
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(dirs);
        shell
    }

    #[cfg(test)]
    pub(crate) fn use_hosts(&self, hosts: crate::hosts::Hosts) {
        *self.hosts.lock().unwrap() = Some(hosts);
    }

    pub fn configure_hosts(&self, context: crate::hosts::Context, shell: crate::hosts::Shell) {
        let mut guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(crate::hosts::Hosts::from_probe(
            context,
            Some(shell),
            self.tools.clone(),
        ));
    }

    fn boss_homes(&self) -> boss_app::Homes {
        let hosts = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        hosts
            .as_ref()
            .map_or_else(boss_app::Homes::here, crate::hosts::Hosts::boss_homes)
    }

    pub fn refresh_hosts(&self) {
        if let Some(hosts) = self
            .hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            hosts.refresh_probed();
            self.sync_rules(hosts, &[]);
        }
    }

    /// Bring the orchestration block and the modes the server reads per row in line with the
    /// settings and the connected apps. A broken orchestration.json leaves the files alone; a
    /// preview build never writes them.
    fn sync_rules(&self, hosts: &crate::hosts::Hosts, also: &[std::path::PathBuf]) -> Vec<Value> {
        if crate::PREVIEW {
            return Vec::new();
        }
        let Ok(mut document) = crate::members::read_orchestration(&self.root) else {
            return Vec::new();
        };
        let mut failures = Vec::new();
        let effective = hosts.effective_apps(&document);
        if self.root.join("orchestration.json").is_file()
            && document.get("effective_apps") != Some(&effective)
        {
            document["effective_apps"] = effective;
            if let Err(error) = crate::members::save_orchestration(&self.root, &document) {
                failures
                    .push(json!({"path": self.root.join("orchestration.json"), "error": error}));
            }
        }
        failures.extend(hosts.sync_rules(&document, also));
        failures
    }

    /// The global rules files with whether each carries the current orchestration block.
    fn rule_files(&self) -> Value {
        let home = dirs::home_dir().unwrap_or_default();
        let guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        let (installed, connected) = guard
            .as_ref()
            .map(|hosts| (hosts.rule_apps(), hosts.connected_rule_apps()))
            .unwrap_or_default();
        let document = crate::members::read_orchestration(&self.root)
            .unwrap_or_else(|_| json!({"preset": "off"}));
        crate::rules::list(&installed, &connected, &home, &document)
    }

    pub(crate) fn tool_dirs(&self) -> Option<crate::hosts::ToolDirs> {
        self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn host_request(
        &self,
        method: &str,
        path: &str,
        action: Option<&str>,
        body: &[u8],
    ) -> Response {
        let read = matches!(path, "/app/hosts" | "/app/onboarding");
        let write = matches!(
            path,
            "/app/hosts/refresh"
                | "/app/hosts/connect"
                | "/app/hosts/remove"
                | "/app/hosts/root"
                | "/app/hosts/confirm-reload"
                | "/app/hosts/skill"
                | "/app/hosts/reveal-backups"
                | "/app/onboarding/dismiss"
                | "/app/onboarding/reopen"
        );
        if !read && !write {
            return Response::error(404, "未知路由");
        }
        if (read && method != "GET") || (write && method != "POST") {
            return Response::error(405, "此路由不接受该方法");
        }
        if write && action != Some("1") {
            return Response::error(403, "缺少操作请求头");
        }
        let mut guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        let Some(hosts) = guard.as_mut() else {
            return Response::error(503, "宿主接入尚未初始化");
        };
        match path {
            "/app/hosts" => {
                if !crate::PREVIEW {
                    hosts.remember_reloads();
                }
                Response::json(&hosts.snapshot())
            }
            "/app/hosts/refresh" => {
                let snapshot = hosts.refresh();
                self.sync_rules(hosts, &[]);
                Response::json(&snapshot)
            }
            "/app/onboarding" => Response::json(&crate::onboarding::state(hosts, &self.root)),
            "/app/onboarding/dismiss" => match crate::onboarding::dismiss(hosts) {
                Ok(result) => Response::json(&result),
                Err(error) => Response::error(500, &error),
            },
            "/app/onboarding/reopen" => match crate::onboarding::reopen(hosts) {
                Ok(result) => Response::json(&result),
                Err(error) => Response::error(500, &error),
            },
            "/app/hosts/reveal-backups" => match hosts.reveal_backups() {
                Ok(result) => Response::json(&result),
                Err(error) => Response::error(500, &error),
            },
            _ => {
                let Ok(payload) = serde_json::from_slice::<Value>(body) else {
                    return Response::error(400, "请求必须为 JSON");
                };
                let operation = path.rsplit('/').next().unwrap();
                // A moved host's old rules files still carry the block; clean them after the move.
                let before = if operation == "root" {
                    hosts.rule_paths()
                } else {
                    Vec::new()
                };
                let result = hosts.action(operation, &payload);
                // Connecting, removing or moving a host changes which rules files carry the block.
                if matches!(operation, "connect" | "remove" | "root") {
                    self.sync_rules(hosts, &before);
                }
                match result {
                    Ok(result) => Response::json(&result),
                    Err(error) => Response::error(400, &error),
                }
            }
        }
    }

    /// `GET /settings/members`, re-probing the CLIs when `refresh` is set.
    pub fn members(&self, refresh: bool) -> Response {
        Response::json(&crate::members::state(&self.root, refresh))
    }

    pub fn overview(&self) -> Overview {
        self.scan().0
    }

    /// Share parsed saved views across all polling routes; live files are always reread.
    fn scan(&self) -> (Overview, Vec<Snapshot>) {
        let mut overview = Overview::default();
        let database = crate::db::read(&self.root);
        let rows = match &database {
            Ok(Some(connection)) => view_rows(connection),
            Ok(None) => {
                overview.missing = true;
                Ok(Vec::new())
            }
            Err(error) => Err(error.clone()),
        };
        let rows = rows.unwrap_or_else(|error| {
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
                Some((cached, value)) if *cached == rev => value.clone(),
                _ => {
                    let value = database
                        .as_ref()
                        .ok()
                        .and_then(Option::as_ref)
                        .and_then(|connection| view_body(connection, &id))
                        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                        .filter(|value| version(value) == Some(VERSION))
                        .map(Arc::new);
                    cache.insert(id.clone(), (rev, value.clone()));
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
            let readable = saved.is_some();
            let (view, live_updated) = match current {
                Some((view, updated_at)) => (Some(view), updated_at),
                None => (saved, None),
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
            } else if !readable {
                overview.unavailable.push((
                    format!("{}#{id}", crate::db::path(&self.root).display()),
                    "视图快照无法读取或版本不受支持".to_owned(),
                ));
            }
        }
        // A version 1 live publication can precede the first disk snapshot.
        for (id, live) in live {
            if !valid_id(&id) {
                continue;
            }
            let Some(view) = live_view(&mut merged, &id, None, &live) else {
                continue;
            };
            if let Some(item) = summary(&view).filter(|item| item.id == id) {
                shown.insert(id.clone());
                items.push((SystemTime::now(), item.clone()));
                snapshots.push(Snapshot {
                    item,
                    view,
                    live: true,
                    live_updated: live.updated_at,
                });
            }
        }
        merged.retain(|id, _| shown.contains(id));
        cache.retain(|id, _| seen.contains(id));
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

    /// Serve one path of the `agentnave` scheme: the dashboard assets and its `state` JSON.
    pub fn respond(&self, path: &str) -> Response {
        if let Some(name) = path.strip_prefix("/art/") {
            if name.contains("..") {
                return Response::not_found();
            }
            return match ART.iter().find(|(key, _)| *key == name) {
                Some((_, bytes)) => {
                    let mime = match name.rsplit('.').next() {
                        Some("webp") => "image/webp",
                        Some("svg") => "image/svg+xml",
                        Some("png") => "image/png",
                        Some("json") => "application/json",
                        _ => "application/octet-stream",
                    };
                    let mut response = Response::ok(mime, bytes.to_vec());
                    response.cache_control = "public, max-age=31536000";
                    response
                }
                None => Response::not_found(),
            };
        }
        if path == "/app/stage" || path == "/app/chats" || path.starts_with("/app/chats/") {
            return self.route(path);
        }
        if path == "/app/characters" {
            return Response::json(&crate::characters::list(&self.root));
        }
        if let Some(name) = path.strip_prefix("/character/") {
            return crate::characters::frame(&self.root, name).unwrap_or_else(Response::not_found);
        }
        if path == "/avatar-settings" {
            return match crate::avatars::read(&self.root) {
                Ok(value) => Response::json(&value),
                Err(error) => Response::error(500, &error),
            };
        }
        if let Some((mime, bytes)) = crate::avatars::asset(path.trim_start_matches('/')) {
            return Response::ok(mime, bytes.to_vec());
        }
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if let ["chat", resource] = parts.as_slice() {
            return match *resource {
                "chat.js" => Response::ok("text/javascript; charset=utf-8", CHAT_JS.to_vec()),
                "actions.js" => Response::ok(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../ui/actions.js").to_vec(),
                ),
                "chat.css" => Response::ok("text/css; charset=utf-8", CHAT_CSS.to_vec()),
                "onboarding.css" => {
                    Response::ok("text/css; charset=utf-8", ONBOARDING_CSS.to_vec())
                }
                "common.js" => Response::ok("text/javascript; charset=utf-8", COMMON_JS.to_vec()),
                "mock.js" => Response::ok("text/javascript; charset=utf-8", MOCK_JS.to_vec()),
                "" => Response::ok("text/html; charset=utf-8", CHAT_HTML.to_vec()),
                id if valid_id(id) => Response::ok("text/html; charset=utf-8", CHAT_HTML.to_vec()),
                _ => Response::not_found(),
            };
        }
        if let ["settings", resource] = parts.as_slice() {
            return match *resource {
                "" => Response::ok("text/html; charset=utf-8", SETTINGS_HTML.to_vec()),
                "avatar-settings.js" => Response::ok(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../ui/avatar-settings.js").to_vec(),
                ),
                "advanced.js" => Response::ok(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../ui/advanced.js").to_vec(),
                ),
                "settings.js" => {
                    Response::ok("text/javascript; charset=utf-8", SETTINGS_JS.to_vec())
                }
                "settings.css" => Response::ok("text/css; charset=utf-8", SETTINGS_CSS.to_vec()),
                "hosts.js" => Response::ok("text/javascript; charset=utf-8", HOSTS_JS.to_vec()),
                "hosts.css" => Response::ok("text/css; charset=utf-8", HOSTS_CSS.to_vec()),
                "skills.js" => Response::ok(
                    "text/javascript; charset=utf-8",
                    include_bytes!("../ui/skills.js").to_vec(),
                ),
                _ => Response::not_found(),
            };
        }
        if let ["manage", resource] = parts.as_slice() {
            return match *resource {
                // Conversations are managed in the office now; old links lead there.
                "" => Response::ok("text/html; charset=utf-8", TO_OFFICE.to_vec()),
                "manage.css" => Response::ok("text/css; charset=utf-8", MANAGE_CSS.to_vec()),
                "state" => Response::json(&crate::manage::list(&self.root, self.overview())),
                _ => Response::not_found(),
            };
        }
        if let ["panel", resource] = parts.as_slice() {
            return match *resource {
                "" => Response::ok("text/html; charset=utf-8", PANEL_HTML.to_vec()),
                "panel.js" => Response::ok("text/javascript; charset=utf-8", PANEL_JS.to_vec()),
                "panel.css" => Response::ok("text/css; charset=utf-8", PANEL_CSS.to_vec()),
                "onboarding.css" => {
                    Response::ok("text/css; charset=utf-8", ONBOARDING_CSS.to_vec())
                }
                "common.js" => Response::ok("text/javascript; charset=utf-8", COMMON_JS.to_vec()),
                "mock.js" => Response::ok("text/javascript; charset=utf-8", MOCK_JS.to_vec()),
                _ => Response::not_found(),
            };
        }
        match parts.as_slice() {
            // The old workbench address now leads to the chat page.
            [""] => Response::ok("text/html; charset=utf-8", TO_OFFICE.to_vec()),
            _ => Response::not_found(),
        }
    }

    /// Stage and chat answers, reused while every snapshot and the reuse window are unchanged.
    fn route(&self, path: &str) -> Response {
        let snapshots = self.scan().1;
        let window = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / ROUTE_REUSE.as_secs();
        let limit = crate::settings::max_running(&self.root) as usize;
        let (handovers, apps) = match crate::db::read(&self.root) {
            Ok(Some(connection)) => (
                handovers(&connection),
                self.boss_apps.of(&connection, || self.boss_homes()),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let key = (
            snapshots.iter().map(crate::stage::source_key).collect(),
            self.root.is_dir(),
            window,
            limit,
            handovers,
            apps,
        );
        let mut routes = self.routes.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((saved, _, response)) = routes.get(path)
            && *saved == key
        {
            return response.clone();
        }
        let views = snapshots.iter().map(|s| s.view.clone()).collect();
        let handovers = key.4.iter().cloned().collect();
        let apps = key.5.iter().cloned().collect();
        let response = crate::stage::respond(path, snapshots, key.1, limit, &handovers, &apps);
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
            // A clock running behind the file's mtime counts as fresh.
            if path.extension().is_none_or(|ext| ext != "json")
                || SystemTime::now()
                    .duration_since(modified)
                    .is_ok_and(|age| age > LIVE_FRESH)
            {
                continue;
            }
            seen.insert(path.clone());
            let Some(payload) = cached(&mut cache, &path, &meta, |v| {
                matches!(version(v), Some(VERSION | LIVE_TAILS))
            }) else {
                continue;
            };
            let Some(Value::Object(conversations)) = payload.get("conversations") else {
                continue;
            };
            let ids: Vec<String> = conversations.keys().cloned().collect();
            let tails = version(&payload) == Some(LIVE_TAILS);
            // Version 2 rewrites only changed content and refreshes its mtime otherwise.
            let updated_at = if tails {
                Some(crate::stage::iso(
                    modified
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as i64,
                ))
            } else {
                payload["updated_at"].as_str().map(str::to_owned)
            };
            found.push((
                modified,
                ids.into_iter()
                    .map(|id| {
                        let live = Live {
                            payload: payload.clone(),
                            tails,
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

/// One conversation's live view: a version 1 projection as published, or version 2 tails laid
/// over the saved views, which they need beneath them. Reused while its inputs are the same.
/// A write route: an explicit POST with the action header and a JSON body.
fn post(
    method: &str,
    action: Option<&str>,
    body: &[u8],
    handle: impl FnOnce(Value) -> Response,
) -> Response {
    if method != "POST" {
        return Response::error(405, "此操作只接受 POST");
    }
    with_payload(action, body, handle)
}

/// The action header and JSON body of a write, for a route that has already checked its method.
fn with_payload(
    action: Option<&str>,
    body: &[u8],
    handle: impl FnOnce(Value) -> Response,
) -> Response {
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return Response::error(400, "请求必须为 JSON");
    };
    handle(payload)
}

/// A rejected document is the caller's error; a failed save is ours.
fn save<T>(
    root: &Path,
    payload: &Value,
    validate: impl FnOnce(&Value) -> Result<T, String>,
    save: impl FnOnce(&Path, &Value) -> Result<(), String>,
) -> Response {
    if let Err(error) = validate(payload) {
        return Response::error(400, &error);
    }
    match save(root, payload) {
        Ok(()) => Response::json(&json!({"saved": true})),
        Err(error) => Response::error(500, &error),
    }
}

fn live_view(
    merged: &mut HashMap<String, Merged>,
    id: &str,
    saved: Option<&Arc<Value>>,
    live: &Live,
) -> Option<Arc<Value>> {
    let view = live.payload.get("conversations")?.get(id)?;
    let base = if live.tails {
        Some(saved?.clone())
    } else if summary(view).is_some() {
        None
    } else {
        return None;
    };
    if let Some((held, payload, at, out)) = merged.get(id)
        && Arc::ptr_eq(payload, &live.payload)
        && *at == live.updated_at
        && held.as_ref().map(Arc::as_ptr) == base.as_ref().map(Arc::as_ptr)
    {
        return Some(out.clone());
    }
    let out = Arc::new(match &base {
        Some(saved) => lay_tails(saved, view, live.updated_at.as_deref()),
        None => view.clone(),
    });
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
/// its creator; none before a server added the table.
fn handovers(connection: &rusqlite::Connection) -> Vec<(String, crate::stage::Handover)> {
    // This connection reads only, so it cannot add what an older server's database lacks.
    let labels = match crate::db::workspace_has(connection, "title") {
        Ok(true) => "title, cwd",
        _ => "NULL, NULL",
    };
    let rows = connection
        .prepare(&format!(
            "SELECT id, owner != id OR previous_host IS NOT NULL, owner_host, pending, \
             note, released, previous_host, {labels} \
             FROM workspaces WHERE pending = 1 OR owner != id OR previous_host IS NOT NULL \
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
    rows.unwrap_or_default()
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

/// The saved views with running work's reply tails, reported models and elapsed time laid
/// over them. Only work the saved views still show as running takes a tail.
fn lay_tails(saved: &Value, tails: &Value, updated_at: Option<&str>) -> Value {
    fn lay(target: &mut Value, tail: &Value) {
        target["output"] = tail["output"].clone();
        if let Some(model) = tail["reported_model"].as_str() {
            target["reported_model"] = json!(model);
        }
        // What the CLI is doing now and when its reply last grew; absent from older servers.
        for key in ["doing", "output_at"] {
            if let Some(value) = tail[key].as_str() {
                target[key] = json!(value);
            }
        }
    }
    let now = updated_at.and_then(crate::stage::timestamp);
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
            if let (Some(now), Some(start)) =
                (now, task["time"].as_str().and_then(crate::stage::timestamp))
            {
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

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Count actual invocations, never conversations or merged desk identities.
pub(crate) fn call_counts(snapshot: &Snapshot) -> (usize, usize) {
    let view = &snapshot.view["director"];
    let tasks = view["tasks"].as_array().into_iter().flatten();
    let seats = view["round"]["seats"].as_array().into_iter().flatten();
    let mut running = 0;
    let mut review = 0;
    for call in tasks.chain(seats) {
        if call["review"] == "pending" || call["status"] == "review" {
            review += 1;
        } else if call["status"] == "running" {
            running += 1;
        }
    }
    (running, review)
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

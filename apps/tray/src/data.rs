//! Studio snapshots (ADR-0008) and the tray-only management routes (ADR-0012).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

/// Studio rewrites live files every 1.5s; older ones were left by a server that crashed.
const LIVE_FRESH: Duration = Duration::from_secs(10);
const VERSION: u64 = 1;

/// The dashboard front end has one source, shared with the browser view.
macro_rules! asset {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/agentnave/dashboard_assets/",
            $name
        ))
    };
}

const PANEL_HTML: &[u8] = include_bytes!("../ui/panel.html");
const PANEL_JS: &[u8] = include_bytes!("../ui/panel.js");
const PANEL_CSS: &[u8] = include_bytes!("../ui/panel.css");
const MANAGE_HTML: &[u8] = include_bytes!("../ui/manage.html");
const MANAGE_JS: &[u8] = include_bytes!("../ui/manage.js");
const MANAGE_CSS: &[u8] = include_bytes!("../ui/manage.css");
const SETTINGS_HTML: &[u8] = include_bytes!("../ui/settings.html");
const SETTINGS_JS: &[u8] = include_bytes!("../ui/settings.js");
const SETTINGS_CSS: &[u8] = include_bytes!("../ui/settings.css");
const CHAT_HTML: &[u8] = include_bytes!("../ui/chat.html");
const CHAT_JS: &[u8] = include_bytes!("../ui/chat.js");
const CHAT_CSS: &[u8] = include_bytes!("../ui/chat.css");
const COMMON_JS: &[u8] = include_bytes!("../ui/common.js");
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
}

impl Overview {
    pub fn count(&self, status: &str) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == status)
            .count()
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
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

type Cached = (SystemTime, u64, Option<Arc<Value>>);

struct Live {
    view: Value,
    updated_at: Option<String>,
}

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
    /// Parsed views by path, keyed on (mtime, size), so polling parses only changed files.
    summaries: Mutex<HashMap<PathBuf, Cached>>,
}

impl Reader {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            hosts: Mutex::new(None),
            tools: Arc::default(),
            summaries: Mutex::new(HashMap::new()),
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
        if path == "/app/avatars" {
            return crate::avatars::request(&self.root, method, action, body);
        }
        if path == "/app/members" {
            if method != "POST" {
                return Response::error(405, "此操作只接受 POST");
            }
            if action != Some("1") {
                return Response::error(403, "缺少操作请求头");
            }
            let Ok(payload) = serde_json::from_slice::<Value>(body) else {
                return Response::error(400, "请求必须为 JSON");
            };
            if let Err(error) = crate::members::validate(&payload) {
                return Response::error(400, &error);
            }
            return match crate::members::save(&self.root, &payload) {
                Ok(()) => Response::json(&json!({"saved": true})),
                Err(error) => Response::error(500, &error),
            };
        }
        if path == "/app/conversations/delete" {
            if method != "POST" {
                return Response::error(405, "此操作只接受 POST");
            }
            if action != Some("1") {
                return Response::error(403, "缺少操作请求头");
            }
            let Ok(payload) = serde_json::from_slice::<Value>(body) else {
                return Response::error(400, "请求必须为 JSON");
            };
            let Some(ids) = payload.get("ids").and_then(Value::as_array).filter(|ids| {
                !ids.is_empty() && ids.iter().all(|id| id.as_str().is_some_and(valid_id))
            }) else {
                return Response::error(400, "ids 必须为非空的有效会话 ID 数组");
            };
            let results: Vec<Value> = ids
                .iter()
                .map(|id| crate::manage::delete(&self.root, id.as_str().unwrap()))
                .collect();
            return Response::json(&json!({"results": results}));
        }
        if method != "GET" {
            return Response::error(405, "此路由只接受 GET");
        }
        self.respond(path)
    }

    /// This phase only runs the bounded shell probe, never reads host configuration.
    pub(crate) fn probe_tools(&self, context: &crate::hosts::Context) -> crate::hosts::Shell {
        let (shell, dirs) = crate::hosts::probe_tools(context);
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = Some(dirs);
        shell
    }

    pub fn configure_hosts(&self, context: crate::hosts::Context, shell: crate::hosts::Shell) {
        let mut guard = self.hosts.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(crate::hosts::Hosts::from_probe(
            context,
            Some(shell),
            self.tools.clone(),
        ));
    }

    pub fn refresh_hosts(&self) {
        if let Some(hosts) = self
            .hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            hosts.refresh_probed();
        }
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
            "/app/hosts" => Response::json(&hosts.snapshot()),
            "/app/hosts/refresh" => Response::json(&hosts.refresh()),
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
                match hosts.action(operation, &payload) {
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

    /// Share parsed disk snapshots across all polling routes; live files are always reread.
    fn scan(&self) -> (Overview, Vec<Snapshot>) {
        let entries = fs::read_dir(self.root.join("conversations"));
        let mut overview = Overview {
            missing: entries.is_err(),
            ..Overview::default()
        };
        let mut live = self.live();
        let mut cache = self.summaries.lock().unwrap_or_else(|p| p.into_inner());
        let mut seen = HashSet::new();
        let mut snapshots = Vec::new();
        let mut items = Vec::new();
        for entry in entries.into_iter().flatten().flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            let directory = entry.path();
            if !valid_id(&id) || !directory.is_dir() {
                continue;
            }
            let path = directory.join("views.json");
            let disk = fs::metadata(&path).ok().map(|meta| {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let fresh = matches!(cache.get(&path), Some((m, len, _)) if *m == modified && *len == meta.len());
                if !fresh {
                    cache.insert(path.clone(), (modified, meta.len(), read_json(&path).map(Arc::new)));
                }
                seen.insert(path.clone());
                (modified, cache[&path].2.clone())
            });
            let current = live.remove(&id).filter(|v| summary(&v.view).is_some());
            let is_live = current.is_some();
            let live_updated = current.as_ref().and_then(|v| v.updated_at.clone());
            let modified = if is_live {
                SystemTime::now()
            } else {
                disk.as_ref().map_or(SystemTime::UNIX_EPOCH, |d| d.0)
            };
            let view = current
                .map(|live| Arc::new(live.view))
                .or_else(|| disk.as_ref().and_then(|d| d.1.clone()));
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
            } else if disk.is_some() || directory.join("state.json").exists() {
                overview.unavailable.push((
                    path.display().to_string(),
                    if disk.is_some() {
                        "视图快照无法读取或版本不受支持"
                    } else {
                        "尚无视图快照：任一工作室服务启动后会补写"
                    }
                    .to_owned(),
                ));
            }
        }
        // Atomic live publication can precede the first disk snapshot.
        for (id, live) in live {
            if let Some(item) = summary(&live.view).filter(|item| item.id == id && valid_id(&id)) {
                items.push((SystemTime::now(), item.clone()));
                snapshots.push(Snapshot {
                    item,
                    view: Arc::new(live.view),
                    live: true,
                    live_updated: live.updated_at,
                });
            }
        }
        cache.retain(|path, _| seen.contains(path));
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        overview.items = items.into_iter().map(|(_, item)| item).collect();
        overview.unavailable.sort();
        (overview, snapshots)
    }

    /// Serve one path of the `agentnave` scheme: the dashboard assets and its `state` JSON.
    pub fn respond(&self, path: &str) -> Response {
        if path == "/app/stage" || path == "/app/chats" || path.starts_with("/app/chats/") {
            return crate::stage::respond(path, self.scan().1, self.root.is_dir());
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
                "settings.js" => {
                    Response::ok("text/javascript; charset=utf-8", SETTINGS_JS.to_vec())
                }
                "settings.css" => Response::ok("text/css; charset=utf-8", SETTINGS_CSS.to_vec()),
                "hosts.js" => Response::ok("text/javascript; charset=utf-8", HOSTS_JS.to_vec()),
                "hosts.css" => Response::ok("text/css; charset=utf-8", HOSTS_CSS.to_vec()),
                _ => Response::not_found(),
            };
        }
        if let ["manage", resource] = parts.as_slice() {
            return match *resource {
                "" => Response::ok("text/html; charset=utf-8", MANAGE_HTML.to_vec()),
                "manage.js" => Response::ok("text/javascript; charset=utf-8", MANAGE_JS.to_vec()),
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
            // Host avatars, shared with the browser view.
            ["avatars.png"] => Response::ok("image/png", asset!("avatars.png").to_vec()),
            // The old workbench address now leads to the chat page.
            [""] => Response::ok(
                "text/html; charset=utf-8",
                br#"<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=/chat/">"#
                    .to_vec(),
            ),
            _ => Response::not_found(),
        }
    }

    /// Snapshots of running conversations from servers that wrote within LIVE_FRESH.
    fn live(&self) -> HashMap<String, Live> {
        let mut found: Vec<(SystemTime, HashMap<String, Live>)> = Vec::new();
        for entry in fs::read_dir(self.root.join("live"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let Some(modified) = entry.metadata().ok().and_then(|meta| meta.modified().ok()) else {
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
            if let Some(mut payload) = read_json(&path) {
                let updated_at = payload["updated_at"].as_str().map(str::to_owned);
                if let Some(Value::Object(conversations)) =
                    payload.get_mut("conversations").map(Value::take)
                {
                    found.push((
                        modified,
                        conversations
                            .into_iter()
                            .map(|(id, view)| {
                                (
                                    id,
                                    Live {
                                        view,
                                        updated_at: updated_at.clone(),
                                    },
                                )
                            })
                            .collect(),
                    ));
                }
            }
        }
        found.sort_by_key(|(modified, _)| *modified);
        found
            .into_iter()
            .flat_map(|(_, conversations)| conversations)
            .collect()
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A versioned snapshot document, or None when missing, unreadable or of another version.
fn read_json(path: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (value.get("version").and_then(Value::as_u64) == Some(VERSION)).then_some(value)
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
mod tests {
    use super::*;

    fn snapshot(id: &str, status: &str) -> Value {
        json!({
            "version": 1,
            "summary": {"id": id, "title": format!("T {id}"), "mode": "show", "status": status, "updated": "2026-10-15T00:00:00+00:00"},
            "director": {"view": "director", "mode": "show", "status": status, "messages": [{"text": "hi"}]},
            "public": {"view": "public", "mode": "show", "messages": [{"text": "hi"}]},
        })
    }

    fn write(path: &Path, value: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    }

    fn state(reader: &Reader, path: &str) -> Value {
        let response = reader.respond(path);
        assert_eq!(response.status, 200, "{path}");
        serde_json::from_slice(&response.body).unwrap()
    }

    #[test]
    fn fresh_live_files_override_saved_views() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("conversations/a1/views.json"),
            &snapshot("a1", "finished"),
        );
        write(
            &root.join("conversations/b2/views.json"),
            &json!({"version": 2}),
        );
        fs::write(root.join("conversations/b2/state.json"), "{}").unwrap();
        fs::create_dir_all(root.join("conversations/c3")).unwrap();
        fs::write(root.join("conversations/c3/state.json"), "{}").unwrap();
        let reader = Reader::new(root.to_path_buf());

        let overview = reader.overview();
        assert_eq!(overview.items.len(), 1);
        assert_eq!(overview.unavailable.len(), 2);
        assert_eq!(reader.overview().items[0].status, "finished");

        let live = root.join("live/1-x.json");
        write(
            &live,
            &json!({"version": 1, "conversations": {"a1": snapshot("a1", "running")}}),
        );
        assert_eq!(reader.overview().count("running"), 1);
        assert_eq!(reader.overview().items[0].status, "running");

        // A crashed server's file stops counting once it is older than LIVE_FRESH.
        let old = SystemTime::now() - LIVE_FRESH - Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&live)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(reader.overview().count("running"), 0);
        assert_eq!(reader.overview().items[0].status, "finished");
    }

    #[test]
    fn routes_serve_views_and_reject_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("conversations/a1/views.json"),
            &snapshot("a1", "review"),
        );
        let mut task = snapshot("t1", "succeeded");
        task["director"]["mode"] = json!("task");
        task["public"] = Value::Null;
        write(&root.join("conversations/t1/views.json"), &task);
        let reader = Reader::new(root.to_path_buf());

        assert!(String::from_utf8_lossy(&reader.respond("/").body).contains("url=/chat/"));
        assert_eq!(reader.respond("/avatars.png").status, 200);
        assert_eq!(reader.respond("/panel/").status, 200);
        for path in [
            "/manage/",
            "/manage/manage.js",
            "/manage/manage.css",
            "/settings/",
            "/settings/settings.js",
            "/settings/settings.css",
            "/chat/",
            "/chat/missing-group",
            "/chat/chat.js",
            "/chat/chat.css",
            "/chat/common.js",
            "/chat/mock.js",
            "/panel/common.js",
            "/panel/mock.js",
        ] {
            assert_eq!(reader.respond(path).status, 200, "{path}");
        }
        assert_eq!(
            reader.respond("/chat/").body,
            reader.respond("/chat/missing-group").body
        );
        assert_eq!(
            reader.respond("/panel/common.js").body,
            reader.respond("/chat/common.js").body
        );
        for path in [
            "/state",
            "/c/a1/",
            "/p/a1/state",
            "/p/a1/transcript.jsonl",
            "/p/t1/state",
            "/c/t1/transcript.jsonl",
            "/transcript.jsonl",
            "/c/../state",
            "/c/a1",
            "/c/zz/state",
            "/x/a1/state",
            "/panel/state",
            "/manage/unknown",
            "/chat/not-a-file.js",
            "/chat/group/extra",
        ] {
            assert_eq!(reader.respond(path).status, 404, "{path}");
        }
    }

    const MANAGER: &str = "s-0123456789abcdef0123456789abcdef";

    fn task_snapshot(id: &str, status: &str, updated: &str) -> Value {
        json!({"version": 1, "summary": {"id": id, "title": format!("任务 {id}"),
            "mode": "task", "status": status, "updated": updated, "host": "claude", "manager_session": MANAGER},
            "director": {"mode": "task", "tasks": [{"id": format!("t-{id}"), "provider": "codex", "model": "m",
                "time": updated, "status": status, "prompt": "请检查", "output": "# 结果\n\n**最后一行**", "elapsed_ms": 0}]}})
    }

    fn save(root: &Path, value: &Value) {
        write(
            &root
                .join("conversations")
                .join(value["summary"]["id"].as_str().unwrap())
                .join("views.json"),
            value,
        );
    }

    #[test]
    fn chats_separate_host_sessions_and_do_not_merge_legacy_connections() {
        let dir = tempfile::tempdir().unwrap();
        for (id, session) in [
            ("a", MANAGER),
            ("b", MANAGER),
            ("c", "s-ffffffffffffffffffffffffffffffff"),
            ("old-a", "0123456789abcdef0123456789abcdef"),
            ("old-b", "0123456789abcdef0123456789abcdef"),
        ] {
            let mut item = task_snapshot(id, "succeeded", "2026-10-15T00:00:00Z");
            item["summary"]["manager_session"] = json!(session);
            save(dir.path(), &item);
        }
        let reader = Reader::new(dir.path().to_path_buf());
        let groups = state(&reader, "/app/chats");
        assert_eq!(groups["groups"].as_array().unwrap().len(), 4);
        assert_eq!(
            state(&reader, &format!("/app/chats/{MANAGER}"))["conversations"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            state(&reader, "/app/chats/c-old-a")["conversations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            state(&reader, "/app/chats/c-old-b")["conversations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn chats_aggregate_sessions_merge_latest_members_and_reflect_saved_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        save(
            root,
            &task_snapshot("z", "succeeded", "2026-10-15T02:00:01.000001Z"),
        );
        // A different offset still denotes a newer activity than z, even within the same millisecond.
        let newer = task_snapshot("b", "blocked", "2026-10-15T10:00:01.000002+08:00");
        save(root, &newer);
        let mut legacy = task_snapshot("legacy", "succeeded", "2026-10-15T00:00:00Z");
        legacy["summary"]["manager_session"] = Value::Null;
        legacy["summary"]["host"] = Value::Null;
        legacy["director"]["tasks"][0]["model"] = Value::Null;
        legacy["director"]["tasks"][0]["time"] = json!("2026-10-14T23:59:59.750123Z");
        legacy["director"]["tasks"][0]["elapsed_ms"] = json!(1500);
        save(root, &legacy);
        let reader = Reader::new(root.to_path_buf());
        let groups = state(&reader, "/app/chats");
        assert_eq!(groups["groups"].as_array().unwrap().len(), 2);
        assert_eq!(groups["groups"][0]["group"], MANAGER);
        assert_eq!(groups["groups"][0]["conversations"], 2);
        assert_eq!(groups["groups"][0]["title"], "任务 b");
        assert_eq!(groups["groups"][0]["label"], "Claude Code");
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        assert_eq!(chat["members"].as_array().unwrap().len(), 1);
        assert_eq!(chat["members"][0]["conversation"], "b");
        assert_eq!(chat["members"][0]["status"], "blocked");
        assert_eq!(chat["conversations"][0]["id"], "z");
        let legacy = state(&reader, "/app/chats/c-legacy");
        assert_eq!(legacy["host"], Value::Null);
        assert_eq!(legacy["label"], "未记录主控");
        assert_eq!(legacy["members"][0]["key"], "codex/");
        assert_eq!(legacy["members"][0]["line"], "结果");
        assert_eq!(legacy["messages"][1]["time"], "2026-10-15T00:00:01.250123Z");
        assert!(legacy["color"].as_u64().unwrap() < 8);
        assert_eq!(state(&reader, &format!("/app/chats/{MANAGER}")), chat);
        assert_eq!(
            state(&reader, "/app/chats/c-legacy")["color"],
            legacy["color"]
        );
        let mut changed = newer;
        changed["director"]["tasks"][0]["error"] = json!("**需要授权**");
        save(root, &changed);
        assert_eq!(
            state(&reader, &format!("/app/chats/{MANAGER}"))["members"][0]["line"],
            "受阻：需要授权"
        );
    }

    #[test]
    fn chat_live_overrides_disk_and_typing_is_unique_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        save(
            root,
            &task_snapshot("a", "succeeded", "2026-10-15T01:00:00Z"),
        );
        let mut a = task_snapshot("a", "running", "2026-10-15T02:00:00Z");
        a["director"]["tasks"][0]["output"] = json!("one\ntwo\nthree\nfour\n**five**\n");
        let b = task_snapshot("b", "running", "2026-10-15T01:30:00Z");
        // A newer finished call of the same member must not hide another active call's typing.
        save(
            root,
            &task_snapshot("c", "succeeded", "2026-10-15T03:00:00Z"),
        );
        let live = root.join("live/1-fresh.json");
        write(
            &live,
            &json!({"version": 1, "conversations": {"a": a, "b": b}}),
        );
        let reader = Reader::new(root.to_path_buf());
        let path = format!("/app/chats/{MANAGER}");
        let chat = state(&reader, &path);
        assert_eq!(chat["live"], true);
        assert_eq!(chat["status"], "running");
        assert_eq!(chat["conversations"].as_array().unwrap().len(), 3);
        assert_eq!(chat["members"][0]["conversation"], "c");
        let messages = chat["messages"].as_array().unwrap();
        // Each running thread keeps its own bubble, keyed by desk and conversation.
        let typing_a = |messages: &[Value]| {
            messages
                .iter()
                .find(|m| m["id"] == "typing:codex|m|:a:")
                .cloned()
                .unwrap()
        };
        assert_eq!(messages.iter().filter(|m| m["kind"] == "typing").count(), 2);
        assert!(messages.iter().any(|m| m["id"] == "typing:codex|m|:b:"));
        assert_eq!(typing_a(messages)["text"], "two\nthree\nfour\n**five**");
        a["director"]["tasks"][0]["output"] = json!("new tail");
        write(
            &live,
            &json!({"version": 1, "conversations": {"a": a, "b": b}}),
        );
        let next = state(&reader, &path);
        assert_eq!(
            typing_a(next["messages"].as_array().unwrap())["text"],
            "new tail"
        );
        fs::File::options()
            .write(true)
            .open(&live)
            .unwrap()
            .set_modified(SystemTime::now() - LIVE_FRESH - Duration::from_secs(1))
            .unwrap();
        let stale = state(&reader, &path);
        assert_eq!(stale["live"], false);
        assert!(
            !stale["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["kind"] == "typing")
        );
    }

    #[test]
    fn show_chats_project_prompts_answers_review_absence_and_typing() {
        let dir = tempfile::tempdir().unwrap();
        let mut show = snapshot("show", "running");
        show["summary"]["updated"] = json!("2026-10-15T00:00:03Z");
        show["summary"]["manager_session"] = json!(MANAGER);
        show["summary"]["host"] = json!("codex");
        show["director"] = json!({"mode": "show", "seats": [
            {"id": "a", "label": "甲", "provider": "claude", "model": "m"},
            {"id": "b", "label": "乙", "provider": "codex", "model": "m"},
            {"id": "c", "label": "丙", "provider": "grok", "model": "m"},
            {"id": "d", "label": "丁", "provider": "claude", "model": "m"}],
            "messages": [
                {"kind": "task", "speaker": "@host", "manager_model": "requested", "manager_reported_model": "gpt-6", "round": 1, "text": "题目", "time": "2026-10-15T00:00:00Z", "recipients": [
                    {"seat": "a", "provider": "claude", "model": "m"},
                    {"seat": "d", "provider": "claude", "model": "m"}]},
                {"kind": "answer", "speaker": "a", "round": 1, "text": "# 已提交", "time": "2026-10-15T00:00:01Z"},
                {"kind": "answer", "speaker": "d", "round": 1, "text": "丁的回答", "time": "2026-10-15T00:00:01Z"},
                {"kind": "absent", "speaker": "b", "round": 1, "text": "（本轮未作答：受阻）", "time": "2026-10-15T00:00:02Z"},
                {"kind": "host", "speaker": "@host", "round": 1, "text": "继续", "time": "2026-10-15T00:00:03Z"}],
            "round": {"number": 2, "seats": [
                {"seat_id": "a", "status": "succeeded", "review": "pending", "output": "待审核私密回答"},
                {"seat_id": "b", "status": "running", "output": "**正在写**"},
                {"seat_id": "c", "status": "not_started"}]}});
        save(dir.path(), &show);
        let reader = Reader::new(dir.path().to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let messages = chat["messages"].as_array().unwrap();
        assert_eq!(messages.iter().filter(|m| m["kind"] == "prompt").count(), 2);
        let prompts: Vec<_> = messages.iter().filter(|m| m["kind"] == "prompt").collect();
        assert_eq!(prompts[0]["speaker"]["model"], "gpt-6");
        assert_eq!(prompts[0]["speaker"]["model_source"], "native");
        assert_eq!(prompts[0]["to"], json!(["claude/m", "claude/m"]));
        assert_eq!(prompts[0]["recipients"][0]["seat"], "a");
        assert_ne!(
            prompts[0]["recipients"][0]["focus"],
            prompts[0]["recipients"][1]["focus"]
        );
        for (body, index) in [("# 已提交", 0), ("丁的回答", 1)] {
            assert_eq!(
                messages.iter().find(|m| m["text"] == body).unwrap()["focus"],
                prompts[0]["recipients"][index]["focus"]
            );
        }
        assert_eq!(prompts[1]["speaker"]["model"], "");
        assert!(
            messages
                .iter()
                .any(|m| m["kind"] == "reply" && m["text"] == "# 已提交")
        );
        assert!(
            messages
                .iter()
                .any(|m| m["kind"] == "notice" && m["status"] == "blocked")
        );
        assert!(
            messages
                .iter()
                .any(|m| m["kind"] == "notice" && m["text"] == "待审核，请在主控中决定")
        );
        assert!(!messages.iter().any(|m| m["text"] == "待审核私密回答"));
        let review_notice = messages
            .iter()
            .find(|m| m["text"] == "待审核，请在主控中决定")
            .unwrap();
        assert_eq!(review_notice["status"], "review");
        assert_eq!(review_notice["focus"], prompts[0]["recipients"][0]["focus"]);
        assert_eq!(messages.last().unwrap()["focus"], "seat:show:b");
        assert_eq!(messages.last().unwrap()["kind"], "typing");
        let members = chat["members"].as_array().unwrap();
        assert_eq!(
            members.iter().find(|m| m["key"] == "claude/m").unwrap()["line"],
            "待审核：请在主控中决定"
        );
        assert_eq!(
            members.iter().find(|m| m["key"] == "codex/m").unwrap()["line"],
            "正在写"
        );
        assert_eq!(
            members.iter().find(|m| m["key"] == "grok/m").unwrap()["status"],
            "pending"
        );
    }

    #[test]
    fn show_chats_keep_private_history_after_new_rounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut show = snapshot("private-show", "finished");
        show["summary"]["manager_session"] = json!(MANAGER);
        show["director"] = json!({"mode": "show", "seats": [
            {"id": "a", "provider": "claude", "model": "m"}],
            "messages": [],
            "private_messages": [
                {"round": 1, "seat_id": "a", "reported_model": "m-old", "text": "旧轮私信", "time": "2026-10-15T00:00:00Z"},
                {"round": 2, "seat_id": "a", "reported_model": "m-native", "text": "本轮私信", "time": "2026-10-15T00:00:01Z"}],
            "round": {"number": 2, "seats": [
                {"seat_id": "a", "status": "succeeded", "reported_model": "m-native", "review": "discard", "output": "本轮私信"}]}});
        save(dir.path(), &show);
        let reader = Reader::new(dir.path().to_path_buf());
        let path = format!("/app/chats/{MANAGER}");
        let first = state(&reader, &path);
        let messages = first["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(
            messages
                .iter()
                .all(|m| m["private"] == true && m["kind"] == "reply")
        );
        assert_eq!(messages[0]["text"], "旧轮私信");
        assert_eq!(messages[1]["text"], "本轮私信");
        assert_eq!(messages[0]["speaker"]["model"], "m-old");
        assert_eq!(messages[1]["speaker"]["model"], "m-native");
        assert_eq!(messages[1]["speaker"]["model_source"], "native");
        assert_eq!(messages[1]["focus"], "seat:private-show:a");
        show["director"]["round"] = json!({"number": 3, "seats": []});
        save(dir.path(), &show);
        assert_eq!(state(&reader, &path)["messages"], first["messages"]);
    }

    #[test]
    fn stage_and_chats_sort_filter_and_enforce_limits() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..205 {
            let mut value = task_snapshot(
                &format!("c{i:03}"),
                "succeeded",
                &format!("2099-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
            );
            value["summary"]["manager_session"] = json!(format!("s-{i:032x}"));
            save(dir.path(), &value);
        }
        // Active groups remain first even when old; idle groups outside two hours disappear.
        let running = task_snapshot("running", "running", "2020-01-01T00:00:00Z");
        save(dir.path(), &running);
        let mut review = task_snapshot("review", "review", "2020-01-02T00:00:00Z");
        review["summary"]["manager_session"] = Value::Null;
        save(dir.path(), &review);
        let mut old = task_snapshot("old", "succeeded", "2000-01-01T00:00:00Z");
        old["summary"]["manager_session"] = Value::Null;
        save(dir.path(), &old);
        let reader = Reader::new(dir.path().to_path_buf());
        let stage = state(&reader, "/app/stage");
        let managers = stage["managers"].as_array().unwrap();
        assert_eq!(managers.len(), 6);
        assert_eq!(managers[0]["group"], "c-review");
        assert_eq!(managers[1]["group"], MANAGER);
        assert_eq!(managers[2]["group"], format!("s-{:032x}", 204));
        assert_eq!(stage["counts"], json!({"running": 1, "review": 1}));
        let groups = state(&reader, "/app/chats");
        assert_eq!(groups["groups"].as_array().unwrap().len(), 200);
        assert_eq!(groups["groups"][0]["group"], "c-review");
        // Remove recent groups: no fallback should bring the old idle group into the stage.
        for i in 0..205 {
            fs::remove_file(dir.path().join(format!("conversations/c{i:03}/views.json"))).unwrap();
        }
        assert_eq!(
            state(&reader, "/app/stage")["managers"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            state(&reader, "/app/chats")["groups"]
                .as_array()
                .unwrap()
                .len(),
            3
        );

        old["summary"]["manager_session"] = json!(MANAGER);
        old["summary"]["archived"] = json!(true);
        save(dir.path(), &old);
        let mut many = task_snapshot("many", "running", "2099-01-01T00:00:00Z");
        many["director"]["tasks"] = json!((0..160).map(|i| json!({"id": i.to_string(), "provider": "pi", "model": "m",
            "status": if i == 159 { "running" } else { "succeeded" }, "time": format!("2099-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
            "prompt": "go", "output": "done"})).collect::<Vec<_>>());
        save(dir.path(), &many);
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let messages = chat["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 300);
        assert_eq!(messages.last().unwrap()["kind"], "typing");
        assert!(!messages.iter().any(|m| m["id"] == "many:task:0:prompt"));
        assert_eq!(messages.iter().filter(|m| m["kind"] == "typing").count(), 2);
        let members = chat["members"].as_array().unwrap();
        assert_eq!(members[0]["key"], "pi/m");
        assert_eq!(members[1]["key"], "codex/m");

        // A management deep link must still expose an older conversation in the same group.
        let focused = state(&reader, &format!("/app/chats/{MANAGER}/focus/old"));
        let focused_messages = focused["messages"].as_array().unwrap();
        assert!(focused_messages.iter().any(|m| m["conversation"] == "old"));
        assert!(focused_messages.len() > 300);
        assert_eq!(
            reader
                .respond(&format!("/app/chats/{MANAGER}/focus/missing"))
                .status,
            404
        );
        assert_eq!(
            state(&reader, &format!("/app/chats/{MANAGER}"))["messages"],
            chat["messages"]
        );
    }

    #[test]
    fn failed_task_keeps_native_output_beside_the_failure_notice() {
        let dir = tempfile::tempdir().unwrap();
        let mut task = task_snapshot("failed", "blocked", "2026-10-15T01:00:00Z");
        task["director"]["tasks"][0]["output"] = json!("已完成的部分结果");
        task["director"]["tasks"][0]["error"] = json!("permission_denied");
        task["director"]["tasks"][0]["manager_model"] = json!("gemini-3.5-pro");
        save(dir.path(), &task);
        let reader = Reader::new(dir.path().to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let messages = chat["messages"].as_array().unwrap();
        assert!(
            messages
                .iter()
                .any(|m| m["kind"] == "reply" && m["text"] == "已完成的部分结果")
        );
        assert_eq!(
            messages.iter().find(|m| m["kind"] == "prompt").unwrap()["speaker"]["model"],
            "gemini-3.5-pro"
        );
        assert!(messages.iter().any(|m| m["kind"] == "notice"
            && m["status"] == "blocked"
            && m["detail"] == "permission_denied"));
    }

    #[test]
    fn chat_routes_reject_writes_and_unknown_groups_without_creating_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("missing");
        let reader = Reader::new(root.clone());
        assert_eq!(state(&reader, "/app/stage")["managers"], json!([]));
        assert_eq!(state(&reader, "/app/stage")["empty"], "no_data");
        assert_eq!(state(&reader, "/app/chats")["groups"], json!([]));
        for path in ["/app/stage", "/app/chats", "/app/chats/c-missing"] {
            for method in ["POST", "PUT", "DELETE", "HEAD"] {
                assert_eq!(reader.request(method, path, Some("1"), b"{}").status, 405);
            }
        }
        for path in [
            "/app/chats/c-missing",
            "/app/chats/../x",
            "/app/chats/",
            "/app/chats/a/b",
        ] {
            assert_eq!(reader.request("GET", path, None, b"").status, 404, "{path}");
        }
        assert!(!root.exists());
        fs::create_dir(&root).unwrap();
        assert_eq!(state(&reader, "/app/stage")["empty"], "quiet");
    }

    fn finished_show(id: &str) -> Value {
        let mut show = snapshot(id, "finished");
        show["summary"]["manager_session"] = json!(MANAGER);
        show["summary"]["updated"] = json!("2099-01-01T12:00:00Z");
        show["director"] = json!({"mode": "show", "seats": [
            {"id": "a", "provider": "codex", "model": "m", "label": "甲"},
            {"id": "z", "provider": "codex", "model": "m", "label": "乙"}],
            "messages": [
                {"kind": "answer", "speaker": "z", "round": 1, "text": "较早回答", "time": "2099-01-01T10:00:00Z"},
                {"kind": "answer", "speaker": "a", "round": 1, "text": "最近回答", "time": "2099-01-01T11:00:00Z"},
                {"kind": "host", "speaker": "@host", "round": 1, "text": "主持人后来发言", "time": "2099-01-01T12:00:00Z"}],
            "round": {"number": 1, "status": "finished", "seats": [
                {"seat_id": "a", "status": "succeeded", "output": "最近回答"},
                {"seat_id": "z", "status": "succeeded", "output": "较早回答"}]}});
        show
    }

    #[test]
    fn regression_member_activity_uses_seat_time_across_seats_and_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let mut show = finished_show("show");
        save(dir.path(), &show);
        let reader = Reader::new(dir.path().to_path_buf());
        let path = format!("/app/chats/{MANAGER}");
        let chat = state(&reader, &path);
        assert_eq!(chat["members"][0]["line"], "最近回答");
        assert_eq!(chat["members"][0]["updated"], "2099-01-01T11:00:00Z");
        show["director"]["messages"][0]["kind"] = json!("absent");
        show["director"]["messages"][0]["text"] = json!("（本轮未作答：受阻）");
        show["director"]["round"]["seats"][1]["status"] = json!("blocked");
        save(dir.path(), &show);
        save(
            dir.path(),
            &task_snapshot("task", "succeeded", "2099-01-01T11:30:00Z"),
        );
        assert_eq!(state(&reader, &path)["members"][0]["conversation"], "task");
        show["summary"]["updated"] = json!("2099-01-01T13:00:00Z");
        show["director"]["messages"].as_array_mut().unwrap().push(json!({"kind": "host", "speaker": "@host", "text": "再说一句", "time": "2099-01-01T13:00:00Z"}));
        save(dir.path(), &show);
        assert_eq!(state(&reader, &path)["members"][0]["conversation"], "task");
        assert_eq!(
            state(&reader, "/app/stage")["managers"][0]["members"][0]["conversation"],
            "task"
        );
        // A live running seat has current activity even when its conversation's last entry is old.
        show["summary"]["updated"] = json!("2000-01-01T00:00:00Z");
        show["summary"]["status"] = json!("running");
        show["director"]["round"]["number"] = json!(2);
        show["director"]["round"]["seats"][0]["status"] = json!("running");
        write(
            &dir.path().join("live/1-now.json"),
            &json!({"version": 1, "updated_at": "2099-01-01T14:00:00Z", "conversations": {"show": show}}),
        );
        let chat = state(&reader, &path);
        let typing = chat["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(typing["kind"], "typing");
        assert_eq!(typing["time"], "2099-01-01T14:00:00Z");
    }

    #[test]
    fn regression_finished_show_status_includes_failed_seats_but_not_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let mut show = finished_show("show");
        let reader = Reader::new(dir.path().to_path_buf());
        for (status, expected) in [
            ("failed", "failed"),
            ("blocked", "failed"),
            ("interrupted", "failed"),
            ("cancelled", "idle"),
        ] {
            show["director"]["round"]["seats"][0]["status"] = json!(status);
            save(dir.path(), &show);
            assert_eq!(
                state(&reader, &format!("/app/chats/{MANAGER}"))["status"],
                expected,
                "{status}"
            );
            assert_eq!(
                state(&reader, "/app/chats")["groups"][0]["status"],
                expected,
                "{status}"
            );
            assert_eq!(
                state(&reader, "/app/stage")["managers"][0]["status"],
                expected,
                "{status}"
            );
        }
        let mut task = task_snapshot("task", "cancelled", "2099-01-01T12:00:01Z");
        save(dir.path(), &task);
        assert_eq!(state(&reader, "/app/chats")["groups"][0]["status"], "idle");
        task["director"]["tasks"][0]["status"] = json!("review");
        save(dir.path(), &task);
        assert_eq!(
            state(&reader, "/app/chats")["groups"][0]["status"],
            "review"
        );
        show["director"]["round"]["seats"][0]["status"] = json!("running");
        save(dir.path(), &show);
        assert_eq!(
            state(&reader, "/app/chats")["groups"][0]["status"],
            "running"
        );
    }

    #[test]
    fn regression_markdown_summary_preserves_paths_flags_and_literal_characters() {
        let dir = tempfile::tempdir().unwrap();
        let reader = Reader::new(dir.path().to_path_buf());
        let mut task = task_snapshot("task", "succeeded", "2099-01-01T00:00:00Z");
        task["director"]["tasks"][0]["output"] = json!(
            "# > - **结果** __OK__ **`a_b.rs **raw**`** --my_flag foo*bar ~tmp [arg] *literal* **未闭合"
        );
        save(dir.path(), &task);
        assert_eq!(
            state(&reader, &format!("/app/chats/{MANAGER}"))["members"][0]["line"],
            "结果 OK a_b.rs **raw** --my_flag foo*bar ~tmp [arg] *literal* **未闭合"
        );
        let literal = "--dry_run ~/a_b [x](y) a*b _x_ ~x~ name__part__x";
        task["director"]["tasks"][0]["output"] = json!(literal);
        save(dir.path(), &task);
        assert_eq!(
            state(&reader, &format!("/app/chats/{MANAGER}"))["members"][0]["line"],
            literal
        );
    }

    #[test]
    fn regression_missing_or_invalid_times_fall_back_without_reordering_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mut show = finished_show("show");
        for (index, entry) in show["director"]["messages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            entry["time"] =
                [Value::Null, json!("invalid"), json!("2099-01-01T12:00:00Z")][index].clone();
        }
        save(dir.path(), &show);
        let mut task = task_snapshot("task", "succeeded", "2099-01-01T12:00:00Z");
        task["director"]["tasks"][0]["time"] = Value::Null;
        let mut second = task["director"]["tasks"][0].clone();
        second["id"] = json!("second");
        second["time"] = json!("2099/01/01T12:00:00Z");
        task["director"]["tasks"]
            .as_array_mut()
            .unwrap()
            .push(second);
        save(dir.path(), &task);
        let reader = Reader::new(dir.path().to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let messages = chat["messages"].as_array().unwrap();
        assert!(messages.iter().all(|m| m["time"] == "2099-01-01T12:00:00Z"));
        assert_eq!(
            messages
                .iter()
                .map(|m| m["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "show:entry:0",
                "show:entry:1",
                "show:entry:2",
                "task:task:t-task:prompt",
                "task:task:t-task:result",
                "task:task:second:prompt",
                "task:task:second:result"
            ]
        );
    }

    #[test]
    fn revision_one_counts_members_and_assigns_consistent_collision_free_colors() {
        let dir = tempfile::tempdir().unwrap();
        let mut work = task_snapshot("work", "running", "2099-01-01T00:00:00Z");
        work["summary"]["manager_session"] = json!(format!("s-{:032x}", 0));
        let mut pi = work["director"]["tasks"][0].clone();
        pi["id"] = json!("pi");
        pi["provider"] = json!("pi");
        let mut review = pi.clone();
        review["id"] = json!("review");
        review["provider"] = json!("claude");
        review["status"] = json!("review");
        work["director"]["tasks"]
            .as_array_mut()
            .unwrap()
            .extend([pi, review]);
        save(dir.path(), &work);
        let mut duplicate = task_snapshot("duplicate", "running", "2098-01-01T00:00:00Z");
        duplicate["summary"]["manager_session"] = work["summary"]["manager_session"].clone();
        save(dir.path(), &duplicate);
        // The IDs ending 0 and 8 collide in FNV-1a modulo 8.
        let mut other = task_snapshot("other", "succeeded", "2099-01-01T00:00:01Z");
        other["summary"]["manager_session"] = json!(format!("s-{:032x}", 8));
        save(dir.path(), &other);
        let reader = Reader::new(dir.path().to_path_buf());
        let stage = state(&reader, "/app/stage");
        assert_eq!(stage["counts"], json!({"running": 2, "review": 1}));
        assert_eq!(stage["empty"], Value::Null);
        let managers = stage["managers"].as_array().unwrap();
        assert_eq!(managers.len(), 2);
        assert_ne!(managers[0]["color"], managers[1]["color"]);
        let groups = state(&reader, "/app/chats");
        for manager in managers {
            let id = manager["group"].as_str().unwrap();
            assert_eq!(
                groups["groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|g| g["group"] == id)
                    .unwrap()["color"],
                manager["color"]
            );
            assert_eq!(
                state(&reader, &format!("/app/chats/{id}"))["color"],
                manager["color"]
            );
        }
        // The limit applies before counts, even if more active groups exist.
        for i in 1..=7 {
            let mut extra = task_snapshot(&format!("extra{i}"), "running", "2099-01-01T00:00:02Z");
            extra["summary"]["manager_session"] = json!(format!("s-{:032x}", i));
            save(dir.path(), &extra);
        }
        let stage = state(&reader, "/app/stage");
        assert_eq!(stage["managers"].as_array().unwrap().len(), 6);
        assert_eq!(stage["counts"], json!({"running": 6, "review": 0}));
        let colors: std::collections::HashSet<_> = stage["managers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["color"].as_u64().unwrap())
            .collect();
        assert_eq!(colors.len(), 6);
    }

    #[test]
    fn revision_one_old_failure_clears_after_a_newer_terminal_member() {
        let dir = tempfile::tempdir().unwrap();
        let reader = Reader::new(dir.path().to_path_buf());
        let failed = task_snapshot("failed", "failed", "2099-01-01T00:00:00Z");
        save(dir.path(), &failed);
        let mut latest = task_snapshot("latest", "succeeded", "2099-01-01T00:00:01Z");
        latest["director"]["tasks"][0]["provider"] = json!("pi");
        save(dir.path(), &latest);
        assert_eq!(state(&reader, "/app/chats")["groups"][0]["status"], "idle");
        assert_eq!(
            state(&reader, "/app/stage")["managers"][0]["status"],
            "idle"
        );
        latest["summary"]["status"] = json!("blocked");
        latest["director"]["tasks"][0]["status"] = json!("blocked");
        latest["director"]["tasks"][0]["error"] = json!("needs permission");
        save(dir.path(), &latest);
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        assert_eq!(chat["status"], "failed");
        let notice = chat["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(notice["text"], "受阻");
        assert_eq!(notice["detail"], "needs permission");
        assert_eq!(notice["speaker"]["provider"], "pi");
        latest["summary"]["status"] = json!("cancelled");
        latest["director"]["tasks"][0]["status"] = json!("cancelled");
        save(dir.path(), &latest);
        assert_eq!(state(&reader, "/app/chats")["groups"][0]["status"], "idle");
    }

    #[test]
    fn revision_one_typing_uses_live_publication_time_and_stops_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let task = task_snapshot("task", "running", "2020-01-01T00:00:00Z");
        save(dir.path(), &task);
        let live = dir.path().join("live/1-time.json");
        write(
            &live,
            &json!({"version": 1, "updated_at": "2099-01-01T00:00:00.123456Z", "conversations": {"task": task}}),
        );
        let reader = Reader::new(dir.path().to_path_buf());
        let path = format!("/app/chats/{MANAGER}");
        let chat = state(&reader, &path);
        assert_eq!(
            chat["messages"].as_array().unwrap().last().unwrap()["time"],
            "2099-01-01T00:00:00.123456Z"
        );
        fs::File::options()
            .write(true)
            .open(&live)
            .unwrap()
            .set_modified(SystemTime::now() - LIVE_FRESH - Duration::from_secs(1))
            .unwrap();
        let chat = state(&reader, &path);
        assert_eq!(chat["live"], false);
        assert_eq!(
            chat["messages"].as_array().unwrap().last().unwrap()["time"],
            "2020-01-01T00:00:00Z"
        );
    }

    fn office_show(id: &str, updated: &str, created: &str) -> Value {
        json!({
            "version": 1,
            "summary": {"id": id, "title": format!("房间 {id}"), "mode": "show", "status": "running",
                "updated": updated, "host": "claude", "manager_session": MANAGER},
            "director": {"mode": "show", "created": created, "seats": [
                {"id": "a", "provider": "claude", "model": "m", "role": "reviewer"},
                {"id": "b", "provider": "codex", "model": "m", "role": Value::Null}],
            "messages": [
                {"kind": "task", "speaker": "@host", "round": 1, "text": "task", "time": created,
                    "recipients": [{"seat": "a", "provider": "claude", "model": "m"}]},
                {"kind": "answer", "speaker": "a", "round": 1, "text": "answer", "time": updated}],
            "round": {"number": 1, "seats": [{"seat_id": "a", "status": "running"}]}}})
    }

    #[test]
    fn desks_stay_persistent_ordered_and_split_by_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut reviewer = task_snapshot("r1", "succeeded", "2026-10-15T01:00:00Z");
        reviewer["director"]["tasks"][0]["role"] = json!("reviewer");
        reviewer["director"]["tasks"][0]["thread"] = json!("native-1");
        reviewer["director"]["tasks"][0]["access"] = json!("read");
        save(root, &reviewer);
        let mut author = task_snapshot("a1", "running", "2026-10-15T02:00:00Z");
        author["director"]["tasks"][0]["role"] = json!("author");
        save(root, &author);
        let mut other_model = task_snapshot("r2", "succeeded", "2026-10-15T03:00:00Z");
        other_model["director"]["tasks"][0]["role"] = json!("reviewer");
        other_model["director"]["tasks"][0]["model"] = json!("m2");
        other_model["director"]["tasks"][0]["thread"] = json!("native-2");
        save(root, &other_model);
        let mut newer = task_snapshot("r3", "succeeded", "2026-10-15T04:00:00Z");
        newer["director"]["tasks"][0]["role"] = json!("reviewer");
        newer["director"]["tasks"][0]["thread"] = json!("native-1");
        save(root, &newer);
        let mut claude = task_snapshot("c1", "succeeded", "2026-10-15T05:00:00Z");
        claude["director"]["tasks"][0]["provider"] = json!("claude");
        save(root, &claude);
        let reader = Reader::new(root.to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let desks = chat["desks"].as_array().unwrap();
        assert_eq!(desks.len(), 4);
        let keys: Vec<_> = desks
            .iter()
            .map(|desk| desk["key"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            keys,
            vec![
                "codex|m|reviewer",
                "codex|m|author",
                "codex|m2|reviewer",
                "claude|m|"
            ]
        );
        assert_eq!(desks[0]["label"], "m · Codex · reviewer");
        assert_eq!(desks[0]["role"], "reviewer");
        assert_eq!(desks[0]["access"], "read");
        assert_eq!(desks[1]["access"], Value::Null);
        assert_eq!(desks[0]["model"], "m");
        assert_eq!(desks[0]["first_seen"], "2026-10-15T01:00:00.000000Z");
        assert_eq!(desks[0]["last_active"], "2026-10-15T04:00:00.000000Z");
        // The same native session stays one persistent desk thread.
        assert_eq!(desks[0]["private"]["threads"], 1);
        assert_eq!(desks[0]["private"]["running"], 0);
        assert_eq!(desks[0]["status"], "idle");
        assert_eq!(desks[0]["private_allowed"], true);
        assert_eq!(desks[1]["status"], "running");
        assert_eq!(desks[1]["private"]["threads"], 1); // no session id counts separately
        assert_eq!(desks[3]["private_allowed"], false); // same as the workspace host's CLI
        assert_eq!(desks[3]["role"], Value::Null);
        assert_eq!(desks[3]["label"], "m · Claude Code");
    }

    #[test]
    fn desks_report_meeting_and_rooms_in_creation_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let older = office_show("older", "2026-10-15T09:00:00Z", "2026-10-15T00:00:00Z");
        save(root, &older);
        let mut newer = office_show("newer", "2026-10-15T10:00:00Z", "2026-10-15T01:00:00Z");
        newer["summary"]["status"] = json!("review");
        newer["director"]["round"]["seats"][0]["review"] = json!("pending");
        newer["director"]["round"]["seats"][0]["status"] = json!("succeeded");
        save(root, &newer);
        let reader = Reader::new(root.to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let rooms = chat["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 2);
        assert_eq!(rooms[0]["id"], "older");
        assert_eq!(rooms[0]["title"], "房间 older");
        assert_eq!(rooms[0]["status"], "running");
        assert_eq!(rooms[0]["pending_review"], 0);
        assert_eq!(rooms[0]["board"], 2);
        assert_eq!(
            rooms[0]["seats"],
            json!([
                {"seat": "a", "desk": "claude|m|reviewer"},
                {"seat": "b", "desk": "codex|m|"}
            ])
        );
        assert_eq!(rooms[1]["id"], "newer");
        assert_eq!(rooms[1]["status"], "review");
        assert_eq!(rooms[1]["pending_review"], 1);
        let desks = chat["desks"].as_array().unwrap();
        let reviewer = desks
            .iter()
            .find(|desk| desk["key"] == "claude|m|reviewer")
            .unwrap();
        assert_eq!(
            reviewer["meeting"],
            json!({"room": "newer", "seat": "a", "status": "review"})
        );
        let pending = desks.iter().find(|desk| desk["key"] == "codex|m|").unwrap();
        assert_eq!(pending["status"], "pending");
        assert_eq!(
            pending["meeting"],
            json!({"room": Value::Null, "seat": Value::Null, "status": Value::Null})
        );
    }

    #[test]
    fn messages_carry_message_id_visibility_thread_desk_and_forward() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut task = task_snapshot("task", "succeeded", "2026-10-15T00:00:00Z");
        task["director"]["tasks"][0]["role"] = json!("reviewer");
        task["director"]["tasks"][0]["thread"] = json!("native-1");
        task["director"]["tasks"][0]["message_id"] = json!("task:t:t-task");
        save(root, &task);
        let mut show = office_show("show", "2026-10-15T02:00:00Z", "2026-10-15T00:00:00Z");
        show["summary"]["status"] = json!("finished");
        show["director"]["messages"] = json!([
            {"kind": "task", "speaker": "@host", "round": 1, "text": "题目", "time": "2026-10-15T01:00:00Z",
                "recipients": [{"seat": "a", "provider": "claude", "model": "m"}], "message_id": "show:e:0"},
            {"kind": "forward", "speaker": "@forward", "round": 1, "text": "转发内容", "time": "2026-10-15T01:01:00Z",
                "message_id": "show:e:1", "forward_id": "task:t:t-task", "forward_source": "房间 · 工位 · 时间"},
            {"kind": "absent", "speaker": "b", "round": 1, "text": "（本轮未作答：失败）", "time": "2026-10-15T01:02:00Z",
                "message_id": "show:e:2"}]);
        show["director"]["private_messages"] = json!([
            {"round": 1, "seat_id": "a", "text": "丢弃回答", "time": "2026-10-15T01:03:00Z"}
        ]);
        show["director"]["round"] = json!({"number": 1, "seats": [
            {"seat_id": "a", "status": "running", "output": "正在写"}]});
        save(root, &show);
        let reader = Reader::new(root.to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let messages = chat["messages"].as_array().unwrap();
        let by_id = |id: &str| messages.iter().find(|m| m["id"] == id).unwrap();
        let prompt = by_id("task:task:t-task:prompt");
        assert_eq!(prompt["visibility"], "private");
        assert_eq!(prompt["message_id"], "task:t:t-task");
        assert_eq!(
            prompt["thread"],
            json!({"kind": "desk", "key": "codex|m|reviewer"})
        );
        assert_eq!(prompt["desk"], "codex|m|reviewer");
        assert_eq!(prompt["forward"], Value::Null);
        let entry = by_id("show:entry:0");
        assert_eq!(entry["visibility"], "public");
        assert_eq!(entry["message_id"], "show:e:0");
        assert_eq!(entry["thread"], json!({"kind": "room", "id": "show"}));
        assert_eq!(entry["desk"], "claude|m|reviewer");
        let forward = by_id("show:entry:1");
        assert_eq!(forward["kind"], "forward");
        assert_eq!(forward["visibility"], "public");
        assert_eq!(forward["message_id"], "show:e:1");
        assert_eq!(forward["desk"], Value::Null);
        assert_eq!(
            forward["forward"],
            json!({"message_id": "task:t:t-task", "source": "房间 · 工位 · 时间"})
        );
        assert_eq!(forward["speaker"]["label"], "主控转发");
        let absent = by_id("show:entry:2");
        assert_eq!(absent["visibility"], "public");
        assert_eq!(absent["message_id"], "show:e:2");
        assert_eq!(absent["desk"], "codex|m|");
        let private = by_id("show:round:1:a:private");
        assert_eq!(private["visibility"], "private");
        assert_eq!(private["message_id"], Value::Null);
        let typing = messages.iter().find(|m| m["kind"] == "typing").unwrap();
        assert_eq!(typing["desk"], "claude|m|reviewer");
        assert_eq!(typing["thread"], json!({"kind": "room", "id": "show"}));
        assert_eq!(typing["message_id"], Value::Null);
    }

    #[test]
    fn old_snapshots_still_project_desks_rooms_and_messages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        save(
            root,
            &task_snapshot("old", "succeeded", "2026-10-15T00:00:00Z"),
        );
        let mut show = snapshot("old-show", "finished");
        show["summary"]["manager_session"] = json!(MANAGER);
        show["summary"]["updated"] = json!("2026-10-15T01:00:00Z");
        show["director"] = json!({"mode": "show",
            "seats": [{"id": "a", "provider": "claude", "model": "m"}],
            "messages": [{"kind": "strange", "speaker": "a", "round": 1, "text": "未知种类",
                "time": "2026-10-15T01:00:00Z"}],
            "round": {"number": 1, "seats": []}});
        save(root, &show);
        let reader = Reader::new(root.to_path_buf());
        let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
        let desks = chat["desks"].as_array().unwrap();
        assert!(desks.iter().any(|desk| desk["key"] == "codex|m|"));
        assert!(
            desks
                .iter()
                .any(|desk| desk["key"] == "claude|m|" && desk["role"].is_null())
        );
        let rooms = chat["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0]["id"], "old-show");
        let unknown = chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["text"] == "未知种类")
            .unwrap();
        assert_eq!(unknown["message_id"], Value::Null);
        assert_eq!(unknown["thread"], json!({"kind": "room", "id": "old-show"}));
        assert_eq!(unknown["visibility"], "public");
        assert_eq!(unknown["desk"], "claude|m|");
        let prompt = chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["kind"] == "prompt")
            .unwrap();
        assert_eq!(prompt["message_id"], Value::Null);
        assert_eq!(prompt["thread"], json!({"kind": "desk", "key": "codex|m|"}));
    }
}

//! Studio snapshots (ADR-0008) and the tray-only management routes (ADR-0012).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
            "/../../src/agentnave_studio/dashboard_assets/",
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

pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'";

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
    pub archived: bool,
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
            .filter(|item| !item.archived && item.status == status)
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

    fn error(status: u16, message: &str) -> Self {
        let mut response = Self::json(&json!({"error": message}));
        response.status = status;
        response
    }
}

enum Scope<'a> {
    Home,
    Director(&'a str),
    Public(&'a str),
}

type Cached = (SystemTime, u64, Option<Item>);

pub struct Reader {
    root: PathBuf,
    /// Summaries by views.json path, keyed on (mtime, size), so polling parses only changed files.
    summaries: Mutex<HashMap<PathBuf, Cached>>,
}

impl Reader {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            summaries: Mutex::new(HashMap::new()),
        }
    }

    /// Writes require an explicit POST and action header; navigation cannot mutate files.
    pub fn request(&self, method: &str, path: &str, action: Option<&str>, body: &[u8]) -> Response {
        if path == "/app/profiles" {
            if method != "POST" {
                return Response::error(405, "此操作只接受 POST");
            }
            if action != Some("1") {
                return Response::error(403, "缺少操作请求头");
            }
            let Ok(payload) = serde_json::from_slice::<Value>(body) else {
                return Response::error(400, "请求必须为 JSON");
            };
            if let Err(error) = crate::profiles::validate(&payload) {
                return Response::error(400, &error);
            }
            return match crate::profiles::save(&self.root, &payload) {
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

    pub fn overview(&self) -> Overview {
        let Ok(entries) = fs::read_dir(self.root.join("conversations")) else {
            return Overview {
                missing: true,
                ..Overview::default()
            };
        };
        let live = self.live();
        let mut cache = self
            .summaries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut seen = Vec::new();
        let mut items = Vec::new();
        let mut unavailable = Vec::new();
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            let directory = entry.path();
            if !valid_id(&id) || !directory.is_dir() {
                continue;
            }
            if let Some(item) = live.get(&id).and_then(summary) {
                items.push((SystemTime::now(), item));
                continue;
            }
            let path = directory.join("views.json");
            let Ok(meta) = fs::metadata(&path) else {
                if directory.join("state.json").exists() {
                    unavailable.push((
                        directory.display().to_string(),
                        "尚无视图快照：任一工作室服务启动后会补写".to_owned(),
                    ));
                }
                continue;
            };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let fresh = matches!(cache.get(&path), Some((m, len, _)) if *m == modified && *len == meta.len());
            if !fresh {
                let item = read_json(&path).as_ref().and_then(summary);
                cache.insert(path.clone(), (modified, meta.len(), item));
            }
            seen.push(path.clone());
            match &cache[&path].2 {
                Some(item) => items.push((modified, item.clone())),
                None => unavailable.push((
                    path.display().to_string(),
                    "视图快照无法读取或版本不受支持".to_owned(),
                )),
            }
        }
        cache.retain(|path, _| seen.contains(path));
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        unavailable.sort();
        Overview {
            items: items.into_iter().map(|(_, item)| item).collect(),
            unavailable,
            missing: false,
        }
    }

    /// Serve one path of the `agentnave` scheme: the dashboard assets and its `state` JSON.
    pub fn respond(&self, path: &str) -> Response {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if let ["settings", resource] = parts.as_slice() {
            return match *resource {
                "" => Response::ok("text/html; charset=utf-8", SETTINGS_HTML.to_vec()),
                "settings.js" => {
                    Response::ok("text/javascript; charset=utf-8", SETTINGS_JS.to_vec())
                }
                "settings.css" => Response::ok("text/css; charset=utf-8", SETTINGS_CSS.to_vec()),
                "state" => match crate::profiles::read(&self.root) {
                    Ok(document) => Response::json(&document),
                    Err(error) => Response::error(500, &error),
                },
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
                _ => Response::not_found(),
            };
        }
        let (scope, resource) = match parts.as_slice() {
            [resource] => (Scope::Home, *resource),
            ["c", id, resource] if valid_id(id) => (Scope::Director(id), *resource),
            ["p", id, resource] if valid_id(id) => (Scope::Public(id), *resource),
            _ => return Response::not_found(),
        };
        let asset = match resource {
            "" => Some(("text/html; charset=utf-8", &asset!("index.html")[..])),
            "app.js" => Some(("text/javascript; charset=utf-8", &asset!("app.js")[..])),
            "style.css" => Some(("text/css; charset=utf-8", &asset!("style.css")[..])),
            "avatars.png" => Some(("image/png", &asset!("avatars.png")[..])),
            _ => None,
        };
        if let Some((content_type, body)) = asset {
            return Response::ok(content_type, body.to_vec());
        }
        let view = match scope {
            Scope::Home => Some(self.home()),
            Scope::Director(id) => self.snapshot(id).and_then(|s| s.get("director").cloned()),
            Scope::Public(id) => self.snapshot(id).and_then(|s| s.get("public").cloned()),
        };
        let Some(view) = view.filter(Value::is_object) else {
            return Response::not_found();
        };
        match resource {
            "state" => Response::json(&view),
            "transcript.jsonl" => {
                // As in the browser view: only show transcripts, never the workbench or tasks.
                if view["view"] == "workbench" || view["mode"] == "task" {
                    return Response::not_found();
                }
                let mut body = String::new();
                for message in view["messages"].as_array().into_iter().flatten() {
                    body.push_str(&message.to_string());
                    body.push('\n');
                }
                Response::ok("application/x-ndjson; charset=utf-8", body.into_bytes())
            }
            _ => Response::not_found(),
        }
    }

    fn home(&self) -> Value {
        let overview = self.overview();
        json!({
            "view": "workbench",
            "id": "workbench",
            "title": "工作台",
            "seats": [],
            "messages": [],
            "shows": overview.items.iter().map(|item| json!({
                "title": item.title,
                "mode": item.mode,
                "archived": item.archived,
                "director_url": format!("/c/{}/", item.id),
                "status": item.status,
                "updated": item.updated,
            })).collect::<Vec<_>>(),
            "refresh_error": if overview.missing { Some("未找到工作室数据目录") } else { None },
            "unavailable": overview.unavailable.iter().map(|(path, reason)| json!({"path": path, "reason": reason})).collect::<Vec<_>>(),
            "storage": self.root.display().to_string(),
        })
    }

    fn snapshot(&self, id: &str) -> Option<Value> {
        self.live()
            .remove(id)
            .or_else(|| read_json(&self.root.join("conversations").join(id).join("views.json")))
    }

    /// Snapshots of running conversations from servers that wrote within LIVE_FRESH.
    fn live(&self) -> HashMap<String, Value> {
        let mut found: Vec<(SystemTime, HashMap<String, Value>)> = Vec::new();
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
            if let Some(Value::Object(conversations)) = read_json(&path)
                .and_then(|mut payload| payload.get_mut("conversations").map(Value::take))
            {
                found.push((modified, conversations.into_iter().collect()));
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
        archived: summary
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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
            "summary": {"id": id, "title": format!("T {id}"), "mode": "show", "archived": false, "status": status, "updated": "2026-10-15T00:00:00+00:00"},
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
        assert_eq!(state(&reader, "/c/a1/state")["status"], "finished");

        let live = root.join("live/1-x.json");
        write(
            &live,
            &json!({"version": 1, "conversations": {"a1": snapshot("a1", "running")}}),
        );
        assert_eq!(reader.overview().count("running"), 1);
        assert_eq!(state(&reader, "/c/a1/state")["status"], "running");

        // A crashed server's file stops counting once it is older than LIVE_FRESH.
        let old = SystemTime::now() - LIVE_FRESH - Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&live)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(reader.overview().count("running"), 0);
        assert_eq!(state(&reader, "/c/a1/state")["status"], "finished");
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

        let home = state(&reader, "/state");
        assert_eq!(home["shows"].as_array().unwrap().len(), 2);
        assert!(
            home["shows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["director_url"] == "/c/a1/"
                    && s["updated"] == "2026-10-15T00:00:00+00:00")
        );
        assert_eq!(state(&reader, "/p/a1/state")["view"], "public");
        assert_eq!(reader.respond("/c/a1/").status, 200);
        assert_eq!(reader.respond("/panel/").status, 200);
        for path in [
            "/manage/",
            "/manage/manage.js",
            "/manage/manage.css",
            "/settings/",
            "/settings/settings.js",
            "/settings/settings.css",
        ] {
            assert_eq!(reader.respond(path).status, 200, "{path}");
        }
        assert_eq!(
            reader.respond("/p/a1/transcript.jsonl").body,
            b"{\"text\":\"hi\"}\n"
        );
        for path in [
            "/p/t1/state",
            "/c/t1/transcript.jsonl",
            "/transcript.jsonl",
            "/c/../state",
            "/c/a1",
            "/c/zz/state",
            "/x/a1/state",
            "/panel/state",
            "/manage/unknown",
        ] {
            assert_eq!(reader.respond(path).status, 404, "{path}");
        }
    }
}

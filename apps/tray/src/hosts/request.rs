use super::Hosts;
use crate::response::{Response, with_payload};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;

pub(crate) fn orchestration_request(
    root: &Path,
    host_state: &Mutex<Option<Hosts>>,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if path == "/app/orchestration" {
        if method == "GET" {
            return orchestration(root, host_state);
        }
        if method != "POST" {
            return Response::error(405, "此路由只接受 GET 或 POST");
        }
        return with_payload(action, body, |payload| {
            save_orchestration(root, host_state, &payload)
        });
    }
    if path == "/app/rules" || path == "/app/rules/open" {
        let home = dirs::home_dir().unwrap_or_default();
        let apps = host_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(crate::hosts::Hosts::rule_apps)
            .unwrap_or_default();
        return crate::rules::request(method, path, action, body, &apps, &home, || {
            rule_files(root, host_state)
        });
    }
    unreachable!("settings route checked by caller")
}

fn orchestration(root: &Path, host_state: &Mutex<Option<Hosts>>) -> Response {
    let (document, error) = match crate::members::read_orchestration(root) {
        Ok(mut document) => {
            crate::rules::migrate_custom(&mut document);
            (document, Value::Null)
        }
        Err(error) => (json!({"version": 1, "preset": "off"}), json!(error)),
    };
    let rules = rule_files(root, host_state);
    Response::json(&json!({"document": document, "error": error,
        "presets": crate::rules::presets(&document), "roles": crate::orchestration_roles::catalog(),
        "files": rules["files"], "bosses": rules["bosses"]}))
}
fn save_orchestration(root: &Path, host_state: &Mutex<Option<Hosts>>, payload: &Value) -> Response {
    // The modes each row runs on are worked out here, never taken from the page.
    let mut document = payload.clone();
    if let Some(fields) = document.as_object_mut() {
        fields.remove("effective_apps");
    }
    if let Err(error) = crate::members::validate_orchestration(&document) {
        return Response::error(400, &error);
    }
    let guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
    document["effective_apps"] = guard
        .as_ref()
        .map(|hosts| hosts.effective_apps(&document))
        .unwrap_or_else(|| json!({}));
    if let Err(error) = crate::members::save_orchestration(root, &document) {
        return Response::error(500, &error);
    }
    let failures = guard
        .as_ref()
        .map(|hosts| sync_orchestration(root, hosts, &[]))
        .unwrap_or_default();
    drop(guard);
    let rules = rule_files(root, host_state);
    Response::json(&json!({"saved": true, "failures": failures,
        "files": rules["files"], "bosses": rules["bosses"]}))
}
/// Bring the orchestration block and the modes the server reads per row in line with the
/// settings and the connected apps. A broken orchestration.json leaves the files alone; a
/// preview build never writes them.
pub(crate) fn sync_orchestration(
    root: &Path,
    hosts: &crate::hosts::Hosts,
    also: &[std::path::PathBuf],
) -> Vec<Value> {
    if crate::PREVIEW {
        return Vec::new();
    }
    let Ok(mut document) = crate::members::read_orchestration(root) else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    let effective = hosts.effective_apps(&document);
    if root.join("orchestration.json").is_file()
        && document.get("effective_apps") != Some(&effective)
    {
        document["effective_apps"] = effective;
        if let Err(error) = crate::members::save_orchestration(root, &document) {
            failures.push(json!({"path": root.join("orchestration.json"), "error": error}));
        }
    }
    failures.extend(hosts.sync_rules(&document, also));
    failures
}
/// The global rules files with whether each carries the current orchestration block.
fn rule_files(root: &Path, host_state: &Mutex<Option<Hosts>>) -> Value {
    let home = dirs::home_dir().unwrap_or_default();
    let guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
    let (installed, connected) = guard
        .as_ref()
        .map(|hosts| (hosts.rule_apps(), hosts.connected_rule_apps()))
        .unwrap_or_default();
    let document =
        crate::members::read_orchestration(root).unwrap_or_else(|_| json!({"preset": "off"}));
    crate::rules::list(&installed, &connected, &home, &document)
}
pub(crate) fn skill_request(method: &str) -> Response {
    if method != "GET" {
        return Response::error(405, "此路由只接受 GET");
    }
    Response::json(&super::skill_catalog())
}

pub(crate) fn request(
    root: &Path,
    host_state: &Mutex<Option<Hosts>>,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let read = path == "/app/hosts";
    let write = (cfg!(unix) && path == "/app/hosts/reload-copy")
        || matches!(
            path,
            "/app/hosts/refresh"
                | "/app/hosts/connect"
                | "/app/hosts/remove"
                | "/app/hosts/root"
                | "/app/hosts/confirm-reload"
                | "/app/hosts/skill"
                | "/app/hosts/reveal-backups"
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
    let mut guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
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
            sync_orchestration(root, hosts, &[]);
            Response::json(&snapshot)
        }
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
            // Judged before the connection counts itself as a boss that ran here.
            let used = operation == "connect"
                && crate::members::used_before(root, hosts.cached_snapshot());
            let result = hosts.action(operation, &payload);
            // Connecting, removing or moving a host changes which rules files carry the block.
            if matches!(operation, "connect" | "remove" | "root") {
                sync_orchestration(root, hosts, &before);
            }
            host_action_response(root, operation, result, used)
        }
    }
}

fn host_action_response(
    root: &Path,
    operation: &str,
    result: Result<Value, String>,
    used: bool,
) -> Response {
    let mut result = match result {
        Ok(result) => result,
        Err(error) => return Response::error(400, &error),
    };
    // The connection already took effect, so a failed member update is told, not an error.
    if operation == "connect"
        && result["ok"] == true
        && let Err(error) = crate::members::confirm_host(
            root,
            result["host"]["id"].as_str().unwrap_or_default(),
            used,
        )
    {
        let message = result["message"].as_str().unwrap_or_default();
        result["message"] = json!(format!("{message}；员工名单未更新：{error}"));
    }
    Response::json(&result)
}

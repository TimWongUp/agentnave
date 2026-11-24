//! The routes of T3 Code orchestration (ADR-0121): reading T3 models plus `t3-seats.json`,
//! and saving `t3-seats.json` followed immediately by syncing global rules files.
use super::Hosts;
use super::request::{rule_files, sync_orchestration};
use crate::response::{Response, with_payload};
use crate::{t3_models, t3_seats};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;

pub(crate) fn t3_request(
    root: &Path,
    host_state: &Mutex<Option<Hosts>>,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if path != "/app/t3-seats" {
        return Response::not_found();
    }
    if method == "GET" {
        return state(root, host_state);
    }
    if method != "POST" {
        return Response::error(405, "此路由只接受 GET 或 POST");
    }
    with_payload(action, body, |payload| {
        save_seats(root, host_state, payload)
    })
}

fn t3_catalog(host_state: &Mutex<Option<Hosts>>) -> Value {
    let guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
    let (env, home) = match guard.as_ref() {
        Some(hosts) => (
            Some(hosts.context.env.clone()),
            Some(hosts.context.home.clone()),
        ),
        None => (None, None),
    };
    drop(guard);
    t3_models::read_for(env.as_ref(), home.as_deref())
}

fn state(root: &Path, host_state: &Mutex<Option<Hosts>>) -> Response {
    let (document, error) = match t3_seats::read(root) {
        Ok(document) => (document.unwrap_or(Value::Null), Value::Null),
        Err(error) => (Value::Null, json!(error)),
    };
    let t3 = t3_catalog(host_state);
    Response::json(&json!({
        "document": document,
        "error": error,
        "t3": t3,
    }))
}

fn save_seats(root: &Path, host_state: &Mutex<Option<Hosts>>, mut document: Value) -> Response {
    t3_seats::normalize(&mut document);
    if let Err(error) = t3_seats::validate_for_save(&document) {
        return Response::error(400, &error);
    }
    let guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(error) = t3_seats::save(root, &document) {
        return Response::error(500, &error);
    }
    let failures = guard
        .as_ref()
        .map(|hosts| sync_orchestration(root, hosts, &[]))
        .unwrap_or_default();
    drop(guard);
    let rules = rule_files(root, host_state);
    Response::json(&json!({
        "saved": true,
        "failures": failures,
        "files": rules["files"],
        "bosses": rules["bosses"],
    }))
}

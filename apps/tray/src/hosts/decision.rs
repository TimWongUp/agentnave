//! The routes of the decision-model seat experiment (ADR-0087): the catalog and the decision
//! model's key. A preview build refuses their writes in the entry like every other.
use crate::decision_seats;
use crate::response::{Response, with_payload};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn decision_request(
    root: &Path,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if path == "/app/decision-seats" && method == "GET" {
        return state(root);
    }
    if method != "POST" {
        return Response::error(405, "此路由只接受 GET 或 POST");
    }
    match path {
        "/app/decision-seats" => with_payload(action, body, |payload| save_seats(root, payload)),
        "/app/decision-key" => {
            with_payload(action, body, |payload| {
                match decision_seats::save_key(root, &payload) {
                    Ok(key_set) => Response::json(&json!({"key_set": key_set})),
                    Err(error) => Response::error(400, &error),
                }
            })
        }
        _ => Response::not_found(),
    }
}

/// The saved file (null while missing), why it cannot be edited, and whether a key is set; a key
/// is never part of the answer.
fn state(root: &Path) -> Response {
    let (document, error) = match decision_seats::read(root) {
        Ok(document) => (document.unwrap_or(Value::Null), Value::Null),
        Err(error) => (Value::Null, json!(error)),
    };
    let (key_set, key_error) = match decision_seats::key_state(root) {
        Ok(set) => (set, Value::Null),
        Err(error) => (false, json!(error)),
    };
    Response::json(&json!({"document": document, "error": error,
        "key_set": key_set, "key_error": key_error}))
}

fn save_seats(root: &Path, mut document: Value) -> Response {
    decision_seats::normalize(&mut document);
    if let Err(error) = decision_seats::validate(&document) {
        return Response::error(400, &error);
    }
    match decision_seats::save(root, &document) {
        Ok(()) => Response::json(&json!({"saved": true})),
        Err(error) => Response::error(500, &error),
    }
}

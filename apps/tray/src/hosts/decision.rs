//! The routes of the decision-model seat experiment (ADR-0087, ADR-0123): the catalog, the decision
//! sources' keys and their model catalogs. A preview build refuses the writes in the entry like
//! every other; reading a catalog writes nothing, so it stays open there.
use crate::decision_seats;
use crate::response::{Response, with_payload};
use serde_json::{Value, json};
use std::path::Path;

/// `path` may carry a query, which only the model catalog reads.
pub(crate) fn decision_request(
    root: &Path,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    match (method, route) {
        ("GET", "/app/decision-seats") => return state(root),
        ("GET", "/app/decision-models") => return models(root, query),
        (_, "/app/decision-models") => return Response::error(405, "此路由只接受 GET"),
        _ => {}
    }
    if method != "POST" {
        return Response::error(405, "此路由只接受 GET 或 POST");
    }
    match route {
        "/app/decision-seats" => with_payload(action, body, |payload| save_seats(root, payload)),
        "/app/decision-key" => with_payload(action, body, |payload| save_key(root, &payload)),
        _ => Response::not_found(),
    }
}

/// The saved file (null while missing), why it cannot be edited, which sources have a key (the
/// `key_set` of the one in use), and the sources the page can offer; a key is never part of the
/// answer.
fn state(root: &Path) -> Response {
    let (document, error) = match decision_seats::read(root) {
        Ok(document) => (document.unwrap_or(Value::Null), Value::Null),
        Err(error) => (Value::Null, json!(error)),
    };
    let (keys, key_error) = match decision_seats::keys_state(root) {
        Ok(keys) => (keys, Value::Null),
        Err(error) => (decision_seats::no_keys(), json!(error)),
    };
    let current = document["decision_provider"].as_str().unwrap_or("gemini");
    Response::json(&json!({"document": document, "error": error,
        "key_set": keys[current] == true, "keys": keys, "key_error": key_error,
        "providers": decision_seats::providers()}))
}

fn save_key(root: &Path, payload: &Value) -> Response {
    let saved = decision_seats::save_key(root, payload)
        .and_then(|provider| decision_seats::keys_state(root).map(|keys| (provider, keys)));
    match saved {
        Ok((provider, keys)) => Response::json(&json!({"provider": provider,
            "key_set": keys[provider] == true, "keys": keys})),
        Err(error) => Response::error(400, &error),
    }
}

/// `?provider=<id>[&base_url=<url>][&refresh=1]`: the models the source lists now.
fn models(root: &Path, query: &str) -> Response {
    let Ok(url) = reqwest::Url::parse(&format!("http://local/?{query}")) else {
        return Response::error(400, "查询参数无效");
    };
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let provider = param("provider").unwrap_or_default();
    let refresh = param("refresh").is_some_and(|value| value == "1");
    match decision_seats::fetch_models(root, &provider, param("base_url").as_deref(), refresh) {
        Ok(models) => Response::json(&json!({"provider": provider, "models": models})),
        Err((status, error)) => Response::error(status, &error),
    }
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

//! Opens GitHub links from the tray (`/app/git-pr/open-url`). The only action left from ADR-0115 §3;
//! the Git and PR write operations were removed by ADR-0126.

use serde_json::{Value, json};

use super::gh::validate_github_url;
use crate::response::{self, Response};

/// Protocol dispatcher for `/app/git-pr/*` action routes that are not read-only status routes.
pub(crate) fn request(method: &str, path: &str, action: Option<&str>, body: &[u8]) -> Response {
    match path {
        "/app/git-pr/open-url" => response::post(method, action, body, handle_open_url),
        _ => Response::not_found(),
    }
}

fn handle_open_url(payload: Value) -> Response {
    let raw = payload["url"].as_str().unwrap_or("");
    match validate_github_url(raw) {
        Ok(valid) => {
            crate::browse_address(&valid);
            Response::json(&json!({ "ok": true }))
        }
        Err(message) => Response::error(400, &message),
    }
}

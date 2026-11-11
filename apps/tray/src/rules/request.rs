use super::App;
use crate::response::{Response, post};
use serde_json::Value;
use std::path::Path;

pub(crate) fn request(
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
    apps: &[App],
    home: &Path,
    files: impl FnOnce() -> Value,
) -> Response {
    if path == "/app/rules" {
        if method != "GET" {
            return Response::error(405, "此路由只接受 GET");
        }
        return Response::json(&files());
    }
    post(method, action, body, |payload| {
        match super::open(apps, home, payload["path"].as_str().unwrap_or_default()) {
            Ok(result) => Response::json(&result),
            Err(error) => Response::error(400, &error),
        }
    })
}

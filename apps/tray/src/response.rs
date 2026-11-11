use serde_json::Value;
use serde_json::json;
use std::path::Path;

#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub cache_control: &'static str,
    /// Extra response headers, such as a partial response's `Content-Range`.
    pub headers: Vec<(&'static str, String)>,
    /// Something the page should tell the user although the request succeeded.
    pub notice: Option<String>,
}

impl Response {
    pub(crate) fn ok(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            cache_control: "no-store",
            status: 200,
            content_type,
            body,
            headers: Vec::new(),
            notice: None,
        }
    }

    pub(crate) fn with_notice(mut self, notice: Option<String>) -> Self {
        self.notice = notice;
        self
    }

    pub fn json(value: &serde_json::Value) -> Self {
        Self::ok(
            "application/json; charset=utf-8",
            value.to_string().into_bytes(),
        )
    }

    pub(crate) fn not_found() -> Self {
        Self {
            cache_control: "no-store",
            status: 404,
            content_type: "text/plain; charset=utf-8",
            body: b"not found".to_vec(),
            headers: Vec::new(),
            notice: None,
        }
    }

    pub fn error(status: u16, message: &str) -> Self {
        let mut response = Self::json(&json!({"error": message}));
        response.status = status;
        response
    }
}

/// A write route: an explicit POST with the action header and a JSON body.
pub(crate) fn post(
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

pub(crate) fn with_payload(
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
pub(crate) fn save<T>(
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

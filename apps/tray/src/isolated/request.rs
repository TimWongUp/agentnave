//! HTTP route handlers for manual isolated-directory inspection and deletion.

use std::path::Path;

use crate::data_files::valid_id;
use crate::response::{Response, post};

use super::leases::valid_isolation_id;

pub(crate) fn request(
    root: &Path,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    match path {
        "/app/isolated/inspect" => post(method, action, body, |payload| {
            let (Some(id), Some(workspace)) = (
                payload["id"].as_str().filter(|id| valid_isolation_id(id)),
                payload["workspace"].as_str().filter(|ws| valid_id(ws)),
            ) else {
                return Response::error(400, "需要有效的隔离目录 ID 与工作区 ID");
            };
            Response::json(&super::release::inspect(root, id, workspace))
        }),
        "/app/isolated/delete" => post(method, action, body, |payload| {
            let (Some(id), Some(workspace), Some(confirmation)) = (
                payload["id"].as_str().filter(|id| valid_isolation_id(id)),
                payload["workspace"].as_str().filter(|ws| valid_id(ws)),
                payload["confirmation"]
                    .as_str()
                    .map(str::trim)
                    .filter(|token| !token.is_empty()),
            ) else {
                return Response::error(400, "需要有效的隔离目录 ID、工作区 ID 与确认摘要");
            };
            Response::json(&super::release::delete(root, id, workspace, confirmation))
        }),
        _ => Response::not_found(),
    }
}

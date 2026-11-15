use crate::data_files::valid_id;
use crate::response::{Response, post};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn request(
    root: &Path,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if path == "/app/conversations/rename" {
        return post(method, action, body, |payload| {
            let (Some(id), Some(title)) = (
                payload["id"].as_str().filter(|id| valid_id(id)),
                payload["title"].as_str(),
            ) else {
                return Response::error(400, "需要有效的会话 ID 与标题");
            };
            Response::json(&crate::manage::rename(root, id, title))
        });
    }
    let op: Option<fn(&Path, &str) -> Value> = match path {
        "/app/workspaces/handover" => Some(crate::manage::handover),
        "/app/workspaces/cancel-handover" => Some(crate::manage::cancel_handover),
        "/app/workspaces/forget" => Some(crate::manage::forget),
        _ => None,
    };
    if let Some(op) = op {
        return post(method, action, body, |payload| {
            let Some(group) = payload["group"].as_str().filter(|id| valid_id(id)) else {
                return Response::error(400, "需要有效的工作区 ID");
            };
            Response::json(&op(root, group))
        });
    }
    if path == "/app/conversations/delete" {
        return post(method, action, body, |payload| {
            delete_conversations(root, &payload)
        });
    }
    // The entry dispatches by the same paths; a mismatch answers 404 instead of panicking.
    Response::not_found()
}

fn delete_conversations(root: &Path, payload: &Value) -> Response {
    let Some(ids) = payload
        .get("ids")
        .and_then(Value::as_array)
        .filter(|ids| !ids.is_empty() && ids.iter().all(|id| id.as_str().is_some_and(valid_id)))
    else {
        return Response::error(400, "ids 必须为非空的有效会话 ID 数组");
    };
    let cutoff = match payload.get("updated_before") {
        None => None,
        Some(value) => match value.as_i64().filter(|cutoff| *cutoff > 0) {
            Some(cutoff) => Some(cutoff),
            None => return Response::error(400, "updated_before 必须是有效的毫秒时间"),
        },
    };
    let results: Vec<Value> = match cutoff {
        Some(cutoff) => crate::manage::cleanup(
            root,
            &ids.iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            cutoff,
        ),
        None => ids
            .iter()
            .map(|id| crate::manage::delete(root, id.as_str().unwrap()))
            .collect(),
    };
    Response::json(&json!({"results": results}))
}

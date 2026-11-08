//! Optional, validated character videos; preview writes stay outside production storage.
mod download;
mod install;
mod picker;
#[cfg(test)]
mod tests;

use crate::data::Response;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const CHARACTERS: [&str; 13] = [
    "claude", "codex", "deepseek", "gemini", "glm", "grok", "qwen", "kimi", "minimax", "hunyuan",
    "doubao", "muse", "default",
];
const POSTERS: [&str; 7] = [
    "idle", "working", "speaking", "review", "done", "sleep", "sad",
];
const CLIPS: [&str; 10] = [
    "idle", "working", "speaking", "review", "done", "sleep", "sad", "slack-1", "slack-2",
    "sleep-1",
];

fn valid_tier(tier: &str) -> bool {
    matches!(tier, "272" | "544")
}

fn files() -> impl Iterator<Item = PathBuf> {
    CHARACTERS.into_iter().flat_map(|character| {
        POSTERS
            .into_iter()
            .map(move |state| format!("{character}/{state}.webp").into())
            .chain(
                CLIPS
                    .into_iter()
                    .map(move |clip| format!("{character}/{clip}.mov").into()),
            )
    })
}

fn preview_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("com.agentnave.preview")
}

fn write_root(shared: &Path) -> PathBuf {
    if crate::PREVIEW {
        preview_root()
    } else {
        shared.to_owned()
    }
}

fn resolve(shared: &Path, tier: &str) -> Option<(PathBuf, &'static str)> {
    resolve_with_preview(shared, tier, crate::PREVIEW.then(preview_root).as_deref())
}

fn resolve_with_preview(
    shared: &Path,
    tier: &str,
    preview: Option<&Path>,
) -> Option<(PathBuf, &'static str)> {
    if let Some(preview) = preview {
        let dir = preview.join("video-packs").join(tier);
        if dir.join("stamp").is_file() {
            return Some((dir, "preview"));
        }
    }
    let dir = shared.join("video-packs").join(tier);
    dir.join("stamp").is_file().then_some((dir, "shared"))
}

pub fn status(shared: &Path) -> Value {
    let mut packs = json!({});
    for tier in ["272", "544"] {
        let pack = resolve(shared, tier);
        let stamp = pack
            .as_ref()
            .and_then(|(dir, _)| fs::read_to_string(dir.join("stamp")).ok());
        let bytes: u64 = pack.as_ref().map_or(0, |(dir, _)| {
            files()
                .filter_map(|file| fs::metadata(dir.join(file)).ok())
                .map(|m| m.len())
                .sum()
        });
        packs[tier] = json!({"installed": pack.is_some(), "stamp": stamp,
            "bytes": bytes, "source": pack.map(|(_, source)| source),
            "download": download::status(shared, tier)});
    }
    json!({"packs": packs})
}

pub fn asset(shared: &Path, rest: &str) -> Option<Response> {
    let parts: Vec<_> = rest.split('/').collect();
    let [tier, character, file] = parts.as_slice() else {
        return None;
    };
    if !valid_tier(tier) || !CHARACTERS.contains(character) {
        return None;
    }
    let mime = if file
        .strip_suffix(".webp")
        .is_some_and(|s| POSTERS.contains(&s))
    {
        "image/webp"
    } else if file
        .strip_suffix(".mov")
        .is_some_and(|s| CLIPS.contains(&s))
    {
        "video/quicktime"
    } else {
        return None;
    };
    let (dir, _) = resolve(shared, tier)?;
    let mut response = Response::ok(mime, fs::read(dir.join(character).join(file)).ok()?);
    response.cache_control = "public, max-age=31536000";
    Some(response)
}

pub fn request(
    shared: &Path,
    app: Option<&tauri::AppHandle>,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    if path == "/app/video-packs" {
        return if method == "GET" {
            Response::json(&status(shared))
        } else {
            Response::error(405, "此路由只接受 GET")
        };
    }
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    let ["app", "video-packs", tier, operation] = parts.as_slice() else {
        return Response::error(404, "未知视频包路由");
    };
    if !valid_tier(tier) || !matches!(*operation, "import" | "download" | "cancel" | "delete") {
        return Response::error(404, "未知视频包路由");
    }
    if method != "POST" {
        return Response::error(405, "此操作只接受 POST");
    }
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    if *operation == "delete"
        && crate::PREVIEW
        && !write_root(shared)
            .join("video-packs")
            .join(tier)
            .join("stamp")
            .is_file()
        && resolve(shared, tier).is_some()
    {
        return Response::error(403, "预览版不可删除正式版已装的视频包");
    }
    let result = match *operation {
        "import" => import(shared, app, tier, body),
        "download" => download::start(shared, tier).map(|()| status(shared)),
        "cancel" => {
            download::cancel(shared, tier);
            Ok(status(shared))
        }
        "delete" => download::delete(shared, tier).map(|()| status(shared)),
        _ => unreachable!(),
    };
    match result {
        Ok(value) => Response::json(&value),
        Err(error) => Response::error(400, &error),
    }
}

fn import(
    shared: &Path,
    app: Option<&tauri::AppHandle>,
    tier: &str,
    _body: &[u8],
) -> Result<Value, String> {
    #[cfg(test)]
    if let Ok(value) = serde_json::from_slice::<Value>(_body)
        && let Some(path) = value["path"].as_str()
    {
        download::import_dir(shared, tier, Path::new(path))?;
        return Ok(status(shared));
    }
    let chosen = picker::choose(app.ok_or("无法打开文件夹选择器")?)?;
    if let Some(chosen) = chosen {
        download::import_dir(shared, tier, &chosen)?;
        Ok(status(shared))
    } else {
        let mut value = status(shared);
        value["cancelled"] = json!(true);
        Ok(value)
    }
}

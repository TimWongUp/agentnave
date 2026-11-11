//! Data directory information and read-only age-cleanup estimates.
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::response::Response;

fn bytes(root: &Path) -> io::Result<u64> {
    let mut directories = vec![root.to_owned()];
    let mut total = 0u64;
    while let Some(directory) = directories.pop() {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            // Do not follow links outside the data root or recurse through cycles.
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.push(entry.path());
            } else if kind.is_file() {
                match entry.metadata() {
                    Ok(metadata) => total = total.saturating_add(metadata.len()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(total)
}

pub fn request(root: &Path, method: &str, path: &str, action: Option<&str>) -> Response {
    if path == "/app/storage/open" {
        return crate::response::action(method, action, || match open(root) {
            Ok(()) => Response::json(&json!({"opened": true})),
            Err(error) => Response::error(500, &error.to_string()),
        });
    }
    if method != "GET" {
        return Response::error(405, "此路由只接受 GET");
    }
    if path == "/app/storage" {
        return match bytes(root) {
            Ok(bytes) => Response::json(&json!({"path": root, "bytes": bytes,
                "retention_days": crate::settings::state(root)["settings"]["history_retention_days"]})),
            Err(error) => Response::error(500, &error.to_string()),
        };
    }
    if let Some(query) = path.strip_prefix("/app/storage/preview?") {
        let days = query
            .strip_prefix("days=")
            .and_then(|days| days.parse::<u64>().ok());
        let Some(days @ (30 | 90 | 180)) = days else {
            return Response::error(400, "days 必须为 30、90 或 180");
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let cutoff =
            i64::try_from(now.saturating_sub(u128::from(days) * 86_400_000)).unwrap_or(i64::MAX);
        return match crate::manage::retention_preview(root, cutoff) {
            Ok(count) => Response::json(&json!({"workspaces": count})),
            Err(error) => Response::error(500, &error),
        };
    }
    Response::error(400, "缺少有效的存储路由或 days 参数")
}

fn open(root: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let launcher = "xdg-open";
    Command::new(launcher).arg(root).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests;

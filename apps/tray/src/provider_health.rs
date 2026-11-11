//! Shared CLI cooldown state. Writers use the server's disposable OS lock.
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

const FILE: &str = "provider-health.json";

pub fn read(root: &Path) -> Value {
    let document = fs::read(root.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    match document {
        Some(d) if d["version"].as_i64() == Some(1) && d["providers"].is_object() => {
            d["providers"].clone()
        }
        _ => json!({}),
    }
}

#[cfg(unix)]
fn same_file(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}
#[cfg(not(unix))]
fn same_file(_file: &File, path: &Path) -> bool {
    path.exists()
}

pub fn clear(root: &Path, provider: &str, model: Option<&str>) -> Result<(), String> {
    let lock_path = root.join("locks/provider-health.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).map_err(|e| e.to_string())?;
    for _ in 0..5 {
        let mut options = OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(3); // Match Python: readers/writers may open, deletion waits.
        }
        let lock = options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|_| "CLI 冷却状态正忙，请重试".to_string())?;
        if !same_file(&lock, &lock_path) {
            continue;
        }
        let result = clear_locked(root, provider, model);
        #[cfg(unix)]
        let _ = fs::remove_file(&lock_path);
        drop(lock);
        #[cfg(not(unix))]
        let _ = fs::remove_file(&lock_path);
        return result;
    }
    Err("CLI 冷却状态正忙，请重试".into())
}

fn clear_locked(root: &Path, provider: &str, model: Option<&str>) -> Result<(), String> {
    let bytes = match fs::read(root.join(FILE)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let mut document: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if document["version"].as_i64() != Some(1) {
        return Err("不支持的冷却文件版本".into());
    }
    let providers = document["providers"]
        .as_object_mut()
        .ok_or("冷却文件格式错误")?;
    let changed = if let Some(item) = providers.get_mut(provider) {
        if item.get("code").is_some() {
            if model.is_none() {
                providers.remove(provider);
                true
            } else {
                false
            }
        } else if let Some(object) = item.as_object_mut() {
            let removed = if let Some(name) = model {
                object
                    .get_mut("models")
                    .and_then(Value::as_object_mut)
                    .and_then(|models| models.remove(name))
                    .is_some()
            } else {
                object.remove("default").is_some()
            };
            if object
                .get("models")
                .and_then(Value::as_object)
                .is_some_and(|m| m.is_empty())
            {
                object.remove("models");
            }
            if object.is_empty() {
                providers.remove(provider);
            }
            removed
        } else {
            false
        }
    } else {
        false
    };
    if changed {
        crate::data_files::write_atomic(root, FILE, &document)?;
    }
    Ok(())
}

pub(crate) fn request(
    root: &std::path::Path,
    method: &str,
    action: Option<&str>,
    body: &[u8],
) -> crate::response::Response {
    use crate::response::{Response, post};

    post(method, action, body, |payload| {
        let Some(provider) = payload["provider"]
            .as_str()
            .filter(|p| crate::providers::PROVIDERS.contains(p))
        else {
            return Response::error(400, "需要有效的 CLI");
        };
        if !payload["model"].is_null() && payload["model"].as_str().is_none_or(str::is_empty) {
            return Response::error(400, "需要有效的模型");
        }
        match crate::provider_health::clear(root, provider, payload["model"].as_str()) {
            Ok(()) => Response::json(&json!({"cleared": true})),
            Err(error) => Response::error(400, &error),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_one_model_keeps_siblings() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE);
        fs::write(&path, r#"{"version":1,"providers":{"claude":{"models":{"a":{"code":"auth","until":null},"b":{"code":"billing","until":"future"}}}}}"#).unwrap();
        clear(root.path(), "claude", Some("a")).unwrap();
        assert!(read(root.path())["claude"]["models"].get("a").is_none());
        assert_eq!(
            read(root.path())["claude"]["models"]["b"]["code"],
            "billing"
        );
    }
    #[test]
    fn clear_preserves_other_providers_and_unknown_versions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(FILE);
        fs::write(&path, r#"{"version":1,"providers":{"claude":{"code":"auth","until":"future"},"grok":{"code":"quota","until":"future"}}}"#).unwrap();
        clear(root.path(), "claude", None).unwrap();
        assert!(read(root.path()).get("claude").is_none());
        assert_eq!(read(root.path())["grok"]["code"], "quota");
        let future = r#"{"version":2,"providers":{"grok":{}}}"#;
        fs::write(&path, future).unwrap();
        assert!(clear(root.path(), "grok", None).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), future);
    }
}

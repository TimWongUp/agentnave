//! Remove only AgentNave's global Antigravity hook, preserving unrelated entries.
use crate::config_paths::safe_path;
use serde_json::Value;
use std::{fs, io::Write, path::Path};

pub(super) fn remove_interactive(home: &Path) -> Result<(), String> {
    let path = home.join(".gemini/config/hooks.json");
    for _ in 0..3 {
        safe_path(&path, home)?;
        let original = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("write_failed:{error}")),
        };
        let mut document: Value =
            serde_json::from_slice(&original).map_err(|error| format!("parse_failed:{error}"))?;
        let entries = document
            .as_object_mut()
            .ok_or("parse_failed:hooks.json 必须为对象")?;
        if entries.remove("agentnave-interactive").is_none() {
            return Ok(());
        }
        let empty = entries.is_empty();
        let parent = path.parent().ok_or("write_failed:配置路径无父目录")?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| format!("write_failed:{error}"))?;
        tmp.as_file()
            .set_permissions(
                fs::metadata(&path)
                    .map_err(|error| format!("write_failed:{error}"))?
                    .permissions(),
            )
            .map_err(|error| format!("write_failed:{error}"))?;
        serde_json::to_writer_pretty(&mut tmp, &document)
            .map_err(|error| format!("write_failed:{error}"))?;
        tmp.write_all(b"\n")
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|error| format!("write_failed:{error}"))?;
        safe_path(&path, home)?;
        if fs::read(&path).map_err(|error| format!("write_failed:{error}"))? != original {
            continue;
        }
        if empty {
            fs::remove_file(&path).map_err(|error| format!("write_failed:{error}"))?;
        } else {
            tmp.persist(&path)
                .map_err(|error| format!("write_failed:{error}"))?;
        }
        return Ok(());
    }
    Err("conflict:hooks.json 连续发生变化，请重试".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_our_hook_and_deletes_an_empty_file() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let path = home.join(".gemini/config/hooks.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"agentnave-interactive":{},"other":{"enabled":true}}"#,
        )
        .unwrap();
        remove_interactive(&home).unwrap();
        let result: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(result, serde_json::json!({"other":{"enabled":true}}));
        fs::write(&path, r#"{"agentnave-interactive":{}}"#).unwrap();
        remove_interactive(&home).unwrap();
        assert!(!path.exists());
    }
}

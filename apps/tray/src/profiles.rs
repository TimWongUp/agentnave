//! User-owned profiles: structural checks only; Python Adapters validate option semantics.

use std::collections::HashSet;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use serde_json::{Value, json};

pub fn validate(document: &Value) -> Result<(), String> {
    let object = document.as_object().ok_or("档案文件必须为 JSON 对象")?;
    if object.len() != 2 || document["version"].as_u64() != Some(1) {
        return Err("档案文件必须只含 version: 1 与 profiles 数组".into());
    }
    let profiles = document["profiles"]
        .as_array()
        .ok_or("profiles 必须为数组")?;
    let mut ids = HashSet::new();
    for profile in profiles {
        let fields = profile.as_object().ok_or("每条档案必须为对象")?;
        if fields.len() != 5
            || ["id", "name", "provider", "provider_options", "notes"]
                .iter()
                .any(|key| !fields.contains_key(*key))
        {
            return Err("档案必须只含 id、name、provider、provider_options、notes".into());
        }
        for key in ["id", "name"] {
            if !profile[key].as_str().is_some_and(|s| !s.trim().is_empty()) {
                return Err(format!("{key} 必须为非空字符串"));
            }
        }
        if !ids.insert(profile["id"].as_str().unwrap()) {
            return Err("档案 id 不可重复".into());
        }
        if !matches!(
            profile["provider"].as_str(),
            Some("codex" | "claude" | "codebuddy" | "grok" | "antigravity" | "pi")
        ) {
            return Err("请选择受支持的 CLI".into());
        }
        if !profile["notes"].is_string() {
            return Err("notes 必须为字符串".into());
        }
        let options = profile["provider_options"]
            .as_object()
            .ok_or("provider_options 必须为对象")?;
        if options
            .values()
            .any(|value| !(value.is_string() || value.is_number() || value.is_boolean()))
        {
            return Err("参数值只支持字符串、数字或布尔值".into());
        }
    }
    Ok(())
}

pub fn read(root: &Path) -> Result<Value, String> {
    let content = match fs::read(root.join("profiles.json")) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(json!({"version": 1, "profiles": []}));
        }
        Err(error) => return Err(format!("无法读取 profiles.json：{error}")),
    };
    let document =
        serde_json::from_slice(&content).map_err(|error| format!("profiles.json 损坏：{error}"))?;
    validate(&document).map_err(|error| format!("profiles.json 格式错误：{error}"))?;
    Ok(document)
}

pub fn save(root: &Path, document: &Value) -> Result<(), String> {
    validate(document)?;
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root)?;
        // A unique sibling file keeps replacement atomic on the same filesystem. Dropping
        // the temporary file cleans it up on write/persist failures, leaving the old file intact.
        let mut temporary = tempfile::NamedTempFile::new_in(root)?;
        temporary.write_all(&serde_json::to_vec_pretty(document)?)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(root.join("profiles.json"))?;
        Ok(())
    };
    write().map_err(|error| format!("无法保存 profiles.json：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Reader;

    fn document() -> Value {
        json!({"version": 1, "profiles": [{
            "id": "stable-id", "name": "代码核验", "provider": "codex",
            "provider_options": {"model": "chosen", "effort": "high", "unknown": true, "budget": 2.5},
            "notes": "只读核验与理由"
        }]})
    }

    #[test]
    fn saves_require_post_and_action_header_and_replace_the_complete_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let reader = Reader::new(root.clone());
        assert_eq!(read(&root).unwrap()["profiles"], json!([]));
        assert!(!root.exists());
        let body = serde_json::to_vec(&document()).unwrap();
        assert_eq!(
            reader
                .request("GET", "/app/profiles", Some("1"), &body)
                .status,
            405
        );
        assert_eq!(
            reader.request("POST", "/app/profiles", None, &body).status,
            403
        );
        assert!(!root.exists());
        assert_eq!(
            reader
                .request("POST", "/app/profiles", Some("1"), b"{")
                .status,
            400
        );
        assert_eq!(
            reader
                .request("POST", "/app/profiles", Some("1"), &body)
                .status,
            200
        );
        assert_eq!(read(&root).unwrap(), document());
        assert_eq!(reader.respond("/settings/state").status, 200);
        save(&root, &json!({"version": 1, "profiles": []})).unwrap();
        assert_eq!(read(&root).unwrap()["profiles"], json!([]));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        // Rename cannot replace a directory; the old destination and no temporary file remain.
        fs::remove_file(root.join("profiles.json")).unwrap();
        fs::create_dir(root.join("profiles.json")).unwrap();
        assert!(save(&root, &document()).is_err());
        assert!(root.join("profiles.json").is_dir());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn malformed_structure_is_rejected_without_changing_saved_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        save(root, &document()).unwrap();
        let reader = Reader::new(root.to_path_buf());
        let mut malformed = vec![Value::Null, json!({"version": 2, "profiles": []})];
        for (key, value) in [
            ("id", json!(" ")),
            ("name", json!(3)),
            ("provider", json!("other")),
            ("notes", json!(null)),
            ("provider_options", json!({"x": []})),
            ("provider_options", json!({"x": null})),
        ] {
            let mut bad = document();
            bad["profiles"][0][key] = value;
            malformed.push(bad);
        }
        let mut duplicate = document();
        duplicate["profiles"]
            .as_array_mut()
            .unwrap()
            .push(document()["profiles"][0].clone());
        malformed.push(duplicate);
        for bad in malformed {
            let response = reader.request(
                "POST",
                "/app/profiles",
                Some("1"),
                &serde_json::to_vec(&bad).unwrap(),
            );
            assert_eq!(response.status, 400, "{bad}");
            assert_eq!(read(root).unwrap(), document());
        }
        fs::write(root.join("profiles.json"), b"{").unwrap();
        assert_eq!(reader.respond("/settings/state").status, 500);
        assert_eq!(fs::read(root.join("profiles.json")).unwrap(), b"{");
    }
}

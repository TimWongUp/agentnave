//! Only native model listings establish TRAE model availability.
use super::*;

pub(super) fn models(path: &Path) -> Result<Vec<Value>, String> {
    let output = run(path, &["models", "--json"])?;
    listing(&output)
}

fn listing(output: &str) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_str(output).map_err(|_| "TRAE 模型目录不是有效 JSON")?;
    let entries = value.as_array().ok_or("TRAE 模型目录必须为数组")?;
    if entries.is_empty() {
        return Err("TRAE 未返回可用模型；请登录 TRAE CLI 后刷新".into());
    }
    // CLI 0.213.1 emits `name`, not app-server Model's `id`/`displayName`. Verified with
    // `models --json -c model_provider="openai" -c model_catalog_json="<local probe>"`
    // in an isolated TRAE_HOME: a synthetic catalog slug is emitted unchanged as `name`.
    // This projection carries no default or effort contract; the local probe is never a fallback.
    let mut models = Vec::new();
    for entry in entries {
        let name = entry["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .ok_or("TRAE 模型条目缺少有效的 name")?;
        if !models.iter().any(|listed: &Value| listed["id"] == name) {
            models.push(model(name, name, false));
        }
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_listing_uses_names_without_inventing_defaults_or_efforts() {
        // The shape emitted by the real CLI for a synthetic local catalog, with a second
        // entry and duplicate added to check order and unique model identities.
        let output = json!([
            {"name":"agentnave-structure-probe", "description":"AgentNave description probe",
                "context_window":272000, "supported_mime_types":["image/*"],
                "_meta":{"trae":{"contextWindow":272000, "maxContextWindow":272000,
                    "supportsMaxMode":false, "isEnterpriseCustomModel":false,
                    "isUserCustomModel":false}}},
            {"name":"second-probe"},
            {"name":"agentnave-structure-probe"}
        ]);
        assert_eq!(
            listing(&output.to_string()).unwrap(),
            vec![
                json!({"id":"agentnave-structure-probe", "label":"agentnave-structure-probe", "default":false}),
                json!({"id":"second-probe", "label":"second-probe", "default":false})
            ]
        );
    }

    #[test]
    fn empty_or_malformed_listing_never_invents_models() {
        assert!(listing("[]").unwrap_err().contains("登录"));
        for output in [
            "{}",
            "not json",
            r#"[{"id":"app-server-shape","displayName":"other contract"}]"#,
            r#"[{"name":42}]"#,
            r#"[{"name":" "}]"#,
            r#"[{"name":"valid"},{}]"#,
        ] {
            assert!(listing(output).is_err(), "{output}");
        }
    }
}

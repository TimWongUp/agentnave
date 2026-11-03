use serde_json::Value;

pub(super) fn validate(field: &str, value: &Value) -> Result<(), String> {
    match field {
        "notify" => super::one_of(value, &["off", "system"], "alerts.notify"),
        "scope" => super::one_of(value, &["all", "meetings"], "alerts.scope"),
        "threshold" if value.as_u64().is_some_and(|n| [0, 10, 30].contains(&n)) => Ok(()),
        "threshold" => Err("alerts.threshold 必须为 0、10、30（分钟）".into()),
        "problems" if value.is_boolean() => Ok(()),
        "problems" => Err("alerts.problems 必须为布尔值".into()),
        _ => Err(format!("未知的设置项：alerts.{field}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn validates_alerts_without_dropping_unknown_saved_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("preferences.json"),
            br#"{"alerts":{"future":1}}"#,
        )
        .unwrap();
        let values = json!({"alerts": {
            "notify": "system", "scope": "meetings", "threshold": 30, "problems": true
        }});
        let saved = crate::preferences::update(dir.path(), &values).unwrap();
        assert_eq!(saved["alerts"]["future"], 1);
        for patch in [
            json!({"notify": "yes"}),
            json!({"scope": "tasks"}),
            json!({"threshold": 10.0}),
            json!({"threshold": -1}),
            json!({"threshold": 20}),
            json!({"problems": 1}),
            json!({"typo": false}),
        ] {
            assert!(crate::preferences::update(dir.path(), &json!({"alerts": patch})).is_err());
        }
    }
}

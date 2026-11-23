use serde_json::Value;

use crate::isolated::cleanup::IDLE_DAYS;

/// Rules of the automatic isolated-directory cleanup (ADR-0115 §6); every one defaults to off.
pub(super) fn validate(field: &str, value: &Value) -> Result<(), String> {
    match field {
        "merged" | "unchanged" | "workspace_deleted" if value.is_boolean() => Ok(()),
        "merged" | "unchanged" | "workspace_deleted" => {
            Err(format!("isolated_cleanup.{field} 必须为布尔值"))
        }
        "idle_days"
            if value.is_null()
                || value
                    .as_u64()
                    .is_some_and(|days| IDLE_DAYS.iter().any(|d| u64::from(*d) == days)) =>
        {
            Ok(())
        }
        "idle_days" => Err("isolated_cleanup.idle_days 必须为 null、7、14、30 或 90".into()),
        _ => Err(format!("未知的设置项：isolated_cleanup.{field}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::isolated::cleanup::Rules;

    #[test]
    fn rules_default_off_and_only_known_choices_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let none = crate::preferences::read(dir.path()).unwrap();
        assert!(!Rules::from_document(&none).any());
        let saved = crate::preferences::update(
            dir.path(),
            &json!({"isolated_cleanup": {
                "merged": true, "unchanged": false, "idle_days": 30, "workspace_deleted": true
            }}),
        )
        .unwrap();
        let rules = Rules::from_document(saved.as_object().unwrap());
        assert_eq!(
            rules,
            Rules {
                merged: true,
                unchanged: false,
                idle_days: Some(30),
                workspace_deleted: true
            }
        );
        let off = crate::preferences::update(
            dir.path(),
            &json!({"isolated_cleanup": {"idle_days": null}}),
        )
        .unwrap();
        assert_eq!(
            Rules::from_document(off.as_object().unwrap()).idle_days,
            None
        );
        for patch in [
            json!({"merged": 1}),
            json!({"idle_days": 10}),
            json!({"idle_days": "7"}),
            json!({"typo": true}),
        ] {
            assert!(
                crate::preferences::update(dir.path(), &json!({"isolated_cleanup": patch}))
                    .is_err()
            );
        }
    }
}

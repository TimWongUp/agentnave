//! Where the first-run checklist stands: shown, finished, or skipped by the user.
use serde_json::Value;

pub(super) fn validate(field: &str, value: &Value) -> Result<(), String> {
    match field {
        "state" => super::one_of(value, &["active", "done", "skipped"], "onboarding.state"),
        _ => Err(format!("未知的设置项：onboarding.{field}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn saves_only_known_states_and_keeps_other_sections() {
        let dir = tempfile::tempdir().unwrap();
        crate::preferences::update(dir.path(), &json!({"general": {"language": "en"}})).unwrap();
        let saved =
            crate::preferences::update(dir.path(), &json!({"onboarding": {"state": "skipped"}}))
                .unwrap();
        assert_eq!(saved["onboarding"]["state"], "skipped");
        assert_eq!(saved["general"]["language"], "en");
        for patch in [json!({"state": "finished"}), json!({"step": 1})] {
            assert!(crate::preferences::update(dir.path(), &json!({"onboarding": patch})).is_err());
        }
    }
}

use super::*;

#[test]
fn settings_default_validate_and_save_atomically() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
    assert_eq!(state(root.path())["saved"], false);
    for bad in [
        json!({"version": 1, "max_running": 0}),
        json!({"version": 1, "max_running": 33}),
        json!({"version": 1, "max_running": 2.5}),
        json!({"version": 1, "max_running": true}),
        json!({"version": 2, "max_running": 4}),
        json!({"version": 1.0, "max_running": 4}),
        json!({"version": true, "max_running": 4}),
        json!([]),
    ] {
        assert!(save(root.path(), &bad).is_err(), "{bad}");
    }
    assert!(!root.path().join(FILE).exists());
    save(root.path(), &json!({"version": 1, "max_running": 3})).unwrap();
    assert_eq!(max_running(root.path()), 3);
    assert_eq!(state(root.path())["settings"]["max_running"], 3);
    // Like the service: unknown keys are ignored when read, and saving keeps them.
    fs::write(
        root.path().join(FILE),
        br#"{"version":1,"max_running":2,"future":true}"#,
    )
    .unwrap();
    assert_eq!(max_running(root.path()), 2);
    assert_eq!(
        state(root.path())["settings"],
        json!({"version": 1, "max_running": 2, "history_retention_days": null, "claude_interactive": false})
    );
    save(
        root.path(),
        &json!({"version": 1, "max_running": 5, "x": 1}),
    )
    .unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(root.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({"version": 1, "max_running": 5, "future": true, "x": 1, "history_retention_days": null, "claude_interactive": false})
    );
    // Saving one other key keeps the saved limit, and a broken saved limit falls back.
    save(root.path(), &json!({"version": 1, "y": 2})).unwrap();
    assert_eq!(max_running(root.path()), 5);
    fs::write(
        root.path().join(FILE),
        br#"{"version":1,"max_running":99,"z":1}"#,
    )
    .unwrap();
    save(root.path(), &json!({"version": 1, "y": 3})).unwrap();
    assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
    assert_eq!(state(root.path())["error"], Value::Null);
    fs::write(root.path().join(FILE), br#"{"version":1}"#).unwrap();
    assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
    assert_eq!(state(root.path())["saved"], true);
    // A broken file keeps the default limit and tells the page why.
    fs::write(root.path().join(FILE), b"{oops").unwrap();
    assert_eq!(max_running(root.path()), DEFAULT_MAX_RUNNING);
    let broken = state(root.path());
    assert!(broken["error"].as_str().unwrap().contains("损坏"));
    assert_eq!(broken["settings"]["max_running"], DEFAULT_MAX_RUNNING);
}

#[test]
fn retention_is_strict_and_partial_saves_keep_it() {
    let root = tempfile::tempdir().unwrap();
    for days in [json!(null), json!(30), json!(90), json!(180)] {
        save(
            root.path(),
            &json!({"version": 1, "history_retention_days": days}),
        )
        .unwrap();
        save(root.path(), &json!({"version": 1, "max_running": 4})).unwrap();
        assert_eq!(
            state(root.path())["settings"]["history_retention_days"],
            days
        );
    }
    for days in [
        json!(0),
        json!(31),
        json!(-30),
        json!(30.0),
        json!(true),
        json!("30"),
    ] {
        assert!(
            save(
                root.path(),
                &json!({"version": 1, "history_retention_days": days})
            )
            .is_err()
        );
    }
}

#[test]
fn claude_interactive_defaults_off_and_partial_saves_preserve_other_settings() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(state(root.path())["settings"]["claude_interactive"], false);
    assert_eq!(
        validate(&json!({"version": 1})).unwrap()["claude_interactive"],
        false
    );
    save(
        root.path(),
        &json!({"version": 1, "max_running": 4, "history_retention_days": 90, "future": "keep"}),
    )
    .unwrap();
    for enabled in [true, false] {
        save(
            root.path(),
            &json!({"version": 1, "claude_interactive": enabled}),
        )
        .unwrap();
        save(root.path(), &json!({"version": 1, "max_running": 6})).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(root.path().join(FILE)).unwrap()).unwrap();
        assert_eq!(saved["claude_interactive"], enabled);
        assert_eq!(saved["max_running"], 6);
        assert_eq!(saved["history_retention_days"], 90);
        assert_eq!(saved["future"], "keep");
        assert_eq!(
            state(root.path())["settings"]["claude_interactive"],
            enabled
        );
    }
    for bad in [json!("yes"), json!(1), json!(null), json!([]), json!({})] {
        let document = json!({"version": 1, "claude_interactive": bad});
        assert!(save(root.path(), &document).is_err());
        fs::write(
            root.path().join(FILE),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let broken = state(root.path());
        assert!(
            broken["error"]
                .as_str()
                .unwrap()
                .contains("claude_interactive")
        );
        assert_eq!(broken["settings"]["claude_interactive"], false);
    }
    save(root.path(), &defaults()).unwrap();
    assert_eq!(state(root.path())["error"], Value::Null);
}

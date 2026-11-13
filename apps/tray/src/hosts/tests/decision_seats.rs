use super::*;

fn seat() -> Value {
    json!({"id": "s-1a2b3c", "kind": "custom", "role": "写代码", "description": "写代码",
        "provider": "codex", "model": "gpt-6.1-sol", "access": "write", "difficulty": "hard"})
}

fn connected_reader() -> (Reader, tempfile::TempDir, PathBuf) {
    let (_temp, mut hosts) = fixture();
    for id in ["claude", "grok", "codex"] {
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        assert_eq!(
            hosts.action("connect", &json!({"id": id})).unwrap()["ok"],
            true
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let reader = Reader::new(root.clone());
    reader.use_hosts(hosts);
    assert_eq!(
        reader
            .request("POST", "/app/hosts/refresh", Some("1"), b"")
            .status,
        200
    );
    // The fixture's temporary home must outlive the reader.
    std::mem::forget(_temp);
    (reader, directory, root)
}

fn post(reader: &Reader, path: &str, document: &Value) -> u16 {
    let body = serde_json::to_vec(document).unwrap();
    let response = reader.request("POST", path, Some("1"), &body);
    response.status
}

fn decision_file(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("decision-seats.json")).unwrap()).unwrap()
}

#[test]
fn a_body_that_is_not_an_object_is_refused_like_any_invalid_catalog() {
    let (reader, _directory, root) = connected_reader();
    for body in [
        json!([]),
        json!([seat()]),
        json!("seats"),
        json!(3),
        json!(true),
        json!(null),
    ] {
        assert_eq!(post(&reader, "/app/decision-seats", &body), 400, "{body}");
    }
    assert!(!root.join("decision-seats.json").exists());
    // A body that is not JSON at all is refused the same way, and an object still saves.
    assert_eq!(
        reader
            .request("POST", "/app/decision-seats", Some("1"), b"not json")
            .status,
        400
    );
    let document = json!({"version": 1, "seats": [seat()]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
}

#[test]
fn the_catalog_saves_on_its_own_and_a_page_switch_is_not_kept() {
    let (reader, _directory, root) = connected_reader();
    // Nothing is written until the page saves the file.
    assert!(!root.join("decision-seats.json").exists());
    let document = json!({"version": 1, "seats": [seat()]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
    let saved = decision_file(&root);
    assert_eq!(saved["decision_model"], "gemini-decision-1");
    assert_eq!(saved["seats"], json!([seat()]));
    // Fallbacks come back as saved.
    let mut with_fallbacks = seat();
    with_fallbacks["fallbacks"] = json!([{"provider": "grok", "model": "m", "effort": "high"}]);
    let document = json!({"version": 1, "seats": [with_fallbacks]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
    assert_eq!(decision_file(&root)["seats"], json!([with_fallbacks]));
    let document = json!({"version": 1, "seats": [seat()]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
    // Saving the catalog neither needs nor touches the modes.
    assert!(!root.join("orchestration.json").exists());
    // A boss's mode `decision` is saved like any other and leaves the catalog alone.
    let orchestration = json!({"version": 1, "preset": "off", "apps": {"grok": "decision"},
        "decision_options": {"meetings": true}});
    assert_eq!(post(&reader, "/app/orchestration", &orchestration), 200);
    let effective = crate::members::read_orchestration(&root).unwrap()["effective_apps"].clone();
    assert_eq!(effective["claude"], "decision");
    assert_eq!(effective["grok"], "decision");
    assert_eq!(effective["codex"], "off");
    assert_eq!(decision_file(&root)["seats"], json!([seat()]));
}

#[test]
fn a_catalog_the_service_would_refuse_is_not_saved() {
    let (reader, _directory, root) = connected_reader();
    let mut bad = seat();
    bad["id"] = json!("worker-easy");
    let document = json!({"version": 1, "seats": [bad]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 400);
    assert!(!root.join("decision-seats.json").exists());
    // A boss switch from an older page is not accepted.
    let document = json!({"version": 1, "apps": {"codex": true}, "seats": []});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 400);
    assert!(!root.join("decision-seats.json").exists());
}

#[test]
fn the_key_is_stored_privately_and_never_answered() {
    let (reader, _directory, root) = connected_reader();
    assert_eq!(
        post(
            &reader,
            "/app/decision-key",
            &json!({"key": "  AIza-secret-key  "})
        ),
        200
    );
    let file = fs::read_to_string(root.join("decision-keys.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&file).unwrap(),
        json!({"version": 1, "keys": {"gemini": "AIza-secret-key"}})
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join("decision-keys.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let state = reader.request("GET", "/app/decision-seats", None, b"");
    let text = String::from_utf8(state.body).unwrap();
    assert!(!text.contains("AIza"), "{text}");
    let state: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(state["key_set"], true);
    assert_eq!(state["document"], Value::Null);
    // Clearing keeps the file readable by the service.
    assert_eq!(post(&reader, "/app/decision-key", &json!({"key": ""})), 200);
    let file: Value =
        serde_json::from_slice(&fs::read(root.join("decision-keys.json")).unwrap()).unwrap();
    assert_eq!(file, json!({"version": 1, "keys": {}}));
    let state = reader.request("GET", "/app/decision-seats", None, b"");
    let state: Value = serde_json::from_slice(&state.body).unwrap();
    assert_eq!(state["key_set"], false);
    // A key with control characters, or a damaged file, is refused and the file left alone.
    assert_eq!(
        post(&reader, "/app/decision-key", &json!({"key": "a\nb"})),
        400
    );
    // Neither is one the service would refuse: a space inside, or anything but visible ASCII.
    for key in ["a b", "密钥", "a\u{a0}b"] {
        assert_eq!(
            post(&reader, "/app/decision-key", &json!({"key": key})),
            400
        );
    }
    fs::write(root.join("decision-keys.json"), "{broken").unwrap();
    let refused = reader.request("POST", "/app/decision-key", Some("1"), br#"{"key":"x"}"#);
    assert_eq!(refused.status, 400);
    assert!(!String::from_utf8_lossy(&refused.body).contains("broken"));
    assert_eq!(
        fs::read_to_string(root.join("decision-keys.json")).unwrap(),
        "{broken"
    );
}

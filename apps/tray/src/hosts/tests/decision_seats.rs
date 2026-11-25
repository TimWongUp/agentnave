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
    // The key is only recognizable: its masked text is answered, never its characters.
    assert_eq!(state["key_previews"], json!({"gemini": "••••-key"}));
    let stored = reader.request(
        "POST",
        "/app/decision-key",
        Some("1"),
        br#"{"provider":"openrouter","key":"sk-or-v1-abcdef0123456789"}"#,
    );
    let stored = String::from_utf8(stored.body).unwrap();
    assert!(!stored.contains("abcdef0123456789"), "{stored}");
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored["key_set"], true);
    assert_eq!(
        stored["key_previews"],
        json!({"gemini": "••••-key", "openrouter": "sk-o…6789"})
    );
    assert_eq!(
        post(
            &reader,
            "/app/decision-key",
            &json!({"provider": "openrouter", "key": ""})
        ),
        200
    );
    // Clearing keeps the file readable by the service.
    assert_eq!(post(&reader, "/app/decision-key", &json!({"key": ""})), 200);
    let file: Value =
        serde_json::from_slice(&fs::read(root.join("decision-keys.json")).unwrap()).unwrap();
    assert_eq!(file, json!({"version": 1, "keys": {}}));
    let state = reader.request("GET", "/app/decision-seats", None, b"");
    let state: Value = serde_json::from_slice(&state.body).unwrap();
    assert_eq!(state["key_set"], false);
    assert_eq!(state["key_previews"], json!({}));
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
    // A damaged file shows no previews, only the error.
    let (_, state) = get(&reader, "/app/decision-seats");
    assert_eq!(state["key_previews"], json!({}));
    assert!(state["key_error"].is_string());
}

fn get(reader: &Reader, path: &str) -> (u16, Value) {
    let response = reader.request("GET", path, None, b"");
    (
        response.status,
        serde_json::from_slice(&response.body).unwrap(),
    )
}

#[test]
fn keys_are_kept_for_each_source_and_the_state_follows_the_source_in_use() {
    let (reader, _directory, root) = connected_reader();
    let (status, state) = get(&reader, "/app/decision-seats");
    assert_eq!(status, 200);
    assert_eq!(
        state["keys"],
        json!({"gemini": false, "openai": false, "typesafe": false,
            "openrouter": false, "custom": false})
    );
    let ids: Vec<_> = state["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| one["id"].clone())
        .collect();
    assert_eq!(
        ids,
        [
            "gemini",
            "openai",
            "typesafe",
            "openrouter",
            "ollama",
            "custom"
        ]
    );
    let key = |provider: &str, key: &str| {
        let body = serde_json::to_vec(&json!({"provider": provider, "key": key})).unwrap();
        reader.request("POST", "/app/decision-key", Some("1"), &body)
    };
    let saved = key("openrouter", "or-secret-key");
    assert_eq!(saved.status, 200);
    let answer: Value = serde_json::from_slice(&saved.body).unwrap();
    assert_eq!(answer["provider"], "openrouter");
    assert_eq!(answer["key_set"], true);
    assert_eq!(answer["keys"]["openrouter"], true);
    assert_eq!(answer["keys"]["gemini"], false);
    assert!(!String::from_utf8_lossy(&saved.body).contains("secret"));
    // Without a source named, the old call still means Gemini.
    assert_eq!(
        post(
            &reader,
            "/app/decision-key",
            &json!({"key": "g-secret-key"})
        ),
        200
    );
    for refused in [
        key("ollama", "k").status,
        key("magpie", "k").status,
        key("custom", "bad key").status,
    ] {
        assert_eq!(refused, 400);
    }
    // key_set is that of the source the saved file names.
    let (_, state) = get(&reader, "/app/decision-seats");
    assert_eq!(state["key_set"], true);
    let document = json!({"version": 1, "decision_provider": "openrouter", "seats": [seat()]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
    let saved = decision_file(&root);
    assert_eq!(saved["decision_provider"], "openrouter");
    assert_eq!(saved["decision_model"], "liquid/d1.2-ultra");
    let (_, state) = get(&reader, "/app/decision-seats");
    assert_eq!(state["key_set"], true);
    assert_eq!(state["document"]["decision_provider"], "openrouter");
    key("openrouter", "");
    let (_, state) = get(&reader, "/app/decision-seats");
    assert_eq!(state["key_set"], false);
    assert_eq!(state["keys"]["gemini"], true);
    let text = serde_json::to_string(&state).unwrap();
    assert!(!text.contains("secret"), "{text}");
    // A source with no key is never "set".
    let document = json!({"version": 1, "decision_provider": "ollama", "decision_model": "m",
        "seats": []});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
    assert_eq!(get(&reader, "/app/decision-seats").1["key_set"], false);
    // A damaged key file is reported without keys.
    fs::write(root.join("decision-keys.json"), "{broken").unwrap();
    let (_, state) = get(&reader, "/app/decision-seats");
    assert!(state["key_error"].is_string());
    assert_eq!(state["keys"]["gemini"], false);
}

#[test]
fn a_source_the_service_would_refuse_is_not_saved() {
    let (reader, _directory, root) = connected_reader();
    for fields in [
        json!({"decision_provider": "magpie"}),
        json!({"decision_provider": "custom", "decision_model": "m"}),
        json!({"decision_provider": "openai", "decision_base_url": "https://a.example"}),
        json!({"decision_provider": "ollama", "decision_model": "m",
            "decision_base_url": "http://example.com"}),
    ] {
        let mut document = json!({"version": 1, "seats": [seat()]});
        for (name, value) in fields.as_object().unwrap() {
            document[name] = value.clone();
        }
        assert_eq!(
            post(&reader, "/app/decision-seats", &document),
            400,
            "{fields}"
        );
    }
    assert!(!root.join("decision-seats.json").exists());
    // The default source stays as older services read it.
    let document = json!({"version": 1, "decision_provider": "gemini", "seats": [seat()]});
    assert_eq!(post(&reader, "/app/decision-seats", &document), 200);
    assert_eq!(
        decision_file(&root),
        json!({"version": 1, "decision_model": "gemini-decision-1", "seats": [seat()]})
    );
}

#[test]
fn a_model_catalog_is_a_read_that_takes_its_query_and_writes_nothing() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let _ = stream.read(&mut [0; 4096]);
            let body = r#"{"models":[{"name":"sys:1b","capabilities":["decision"]}]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let (reader, _directory, root) = connected_reader();
    let before = fs::read_dir(&root).unwrap().count();
    let url = format!("http://{address}");
    let encoded = url.replace(':', "%3A").replace('/', "%2F");
    let (status, answer) = get(
        &reader,
        &format!("/app/decision-models?provider=ollama&base_url={encoded}&refresh=1"),
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["provider"], "ollama");
    assert_eq!(
        answer["models"],
        json!([{"id": "sys:1b", "name": "sys:1b"}])
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), before);
    // Anything but a GET is refused, and a source with no catalog or a bad address is a 400.
    assert_eq!(
        reader
            .request(
                "POST",
                "/app/decision-models?provider=ollama",
                Some("1"),
                b"{}"
            )
            .status,
        405
    );
    for path in [
        "/app/decision-models",
        "/app/decision-models?provider=gemini",
        "/app/decision-models?provider=custom",
        "/app/decision-models?provider=custom&base_url=http%3A%2F%2Fexample.com",
        "/app/decision-models?provider=typesafe",
    ] {
        assert_eq!(get(&reader, path).0, 400, "{path}");
    }
}

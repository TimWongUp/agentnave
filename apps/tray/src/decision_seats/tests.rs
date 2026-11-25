use super::*;

fn seat() -> Value {
    json!({"id": "s-1a2b3c", "kind": "custom", "role": "写代码", "description": "写代码",
        "provider": "codex", "model": "gpt-6.1-sol", "access": "write", "difficulty": "hard"})
}

fn document(seats: Vec<Value>) -> Value {
    json!({"version": 1, "seats": seats})
}

#[test]
fn what_the_fixtures_do_not_cover_is_refused_like_the_service_does() {
    assert!(validate(&document(vec![seat()])).is_ok());
    let with = |field: &str, value: Value| {
        let mut changed = seat();
        changed[field] = value;
        validate(&document(vec![changed]))
    };
    // A role is trimmed first, then held to 40 printable characters.
    assert!(with("role", json!(format!("  {}  ", "角".repeat(40)))).is_ok());
    for role in [
        json!("角".repeat(41)),
        json!("  "),
        json!("a\nb"),
        json!("a\u{200b}b"),
    ] {
        assert!(with("role", role.clone()).is_err(), "{role}");
    }
    // Fixed seat names and review roles belong to the service.
    for id in [
        "explorer-easy",
        "worker-frontend-hard",
        "image-generator",
        "adversarial-verifier",
    ] {
        assert!(with("id", json!(id)).is_err(), "{id}");
    }
    assert!(with("id", json!("worker")).is_ok());
    assert!(with("id", json!("a".repeat(128))).is_ok());
    assert!(with("effort", json!(3)).is_err());
    assert!(with("note", json!("x")).is_err());
    assert!(with("difficulty", json!("medium")).is_ok());
    // The file keeps to what the service reads: its own fields, 255 seats, the one decision model.
    let many = |count: usize| {
        document(
            (0..count)
                .map(|i| {
                    let mut numbered = seat();
                    numbered["id"] = json!(format!("s-{i}"));
                    numbered["role"] = json!(format!("专业{i}"));
                    numbered["description"] = json!(format!("专业{i}"));
                    numbered
                })
                .collect(),
        )
    };
    assert!(validate(&many(255)).is_ok() && validate(&many(256)).is_err());
    let mut file = document(vec![]);
    file["extra"] = json!(1);
    assert!(validate(&file).is_err());
    for ok in [json!(true), json!(false)] {
        let mut with_flag = document(vec![]);
        with_flag["random_candidates"] = ok;
        assert!(validate(&with_flag).is_ok());
    }
    for bad in [json!(1), json!("true"), json!(null)] {
        let mut with_flag = document(vec![]);
        with_flag["random_candidates"] = bad.clone();
        assert!(validate(&with_flag).is_err(), "{bad}");
    }
    // A card is a kind and a role with one seat per difficulty; a group has one name and keeps
    // its access; the specialty text is bounded; an image seat is write-only with no difficulty.
    let group = |id: &str, role: &str, difficulty: &str| {
        let mut seat = seat();
        seat["id"] = json!(id);
        seat["kind"] = json!("explorer");
        seat["role"] = json!(role);
        seat["access"] = json!("read");
        seat["difficulty"] = json!(difficulty);
        seat
    };
    let two = |a: Value, b: Value| validate(&document(vec![a, b]));
    assert!(
        two(
            group("a", "通用探索", "easy"),
            group("b", "通用探索", "hard")
        )
        .is_ok()
    );
    assert!(
        two(
            group("a", "通用探索", "easy"),
            group("b", "通用探索", "easy")
        )
        .is_err()
    );
    assert!(two(group("a", "通用探索", "easy"), group("b", "探索", "hard")).is_err());
    let mut writes = group("a", "通用探索", "easy");
    writes["access"] = json!("write");
    assert!(validate(&document(vec![writes])).is_err());
    assert!(with("kind", json!("gardening")).is_err());
    assert!(with("kind", json!("worker_writing")).is_ok());
    assert!(with("description", json!("长".repeat(200))).is_ok());
    assert!(with("description", json!("长".repeat(201))).is_err());
    assert!(with("description", json!("  ")).is_err());
    assert!(with("tags", json!(["coding"])).is_err());
    let mut difficulty_less = seat();
    difficulty_less
        .as_object_mut()
        .unwrap()
        .remove("difficulty");
    assert!(validate(&document(vec![difficulty_less.clone()])).is_err());
    difficulty_less["kind"] = json!("image");
    assert!(validate(&document(vec![difficulty_less.clone()])).is_ok());
    let mut other = difficulty_less.clone();
    other["id"] = json!("s-other");
    assert!(two(difficulty_less.clone(), other.clone()).is_err());
    other["role"] = json!("写实");
    assert!(two(difficulty_less.clone(), other).is_ok());
    for (field, value) in [("difficulty", json!("easy")), ("access", json!("read"))] {
        let mut bad = difficulty_less.clone();
        bad[field] = value;
        assert!(validate(&document(vec![bad])).is_err(), "{field}");
    }
    // The boss switches live in orchestration.json now.
    for key in ["apps", "effective_apps"] {
        let mut file = document(vec![]);
        file[key] = json!({"claude": true});
        assert!(validate(&file).is_err(), "{key}");
    }
}

#[test]
fn text_is_held_to_the_table_the_service_checks_too() {
    let with_role = |role: &str| {
        let mut changed = seat();
        changed["role"] = json!(role);
        changed["description"] = json!(role);
        validate(&document(vec![changed])).is_ok()
    };
    // Ordinary Chinese, English, emoji and an inner space are fine; white space at the ends is trimmed.
    for role in [
        "写代码 and code 🚀",
        "  数据库迁移\u{3000}",
        "a b",
        "a\u{2065}b",
    ] {
        assert!(with_role(role), "{role:?}");
    }
    // Soft hyphen, private use, zero width and bidi controls, white space inside and controls are not.
    for role in [
        "a\u{00ad}b",
        "a\u{e000}b",
        "a\u{f0000}b",
        "a\u{200b}b",
        "a\u{2066}b",
        "a\u{feff}b",
        "a\u{00a0}b",
        "a\u{3000}b",
        "a\nb",
        "a\u{1f}b",
        "\u{1f}ab",
        "a\u{85}b",
    ] {
        assert!(!with_role(role), "{role:?}");
    }
    // The role is trimmed by the same table before it is saved.
    let mut file = document(vec![{
        let mut padded = seat();
        padded["role"] = json!("\u{3000} 写代码\u{a0}");
        padded
    }]);
    normalize(&mut file);
    assert_eq!(file["seats"][0]["role"], "写代码");
}

#[test]
fn an_image_seat_may_carry_a_null_difficulty_and_a_card_one_access() {
    let mut image = seat();
    image["kind"] = json!("image");
    image["difficulty"] = Value::Null;
    assert!(validate(&document(vec![image.clone()])).is_ok());
    image["difficulty"] = json!("easy");
    assert!(validate(&document(vec![image])).is_err());
    // Any other kind still needs a real difficulty.
    let mut custom = seat();
    custom["difficulty"] = Value::Null;
    assert!(validate(&document(vec![custom])).is_err());
    // The seats of one card agree on their access.
    let (mut read, mut write) = (seat(), seat());
    read["difficulty"] = json!("easy");
    read["access"] = json!("read");
    write["id"] = json!("s-other");
    assert!(validate(&document(vec![read.clone(), write.clone()])).is_err());
    write["role"] = json!("另一张");
    write["description"] = json!("另一张");
    assert!(validate(&document(vec![read, write])).is_ok());
}

#[test]
fn a_document_that_is_not_an_object_is_left_alone_and_refused() {
    for original in [
        json!([1, 2]),
        json!("text"),
        json!(7),
        json!(false),
        json!(null),
    ] {
        let mut document = original.clone();
        normalize(&mut document);
        assert_eq!(document, original);
        assert!(validate(&document).is_err(), "{original}");
    }
    // Seats that are not objects are left for validate too.
    let mut odd = json!({"version": 1, "seats": [3, "a", [1]]});
    normalize(&mut odd);
    assert_eq!(odd["decision_model"], "gemini-decision-1");
    assert!(validate(&odd).is_err());
}

#[test]
fn the_page_copy_is_made_ready_and_a_saved_file_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = document(vec![{
        let mut padded = seat();
        padded["role"] = json!("  写代码 ");
        padded
    }]);
    assert!(read(dir.path()).unwrap().is_none());
    normalize(&mut file);
    save(dir.path(), &file).unwrap();
    let read = read(dir.path()).unwrap().unwrap();
    assert_eq!(read["seats"][0]["role"], "写代码");
    assert_eq!(read["decision_model"], "gemini-decision-1");
    // A damaged file is reported, not replaced by an empty catalog.
    std::fs::write(dir.path().join(FILE), "{broken").unwrap();
    assert!(self::read(dir.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(FILE)).unwrap(),
        "{broken"
    );
}

#[test]
fn fallbacks_are_ordered_picks_without_repeats_and_at_most_three() {
    let with = |fallbacks: Value| {
        let mut changed = seat();
        changed["fallbacks"] = fallbacks;
        validate(&document(vec![changed]))
    };
    let pick = |model: &str| json!({"provider": "grok", "model": model, "effort": "high"});
    assert!(with(json!([])).is_ok());
    assert!(with(json!([pick("a"), pick("b"), pick("c")])).is_ok());
    for bad in [
        json!([pick("a"), pick("b"), pick("c"), pick("d")]),
        json!([{"provider": "codex", "model": "gpt-6.1-sol"}]),
        json!([pick("a"), pick("a")]),
        json!([{"provider": "grok"}]),
        json!([{"provider": "grok", "model": "a", "role": "x"}]),
    ] {
        assert!(with(bad.clone()).is_err(), "{bad}");
    }
}

fn with_source(fields: Value) -> Value {
    let mut file = document(vec![seat()]);
    for (key, value) in fields.as_object().unwrap() {
        file[key] = value.clone();
    }
    file
}

#[test]
fn every_source_keeps_to_its_own_model_and_address_rules() {
    let ok = |fields: Value| validate(&with_source(fields)).is_ok();
    // The default source, spelled out or not, takes its one model and no address.
    assert!(ok(json!({})));
    assert!(ok(
        json!({"decision_provider": "gemini", "decision_model": "gemini-decision-1"})
    ));
    assert!(!ok(json!({"decision_model": "other"})));
    assert!(!ok(
        json!({"decision_provider": "gemini", "decision_base_url": "https://a.example"})
    ));
    assert!(ok(json!({"decision_provider": "openai"})));
    assert!(ok(
        json!({"decision_provider": "openai", "decision_model": "gpt-6-luna"})
    ));
    assert!(!ok(
        json!({"decision_provider": "openai", "decision_model": "gemini-decision-1"})
    ));
    // Fixed-address sources take any visible ASCII id of 1..=200 characters, and no address.
    for provider in ["typesafe", "openrouter"] {
        assert!(ok(json!({"decision_provider": provider})), "{provider}");
        assert!(ok(
            json!({"decision_provider": provider, "decision_model": "a/b:free"})
        ));
        assert!(ok(
            json!({"decision_provider": provider, "decision_model": "m".repeat(200)})
        ));
        for bad in [
            json!(""),
            json!("a b"),
            json!("模型"),
            json!("m".repeat(201)),
            json!(3),
        ] {
            assert!(
                !ok(json!({"decision_provider": provider, "decision_model": bad})),
                "{provider} {bad}"
            );
        }
        assert!(!ok(
            json!({"decision_provider": provider, "decision_base_url": "https://a.example"})
        ));
        // An empty address is no address.
        assert!(ok(
            json!({"decision_provider": provider, "decision_base_url": ""})
        ));
    }
    // Ollama needs a model; its address may be left out.
    assert!(!ok(json!({"decision_provider": "ollama"})));
    assert!(ok(
        json!({"decision_provider": "ollama", "decision_model": "sys:1b"})
    ));
    assert!(ok(
        json!({"decision_provider": "ollama", "decision_model": "m",
        "decision_base_url": "http://localhost:11434/v1"})
    ));
    // A custom endpoint needs both.
    assert!(!ok(
        json!({"decision_provider": "custom", "decision_model": "m"})
    ));
    assert!(!ok(
        json!({"decision_provider": "custom", "decision_model": "m", "decision_base_url": ""})
    ));
    assert!(!ok(
        json!({"decision_provider": "custom", "decision_base_url": "https://a.example/v1"})
    ));
    assert!(ok(
        json!({"decision_provider": "custom", "decision_model": "m",
        "decision_base_url": "https://a.example/v1"})
    ));
    // Unknown sources and wrongly typed fields are refused.
    for fields in [
        json!({"decision_provider": "magpie"}),
        json!({"decision_provider": 1}),
        json!({"decision_provider": "ollama", "decision_model": "m", "decision_base_url": 1}),
    ] {
        assert!(!ok(fields.clone()), "{fields}");
    }
}

#[test]
fn an_address_is_https_or_loopback_without_credentials_query_or_fragment() {
    let ollama = |url: &str| {
        validate(&with_source(
            json!({"decision_provider": "ollama", "decision_model": "m", "decision_base_url": url}),
        ))
    };
    for good in [
        "https://api.example.com/v1",
        " https://api.example.com/v1/ ",
        "http://127.0.0.1:11434",
        "http://localhost:11434/v1",
        "http://[::1]:8080",
        &format!("https://{}.example", "a".repeat(40)),
    ] {
        assert!(ollama(good).is_ok(), "{good}");
    }
    for bad in [
        "http://api.example.com/v1",
        "http://192.168.1.5:11434",
        "ftp://127.0.0.1",
        "https://user@api.example.com",
        "https://@api.example.com",
        "https:user:pass@api.example.com",
        "https://api.example.com/v1?key=1",
        "https://api.example.com/v1#frag",
        "not a url",
        &format!("https://api.example.com/{}", "a".repeat(512)),
    ] {
        assert!(ollama(bad).is_err(), "{bad}");
    }
    // 512 characters is the limit.
    let edge = format!(
        "https://a.example/{}",
        "p".repeat(512 - "https://a.example/".len())
    );
    assert_eq!(edge.len(), 512);
    assert!(ollama(&edge).is_ok());
    assert!(ollama(&format!("{edge}p")).is_err());
}

#[test]
fn the_page_copy_names_the_model_of_its_source_and_leaves_the_default_source_implicit() {
    let normal = |fields: Value| {
        let mut file = with_source(fields);
        normalize(&mut file);
        file
    };
    let keys = |file: &Value| {
        let mut keys: Vec<_> = file.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    // The default source stays what older services read, however the page spells it.
    for fields in [
        json!({}),
        json!({"decision_provider": "gemini"}),
        json!({"decision_provider": "gemini", "decision_base_url": ""}),
    ] {
        let file = normal(fields);
        assert_eq!(keys(&file), ["decision_model", "seats", "version"]);
        assert_eq!(file["decision_model"], "gemini-decision-1");
    }
    let bad_gemini =
        normal(json!({"decision_provider": "gemini", "decision_base_url": "https://a.example"}));
    assert!(validate(&bad_gemini).is_err());
    let file = normal(json!({"decision_provider": "openai"}));
    assert_eq!(file["decision_provider"], "openai");
    assert_eq!(file["decision_model"], "gpt-6-luna");
    assert!(file.get("decision_base_url").is_none());
    assert_eq!(
        normal(json!({"decision_provider": "typesafe"}))["decision_model"],
        "jev-latest"
    );
    assert_eq!(
        normal(json!({"decision_provider": "openrouter", "decision_model": " a/b "}))["decision_model"],
        "a/b"
    );
    // An empty address goes; a good one is saved as the service reads it; a bad one is refused later.
    let file = normal(
        json!({"decision_provider": "ollama", "decision_model": "m", "decision_base_url": " "}),
    );
    assert!(file.get("decision_base_url").is_none() && validate(&file).is_ok());
    let file = normal(json!({"decision_provider": "custom", "decision_model": "m",
        "decision_base_url": " https://a.example/v1/ "}));
    assert_eq!(file["decision_base_url"], "https://a.example/v1");
    let file = normal(json!({"decision_provider": "custom", "decision_model": "m",
        "decision_base_url": "http://a.example"}));
    assert_eq!(file["decision_base_url"], "http://a.example");
    assert!(validate(&file).is_err());
    // A model is never made up for a source that has none to offer.
    assert!(
        normal(json!({"decision_provider": "ollama"}))
            .get("decision_model")
            .is_none()
    );
    // A file saved with another source reads back with it.
    let dir = tempfile::tempdir().unwrap();
    let file = normal(json!({"decision_provider": "ollama", "decision_model": "m"}));
    save(dir.path(), &file).unwrap();
    assert_eq!(
        read(dir.path()).unwrap().unwrap()["decision_provider"],
        "ollama"
    );
}

#[test]
fn each_source_keeps_its_own_key_and_none_is_answered() {
    let dir = tempfile::tempdir().unwrap();
    let state = || keys_state(dir.path()).unwrap();
    let all_off = json!({"gemini": false, "openai": false, "typesafe": false,
        "openrouter": false, "custom": false});
    assert_eq!(state(), all_off);
    // No source named is the default one.
    assert_eq!(
        save_key(dir.path(), &json!({"key": "g-key"})).unwrap(),
        "gemini"
    );
    assert_eq!(
        save_key(
            dir.path(),
            &json!({"provider": "openrouter", "key": " or-key "})
        )
        .unwrap(),
        "openrouter"
    );
    assert_eq!(state()["gemini"], true);
    assert_eq!(state()["openrouter"], true);
    assert_eq!(state()["openai"], false);
    let file: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("decision-keys.json")).unwrap())
            .unwrap();
    assert_eq!(
        file,
        json!({"version": 1, "keys": {"gemini": "g-key", "openrouter": "or-key"}})
    );
    assert!(!state().to_string().contains("key"));
    // Clearing one leaves the others.
    save_key(dir.path(), &json!({"provider": "gemini", "key": ""})).unwrap();
    assert_eq!(state()["gemini"], false);
    assert_eq!(state()["openrouter"], true);
    // A source without a key, an unknown one and a bad key are refused and change nothing.
    for payload in [
        json!({"provider": "ollama", "key": "k"}),
        json!({"provider": "magpie", "key": "k"}),
        json!({"provider": 3, "key": "k"}),
        json!({"provider": "custom", "key": "a b"}),
        json!({"provider": "custom", "key": "密钥"}),
        json!({"provider": "custom", "key": "k".repeat(4097)}),
        json!({"provider": "custom"}),
    ] {
        assert!(save_key(dir.path(), &payload).is_err(), "{payload}");
    }
    assert_eq!(state()["custom"], false);
    assert!(
        save_key(
            dir.path(),
            &json!({"provider": "custom", "key": "k".repeat(4096)})
        )
        .is_ok()
    );
}

#[test]
fn the_sources_are_described_for_the_page() {
    let list = providers();
    let ids: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|one| one["id"].as_str().unwrap())
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
    let by = |id: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|one| one["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by("gemini")["fixed_model"], true);
    assert_eq!(by("gemini")["default_model"], "gemini-decision-1");
    assert_eq!(by("openrouter")["models"].as_array().unwrap().len(), 8);
    assert_eq!(by("openrouter")["models"][0]["id"], "liquid/d1.2-ultra");
    assert_eq!(by("typesafe")["models"][1]["id"], "jev-1.14.0");
    assert_eq!(by("ollama")["key"], "none");
    assert_eq!(by("ollama")["default_base_url"], "http://127.0.0.1:11434");
    assert_eq!(by("custom")["key"], "optional");
    assert_eq!(by("custom")["url_required"], true);
    assert_eq!(by("gemini")["catalog"], false);
    // Every default model is one the file accepts for its own source.
    for one in list.as_array().unwrap() {
        if let Some(model) = one["default_model"].as_str() {
            let file =
                with_source(json!({"decision_provider": one["id"], "decision_model": model}));
            assert!(validate(&file).is_ok(), "{one}");
        }
    }
}

mod catalog {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// A server on this machine that answers each request from `reply(path)` and records what it got.
    fn serve(
        reply: impl Fn(&str) -> (u16, Value) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (status, body) = reply(&path);
                record.lock().unwrap().push(request);
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (format!("http://{address}"), seen)
    }

    #[test]
    fn openrouter_and_typesafe_lists_are_read_as_their_services_answer() {
        let listed = models::parse_openrouter(&json!({"data": [
            {"id": "b/two", "name": "Two", "pricing": {"prompt": "0"}, "context_length": 4096},
            {"id": "a/one"},
            {"id": "a/one"},
            {"id": "bad id"},
            {"name": "no id"},
        ]}));
        assert_eq!(
            listed,
            [
                json!({"id": "a/one", "name": "a/one"}),
                json!({"id": "b/two", "name": "Two", "price": "0", "context_length": 4096}),
            ]
        );
        assert!(models::parse_openrouter(&json!({"data": "x"})).is_empty());
        let ids = |models: Vec<Value>| -> Vec<String> {
            models
                .iter()
                .map(|one| one["id"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(
            ids(models::parse_typesafe(
                &json!({"models": [{"name": "jev-2"}, {"name": "jev-1"}]})
            )),
            ["jev-1", "jev-2"]
        );
        assert_eq!(
            ids(models::parse_typesafe(&json!({"data": [{"id": "jev-3"}]}))),
            ["jev-3"]
        );
        assert_eq!(
            ids(models::parse_listed(&json!({"data": [{"id": "x"}]}))),
            ["x"]
        );
        assert_eq!(
            ids(models::parse_listed(
                &json!({"models": [{"name": "y"}, {"id": "z"}]})
            )),
            ["y", "z"]
        );
    }

    #[test]
    fn a_typesafe_catalog_needs_a_saved_key_and_a_fixed_source_has_no_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let (status, message) = fetch_models(dir.path(), "typesafe", None, true).unwrap_err();
        assert_eq!(status, 400);
        assert!(message.contains("密钥"), "{message}");
        for provider in ["gemini", "openai", "magpie", ""] {
            assert_eq!(
                fetch_models(dir.path(), provider, None, true)
                    .unwrap_err()
                    .0,
                400
            );
        }
        // A custom endpoint without an address, or with one the App would not call, is refused.
        for base in [
            None,
            Some("http://example.com"),
            Some("https://u@example.com"),
        ] {
            assert_eq!(
                fetch_models(dir.path(), "custom", base, true)
                    .unwrap_err()
                    .0,
                400,
                "{base:?}"
            );
        }
    }

    #[test]
    fn ollama_lists_only_what_can_decide_and_remembers_until_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let (base, seen) = serve(|path| match path {
            "/api/tags" => (
                200,
                json!({"models": [
                    {"name": "sys:1b", "capabilities": ["completion", "decision"]},
                    {"name": "llama:8b", "capabilities": ["completion"]},
                    {"name": "old:1b"},
                    {"model": "sys:7b", "capabilities": ["decision"]},
                ]}),
            ),
            _ => (404, json!({})),
        });
        let ids = |models: &[Value]| -> Vec<String> {
            models
                .iter()
                .map(|one| one["id"].as_str().unwrap().to_owned())
                .collect()
        };
        let listed = fetch_models(dir.path(), "ollama", Some(&base), false).unwrap();
        assert_eq!(ids(&listed), ["sys:1b", "sys:7b"]);
        // The route's own suffixes are not part of the server's address.
        for given in [
            format!("{base}/v1"),
            format!("{base}/v1/systemone/"),
            base.clone(),
        ] {
            assert_eq!(
                fetch_models(dir.path(), "ollama", Some(&given), false).unwrap(),
                listed
            );
        }
        assert_eq!(
            seen.lock().unwrap().len(),
            3,
            "each address is read once, then remembered"
        );
        fetch_models(dir.path(), "ollama", Some(&base), true).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 4);
        let request = seen.lock().unwrap().last().unwrap().to_lowercase();
        assert!(request.starts_with("get /api/tags "), "{request}");
        assert!(!request.contains("authorization"), "{request}");
        // A server that is down is reported, not remembered as empty.
        let (down, _) = serve(|_| (500, json!({})));
        let failed = fetch_models(dir.path(), "ollama", Some(&down), true).unwrap_err();
        assert_eq!(failed.0, 502);
        let (bad, _) = serve(|_| (200, json!("not a list")));
        assert!(
            fetch_models(dir.path(), "ollama", Some(&bad), true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_custom_endpoint_is_asked_for_its_models_with_the_saved_key() {
        let dir = tempfile::tempdir().unwrap();
        let (base, seen) = serve(|path| match path {
            "/v1/models" | "/models" | "/other/v1/models" => {
                (200, json!({"data": [{"id": "m-2"}, {"id": "m-1"}]}))
            }
            _ => (404, json!({})),
        });
        let first = |models: Vec<Value>| models[0]["id"].as_str().unwrap().to_owned();
        // Without a key nothing is sent; with one, as a bearer token.
        assert_eq!(
            first(fetch_models(dir.path(), "custom", Some(&base), true).unwrap()),
            "m-1"
        );
        assert!(
            !seen.lock().unwrap()[0]
                .to_lowercase()
                .contains("authorization")
        );
        save_key(dir.path(), &json!({"provider": "custom", "key": "sk-abc"})).unwrap();
        fetch_models(dir.path(), "custom", Some(&base), true).unwrap();
        let sent = seen.lock().unwrap()[1].to_lowercase();
        assert!(sent.contains("authorization: bearer sk-abc"), "{sent}");
        // The address as the service takes it: with /v1, with /systemone, or bare.
        for given in [
            format!("{base}/v1"),
            format!("{base}/v1/systemone"),
            format!("{base}/systemone"),
        ] {
            fetch_models(dir.path(), "custom", Some(&given), true).unwrap();
        }
        fetch_models(
            dir.path(),
            "custom",
            Some(&format!("{base}/other/v1")),
            true,
        )
        .unwrap();
        let paths: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.split_whitespace().nth(1).unwrap().to_owned())
            .collect();
        assert_eq!(
            paths,
            [
                "/v1/models",
                "/v1/models",
                "/v1/models",
                "/v1/models",
                "/v1/models",
                "/other/v1/models"
            ]
        );
        // The saved address stands in when the page gives none, for the same source only.
        let mut file = with_source(json!({"decision_provider": "custom", "decision_model": "m",
            "decision_base_url": base}));
        normalize(&mut file);
        save(dir.path(), &file).unwrap();
        assert_eq!(
            first(fetch_models(dir.path(), "custom", None, true).unwrap()),
            "m-1"
        );
    }

    #[test]
    fn catalog_urls_follow_the_address_the_service_is_given() {
        assert_eq!(models::ollama_root("http://h:1/v1/systemone"), "http://h:1");
        assert_eq!(models::ollama_root("http://h:1/v1"), "http://h:1");
        assert_eq!(models::ollama_root("http://h:1"), "http://h:1");
        assert_eq!(
            models::custom_models_url("https://a.example"),
            "https://a.example/v1/models"
        );
        assert_eq!(
            models::custom_models_url("https://a.example/v1beta"),
            "https://a.example/v1beta/models"
        );
        assert_eq!(
            models::custom_models_url("https://a.example/api/models"),
            "https://a.example/api/models"
        );
        assert_eq!(
            models::custom_models_url("https://a.example/v1/systemone"),
            "https://a.example/v1/models"
        );
    }
}

use super::*;
use serde_json::json;

#[test]
fn prose_removes_nonspoken_content_and_truncates_at_a_sentence() {
    let input = "# 标题\n**你好**，`行内内容`。\n```rust\nsecret();\n```\n| 名称 | 内容 |\n| --- | --- |\n路径 /tmp/a.rs 链接 https://example.org/x\n- [文档](https://example.org) <b>保留</b>";
    assert_eq!(
        text::clean(input, true),
        "标题\n你好，行内内容。\n路径 链接\n文档 保留"
    );
    assert_eq!(
        text::clean("名称 | 内容\n--- | ---\n甲 | 乙\n\n继续", true),
        "继续"
    );
    assert_eq!(
        text::clean(
            "详情：https://example.org，参见 /tmp/a.md，然后继续。",
            true
        ),
        "详情：，参见 ，然后继续。"
    );
    assert_eq!(text::clean("```\ncode only\n```", false), "");
    assert_eq!(text::clean("~~~js\ncode only\n~~~", true), "");
    assert_eq!(
        text::clean("1. 甲\n2) 乙\n3、丙\n4）丁", true),
        "1、甲\n2、乙\n3、丙\n4、丁"
    );
    assert_eq!(text::clean("3.5 秒就够了", true), "3.5 秒就够了");
    let long = "字".repeat(300);
    assert_eq!(text::clean(&long, true), long);
    assert_eq!(
        text::clean("./src/main.rs ~/file C:\\Users\\a src/agentnave/a.py", true),
        ""
    );
}

#[test]
fn prose_preserves_technical_terms_and_only_removes_links_paths_and_tags() {
    let terms = "CI/CD流水线已经通过，可以合并。I/O开销很小，HTTP/2延迟更低，A/B测试已完成，1/2的耗时已消除。";
    assert_eq!(text::clean(terms, true), terms);
    assert_eq!(
        text::clean(
            "我在https://example.com看到了这个问题，修改src/voice/text.rs并检查apps/tray/ui。",
            true
        ),
        "我在看到了这个问题，修改并检查。"
    );
    assert_eq!(
        text::clean("延迟 < 50ms，吞吐量翻倍。", true),
        "延迟 < 50ms，吞吐量翻倍。"
    );
    assert_eq!(
        text::clean("保留<em>中文</em>与未闭合<tag", true),
        "保留中文与未闭合<tag"
    );
}

#[test]
fn without_a_summary_only_the_opening_paragraphs_are_read() {
    let paragraphs = format!(
        "# 结论\n可以合并。\n{}\n{}",
        "甲".repeat(60),
        "乙".repeat(60)
    );
    assert_eq!(
        text::clean(&paragraphs, false),
        format!("结论\n可以合并。\n{}", "甲".repeat(60))
    );
    let sentences = format!(
        "{}。{}。{}",
        "前".repeat(30),
        "中".repeat(50),
        "后".repeat(50)
    );
    assert_eq!(text::clean(&sentences, false).chars().count(), 82);
    let version = format!("{}升级到 v1.2 后完成。{}", "字".repeat(40), "尾".repeat(80));
    assert_eq!(
        text::clean(&version, false),
        format!("{}升级到 v1.2 后完成。", "字".repeat(40))
    );
    assert_eq!(text::clean(&"字".repeat(300), false).chars().count(), 100);
    assert_eq!(text::clean("短消息。", false), "短消息。");
    assert_eq!(text::cap("短消息。", 10), "短消息。");
    assert_eq!(
        text::cap(&format!("{}。{}", "前".repeat(5), "后".repeat(10)), 10),
        "前前前前前。"
    );
    assert_eq!(text::cap(&"字".repeat(20), 10), "字".repeat(10));
    // A short heading is not all that is read: the paragraph after it follows to a sentence end.
    let headed = format!("结论\n{}。{}", "甲".repeat(60), "乙".repeat(60));
    assert_eq!(
        text::clean(&headed, false),
        format!("结论\n{}。", "甲".repeat(60))
    );
}

#[test]
fn minimax_decoding_and_business_errors_are_not_silenced() {
    assert_eq!(minimax::decode("494433ff").unwrap(), b"ID3\xff");
    for bad in ["a", "xx", "你好"] {
        assert!(minimax::decode(bad).is_err());
    }
    assert!(minimax::check(&json!({"base_resp":{"status_code":0}})).is_ok());
    let error = minimax::check(&json!({"base_resp":{"status_code":1004,"status_msg":"认证失败"}}))
        .unwrap_err();
    assert!(error.contains("1004") && error.contains("认证失败"));
    assert!(minimax::check(&json!({})).is_err());
}

#[test]
fn settings_and_keys_validate_and_never_echo_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let defaults = storage::read_settings(root).unwrap();
    assert_eq!(defaults["enabled"], false);
    assert_eq!(defaults["engine"], "elevenlabs");
    std::fs::write(
        root.join("voice.json"),
        json!({
            "enabled":false,
            "engine":"elevenlabs",
            "rate":"normal",
            "minimax_region":"cn",
            "models":{},
            "voices":{}
        })
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        storage::read_settings(root).unwrap()["summary"],
        json!({"enabled":false,"provider":"magpie","base_url":"","model":"","effort":"","prompt":summary::DEFAULT_PROMPT,"level":"standard","language":"source"})
    );
    let mut with_effort = defaults.clone();
    with_effort["summary"]["effort"] = json!("high");
    assert_eq!(
        storage::validate(&with_effort).unwrap()["summary"]["effort"],
        "high"
    );
    assert!(speak(root, &json!({"family":null,"text":"hello","full":true})).is_err());
    assert!(!root.join("voice-cache").exists());
    for (field, value) in [
        ("engine", json!("unknown")),
        ("minimax_region", json!("other")),
        ("voices", json!({"unknown":"voice"})),
        ("voices", json!({"gpt":"../evil"})),
    ] {
        let mut invalid = defaults.clone();
        invalid[field] = value;
        assert!(storage::save_settings(root, &invalid).is_err());
    }
    storage::save_key(
        root,
        &json!({"engine":"elevenlabs","key":"  fake-only-key  "}),
    )
    .unwrap();
    let keys = storage::read_keys(root).unwrap();
    assert_eq!(keys["elevenlabs"], "fake-only-key");
    let public = storage::public_state(defaults.clone(), &keys);
    assert_eq!(public["keys"]["elevenlabs"], true);
    assert!(!public.to_string().contains("fake-only-key"));
    storage::save_key(root, &json!({"engine":"summary","key":"fake-summary-key"})).unwrap();
    let public = storage::public_state(
        storage::read_settings(root).unwrap(),
        &storage::read_keys(root).unwrap(),
    );
    assert_eq!(public["keys"]["summary"], true);
    assert!(!public.to_string().contains("fake-summary-key"));
    if !crate::PREVIEW {
        let response = get(root, "/app/voice");
        assert_eq!(response.status, 200);
        assert!(
            !String::from_utf8(response.body)
                .unwrap()
                .contains("fake-only-key")
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.join("voice-keys.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    storage::save_key(root, &json!({"engine":"elevenlabs","key":""})).unwrap();
    assert_eq!(
        storage::public_state(defaults.clone(), &storage::read_keys(root).unwrap())["keys"]["elevenlabs"],
        false
    );
    let mut enabled = defaults;
    enabled["enabled"] = json!(true);
    storage::save_settings(root, &enabled).unwrap();
    // Even an explicit voice cannot synthesize without the selected engine's key.
    assert_eq!(
        speak(
            root,
            &json!({"family":null,"text":"大家好。","full":true,"voice":"test-voice"})
        )
        .err()
        .unwrap(),
        "请先保存所选引擎的 API 密钥"
    );
    assert_eq!(
        speak(
            root,
            &json!({"family":null,"text":"```\ncode\n```","full":true})
        )
        .unwrap()
        .status,
        204
    );
    storage::cache(root, "test", b"mp3", "mp3").unwrap();
    assert_eq!(storage::cached(root, "test", "mp3").unwrap(), b"mp3");
}

#[test]
fn combined_tls_features_have_an_explicit_provider() {
    initialize_tls();
    let _ = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    assert!(client().is_ok());
}

#[test]
fn automatic_playback_and_prompt_settings_preserve_old_documents() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = storage::read_settings(directory.path()).unwrap();
    assert_eq!(settings["auto_read"], false);
    assert_eq!(settings["continue_read"], false);
    // Every character starts with its own persona.
    for (family, persona) in storage::DEFAULT_PERSONAS {
        assert_eq!(settings["personas"][family], persona);
    }
    for (field, value) in [
        ("auto_read", json!("yes")),
        ("continue_read", json!(1)),
        ("personas", json!({"unknown":"role"})),
        ("personas", json!({"gpt": "字".repeat(501)})),
        ("personas", json!({"boss":false})),
    ] {
        let mut invalid = settings.clone();
        invalid[field] = value;
        assert!(storage::validate(&invalid).is_err());
    }
    settings["auto_read"] = json!(true);
    settings["continue_read"] = json!(true);
    settings["summary"]["prompt"] = json!("");
    settings["personas"] = json!({"boss":"先讲结论。","gpt":"语气简短。","claude":" "});
    storage::save_settings(directory.path(), &settings).unwrap();
    let saved = storage::read_settings(directory.path()).unwrap();
    assert_eq!(saved["auto_read"], true);
    assert_eq!(saved["continue_read"], true);
    assert_eq!(saved["summary"]["prompt"], summary::DEFAULT_PROMPT);
    assert_eq!(saved["personas"]["boss"], "先讲结论。");
    assert_eq!(saved["personas"]["gpt"], "语气简短。");
    assert_eq!(
        saved["personas"]["claude"],
        storage::DEFAULT_PERSONAS[1].1,
        "a blank persona is reset"
    );
    // A summary platform no longer offered opens as the default instead of failing.
    let mut retired = saved.clone();
    retired["summary"]["provider"] = json!("groq");
    std::fs::write(directory.path().join("voice.json"), retired.to_string()).unwrap();
    let reopened = storage::read_settings(directory.path()).unwrap();
    assert_eq!(reopened["summary"]["provider"], "magpie");
    {
        let custom = "  请保留我的问题和约束。\n";
        settings["summary"]["prompt"] = json!(custom);
        std::fs::write(directory.path().join("voice.json"), settings.to_string()).unwrap();
        assert_eq!(
            storage::read_settings(directory.path()).unwrap()["summary"]["prompt"],
            custom
        );
        storage::save_settings(directory.path(), &settings).unwrap();
        assert_eq!(
            storage::read_settings(directory.path()).unwrap()["summary"]["prompt"],
            custom
        );
    }
    settings["summary"]["prompt"] = json!("字".repeat(4001));
    assert!(storage::validate(&settings).is_err());
}

#[test]
fn speech_options_default_validate_and_enter_the_effective_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let settings = storage::read_settings(directory.path()).unwrap();
    assert_eq!(settings["read_scope"], "all");
    assert_eq!(settings["code"], "skip");
    assert_eq!(settings["cache_limit_mb"], 200);
    assert_eq!(settings["summary"]["level"], "standard");
    assert_eq!(settings["summary"]["language"], "source");
    let mut summary = settings["summary"].clone();
    let mut keys = std::collections::HashSet::new();
    for (level, length) in [
        ("short", "不超过 40 字，一句话"),
        ("standard", "不超过 150 字，三句以内"),
        ("detailed", "不超过 300 字，六句以内"),
    ] {
        summary["level"] = json!(level);
        let language_rules = [
            ("source", "跟随原文的语言输出"),
            ("zh", "只用中文输出"),
            ("en", "Output only English speech"),
        ];
        for (language, rule) in language_rules {
            summary["language"] = json!(language);
            let prompt = summary::fixed_prompt("保留费用分歧。", &summary);
            assert!(prompt.contains(length));
            assert!(prompt.contains(&format!("summary.language={language}")));
            assert!(prompt.contains(rule));
            for (other, other_rule) in language_rules {
                if other != language {
                    assert!(!prompt.contains(other_rule));
                }
            }
            assert!(prompt.contains("保留费用分歧。"));
            assert!(!prompt.contains("80 到 120") && !prompt.contains("最多 300"));
            assert!(keys.insert(storage::hash(&prompt)));
        }
    }
    for (field, invalid) in [
        ("read_scope", json!("none")),
        ("code", json!(true)),
        ("cache_limit_mb", json!(50)),
        ("cache_limit_mb", json!(200.0)),
    ] {
        let mut bad = settings.clone();
        bad[field] = invalid;
        assert!(storage::validate(&bad).is_err());
    }
    for field in ["level", "language"] {
        let mut bad = settings.clone();
        bad["summary"][field] = json!("other");
        assert!(storage::validate(&bad).is_err());
    }
}

#[test]
fn code_mentions_work_with_and_without_summary_and_ignore_language_when_disabled() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = storage::read_settings(directory.path()).unwrap();
    let payload = json!({"text":"已完成。\n```rust\nprivate_code();\n```\n继续看 /tmp/a.rs 和 https://example.org", "full":false});
    settings["code"] = json!("mention");
    settings["summary"]["language"] = json!("en");
    let (spoken, summary) = source_text(&payload, &settings).unwrap();
    assert!(summary.is_none());
    assert!(spoken.contains("这里有一段代码"));
    assert!(
        !spoken.contains("private_code") && !spoken.contains("tmp") && !spoken.contains("https")
    );
    settings["summary"]["enabled"] = json!(true);
    let (_, summary) = source_text(&payload, &settings).unwrap();
    assert!(summary.unwrap().contains("here's a code snippet"));
    settings["code"] = json!("skip");
    assert!(
        !source_text(&payload, &settings)
            .unwrap()
            .1
            .unwrap()
            .contains("snippet")
    );
    assert_eq!(
        text::clean_with_code("~~~js\ncode\n~~~\n```py\nmore\n```", true, "mention", "zh"),
        "这里有一段代码\n这里有一段代码"
    );
    assert_eq!(
        text::clean_with_code("Done.\n```js\ncode\n```", true, "mention", "source"),
        "Done.\nhere's a code snippet"
    );
}

#[test]
fn disabled_cache_neither_reads_nor_writes_and_clear_preserves_settings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut settings = storage::read_settings(root).unwrap();
    settings["cache_limit_mb"] = json!(0);
    storage::save_settings(root, &settings).unwrap();
    for extension in ["txt", "mp3"] {
        storage::cache(root, "disabled", b"unused", extension).unwrap();
        assert!(storage::cached(root, "disabled", extension).is_none());
    }
    assert!(!root.join("voice-cache").exists());
    assert_eq!(storage::cache_bytes(root).unwrap(), 0);
    settings["cache_limit_mb"] = json!(100);
    storage::save_settings(root, &settings).unwrap();
    storage::cache(root, "summary", b"words", "txt").unwrap();
    storage::cache(root, "audio", b"mp3", "mp3").unwrap();
    assert_eq!(storage::cache_bytes(root).unwrap(), 8);
    settings["cache_limit_mb"] = json!(0);
    storage::save_settings(root, &settings).unwrap();
    assert!(storage::cached(root, "audio", "mp3").is_none());
    storage::cache(root, "late-result", b"mp3", "mp3").unwrap();
    assert_eq!(storage::cache_bytes(root).unwrap(), 8);
    storage::clear_cache(root).unwrap();
    storage::clear_cache(root).unwrap();
    assert_eq!(storage::cache_bytes(root).unwrap(), 0);
    assert_eq!(storage::read_settings(root).unwrap(), settings);
    if !crate::PREVIEW {
        let response = get(root, "/app/voice/cache");
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body).unwrap(),
            json!({"bytes":0})
        );
        let response = request(root, "/app/voice/cache/clear", "POST", Some("1"), b"");
        assert_eq!(response.status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body).unwrap(),
            json!({"cleared":true})
        );
        assert_eq!(
            request(root, "/app/voice/cache/clear", "POST", None, b"{}").status,
            403
        );
    }
}

#[test]
fn cache_evicts_to_the_selected_limit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut settings = storage::read_settings(root).unwrap();
    settings["cache_limit_mb"] = json!(100);
    storage::save_settings(root, &settings).unwrap();
    std::fs::create_dir(root.join("voice-cache")).unwrap();
    let file = std::fs::File::create(root.join("voice-cache/old.mp3")).unwrap();
    file.set_len(101 * 1024 * 1024).unwrap();
    // A future-dated old file makes the new entry sort first, even with coarse timestamps.
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(60)),
    )
    .unwrap();
    storage::cache(root, "new", b"new", "txt").unwrap();
    assert!(storage::cache_bytes(root).unwrap() <= 100 * 1024 * 1024);
    assert_eq!(storage::cached(root, "new", "txt").unwrap(), b"new");
}

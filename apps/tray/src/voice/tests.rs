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
    let long = format!(
        "{}。{}！{}",
        "甲".repeat(100),
        "乙".repeat(100),
        "丙".repeat(100)
    );
    assert_eq!(text::clean(&long, false).chars().count(), 101);
    assert_eq!(
        text::clean("1. 甲\n2) 乙\n3、丙\n4）丁", true),
        "1、甲\n2、乙\n3、丙\n4、丁"
    );
    assert_eq!(text::clean("3.5 秒就够了", true), "3.5 秒就够了");
    let points = format!(
        "{}结论。{}\n1. 第一点。{}\n- 细节\n2. **第二点**。{}\n3. 第三点。",
        "前".repeat(10),
        "略".repeat(150),
        "甲".repeat(100),
        "乙".repeat(100)
    );
    assert_eq!(
        text::clean(&points, false),
        format!(
            "{}结论。\n1、第一点。\n2、第二点。\n3、第三点。",
            "前".repeat(10)
        )
    );
    assert_eq!(text::clean(&long, true), long);
    assert_eq!(text::clean(&"字".repeat(300), false).chars().count(), 240);
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
fn summary_uses_sentence_boundaries_without_cutting_versions_or_short_openings() {
    for fragment in [
        "耗时从 3.5 秒降到 1.2 秒",
        "升级到 v1.2 后",
        "修改了 voice.rs 的逻辑",
    ] {
        let long = format!(
            "{}{}{}。尾句。",
            "字".repeat(180),
            fragment,
            "续".repeat(30)
        );
        let sentence_end = long.chars().position(|c| c == '。').unwrap() + 1;
        assert_eq!(text::clean(&long, false).chars().count(), sentence_end);
    }
    let no_nearby_end = format!("我先说结论。{}。", "字".repeat(270));
    assert_eq!(text::clean(&no_nearby_end, false).chars().count(), 240);
    let next_sentence = format!("我先说结论。{}。{}。", "字".repeat(210), "尾".repeat(60));
    assert_eq!(text::clean(&next_sentence, false).chars().count(), 217);
    let preferred = format!(
        "{}。{}。{}",
        "字".repeat(150),
        "字".repeat(60),
        "尾".repeat(60)
    );
    assert_eq!(text::clean(&preferred, false).chars().count(), 151);
    let english = format!("{}End. {}", "字".repeat(180), "tail ".repeat(20));
    assert_eq!(text::clean(&english, false).chars().count(), 184);
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
        json!({"enabled":false,"provider":"magpie","base_url":"","model":"","effort":"","prompt":summary::DEFAULT_PROMPT})
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
        ("rate", json!("turbo")),
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
    assert_eq!(settings["auto_read"], true);
    assert_eq!(settings["personas"], json!({}));
    for (field, value) in [
        ("auto_read", json!("yes")),
        ("personas", json!({"unknown":"role"})),
        ("personas", json!({"gpt": "字".repeat(501)})),
        ("personas", json!({"boss":false})),
    ] {
        let mut invalid = settings.clone();
        invalid[field] = value;
        assert!(storage::validate(&invalid).is_err());
    }
    settings["auto_read"] = json!(false);
    settings["summary"]["prompt"] = json!("");
    settings["personas"] = json!({"boss":"先讲结论。","gpt":"语气简短。"});
    storage::save_settings(directory.path(), &settings).unwrap();
    let saved = storage::read_settings(directory.path()).unwrap();
    assert_eq!(saved["auto_read"], false);
    assert_eq!(saved["summary"]["prompt"], summary::DEFAULT_PROMPT);
    assert_eq!(saved["personas"], settings["personas"]);
    // Simulate an existing document directly: saving it would already normalize the old default.
    for legacy in [
        summary::LEGACY_DEFAULT_PROMPT,
        summary::LEGACY_MESSAGE_PROMPT,
        summary::LEGACY_SPEAKER_PROMPT,
    ] {
        settings["summary"]["prompt"] = json!(legacy);
        std::fs::write(directory.path().join("voice.json"), settings.to_string()).unwrap();
        let migrated = storage::read_settings(directory.path()).unwrap();
        assert_eq!(migrated["summary"]["prompt"], summary::DEFAULT_PROMPT);
        storage::save_settings(directory.path(), &migrated).unwrap();
        let persisted: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("voice.json")).unwrap())
                .unwrap();
        assert_eq!(persisted["summary"]["prompt"], summary::DEFAULT_PROMPT);
    }
    for custom in [
        "  请保留我的问题和约束。\n".to_owned(),
        format!("{} ", summary::LEGACY_DEFAULT_PROMPT),
        format!("{} ", summary::LEGACY_MESSAGE_PROMPT),
    ] {
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

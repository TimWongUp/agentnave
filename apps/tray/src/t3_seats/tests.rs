use super::*;

fn candidate(instance: &str, model: &str) -> Value {
    json!({"instance": instance, "model": model, "options": []})
}

fn seat(id: &str) -> Value {
    json!({
        "id": id,
        "kind": "custom",
        "role": "写代码",
        "description": "写代码",
        "access": "write",
        "difficulty": "medium",
        "candidates": [candidate("codex", "gpt-6.2-astra")]
    })
}

#[test]
fn validation_matches_server_schema_and_save_gate() {
    let valid = json!({
        "version": 1,
        "enabled": true,
        "random_candidates": true,
        "seats": [
            seat("s-1"),
            {
                "id": "s-empty",
                "kind": "explorer",
                "role": "通用探索",
                "description": "读代码",
                "access": "read",
                "difficulty": "easy",
                "candidates": []
            },
            {
                "id": "s-img",
                "kind": "image",
                "role": "画图",
                "description": "画图",
                "access": "write",
                "candidates": [candidate("claudeAgent", "claude-opus-4-6")]
            }
        ],
        "review": {
            "correctness-reviewer": [
                {
                    "instance": "codex",
                    "model": "gpt-6.2-astra",
                    "options": [{"id": "reasoningEffort", "value": "xhigh"}]
                }
            ]
        }
    });
    assert!(validate(&valid).is_ok());
    assert!(validate_for_save(&valid).is_ok());
    assert!(is_active(&valid));

    // Schema allows enabled=true with 0 candidates (inactive on server), while save refuses it.
    let enabled_empty = json!({
        "version": 1,
        "enabled": true,
        "seats": [{
            "id": "s-draft",
            "kind": "custom",
            "role": "草稿",
            "description": "草稿",
            "access": "write",
            "difficulty": "medium",
            "candidates": []
        }],
        "review": {"security-reviewer": []}
    });
    assert!(validate(&enabled_empty).is_ok());
    assert!(!is_active(&enabled_empty));
    let err = validate_for_save(&enabled_empty).unwrap_err();
    assert!(err.contains("至少"), "{err}");

    // Review-only candidates satisfy enabled=true.
    let review_only = json!({
        "version": 1,
        "enabled": true,
        "seats": [],
        "review": {"security-reviewer": [candidate("codex", "gpt-6.2-astra")]}
    });
    assert!(validate_for_save(&review_only).is_ok());
    assert!(is_active(&review_only));

    // Forbidden option IDs (case-insensitive) are refused.
    for forbidden in [
        "mode",
        "sandbox",
        "multitask",
        "permission",
        "permissionMode",
        "approvalPolicy",
        "runtimeMode",
        "fastMode",
        "ultracode",
        "serviceTier",
        "cyberAccessProgram",
        "SANDBOX",
        "PermissionMode",
    ] {
        let mut bad = seat("s-bad");
        bad["candidates"] = json!([{
            "instance": "codex",
            "model": "gpt-6.2-astra",
            "options": [{"id": forbidden, "value": "x"}]
        }]);
        assert!(
            validate(&json!({"version": 1, "seats": [bad]})).is_err(),
            "{forbidden}"
        );
    }

    // Other schema rejections mirroring Python T3SeatsFile.
    let mut five_candidates = seat("s-5");
    five_candidates["candidates"] = json!(
        (0..5)
            .map(|i| candidate(&format!("i{i}"), "m"))
            .collect::<Vec<_>>()
    );
    for bad in [
        json!([]),
        json!({"version": 2}),
        json!({"version": true}),
        json!({"version": 1, "enabled": "true"}),
        json!({"version": 1, "random_candidates": 1}),
        json!({"version": 1, "extra": 1}),
        json!({"version": 1, "seats": [seat("dup"), {
            "id": "dup", "kind": "custom", "role": "其他", "description": "其他",
            "access": "write", "difficulty": "medium", "candidates": []
        }]}),
        json!({"version": 1, "seats": [seat("correctness-reviewer")]}),
        json!({"version": 1, "seats": [five_candidates]}),
        json!({"version": 1, "review": {"unknown": [candidate("a", "b")]}}),
        json!({"version": 1, "review": {"correctness-reviewer": (0..5).map(|i| candidate(&format!("i{i}"), "m")).collect::<Vec<_>>()}}),
        json!({"version": 1, "seats": [{
            "id": "s-1", "kind": "custom", "role": "写", "description": "写",
            "access": "write", "difficulty": "medium",
            "candidates": [{"instance": " codex", "model": "m"}]
        }]}),
        json!({"version": 1, "seats": [{
            "id": "s-1", "kind": "custom", "role": "写", "description": "写",
            "access": "write", "difficulty": "medium",
            "candidates": [{"instance": "codex", "model": "m "}]
        }]}),
        json!({"version": 1, "seats": [{
            "id": "s-1", "kind": "custom", "role": "写", "description": "写",
            "access": "write", "difficulty": "medium",
            "candidates": [{"instance": "codex", "model": "m", "options": [{"id": "effort", "value": "   "}]}]
        }]}),
        json!({"version": 1, "seats": [{
            "id": "s-1", "kind": "custom", "role": "写", "description": "写",
            "access": "write", "difficulty": "medium",
            "candidates": [{"instance": "codex", "model": "m", "options": [
                {"id": "effort", "value": "high"}, {"id": "reasoningEffort", "value": "max"}
            ]}]
        }]}),
        json!({"version": 1, "seats": [{
            "id": "s-1", "kind": "explorer", "role": "探索", "description": "探索",
            "access": "write", "difficulty": "easy", "candidates": []
        }]}),
    ] {
        assert!(validate(&bad).is_err(), "{bad}");
    }
}

#[test]
fn normalize_save_and_read_round_trip_and_report_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read(dir.path()).unwrap().is_none());

    let mut doc = json!({
        "version": 1,
        "enabled": true,
        "random_candidates": false,
        "seats": [{
            "id": "s-1",
            "kind": "custom",
            "role": "\u{3000} 写代码\u{a0}",
            "description": "  负责实现  ",
            "access": "write",
            "difficulty": "medium",
            "candidates": [candidate("codex", "gpt-6.2-astra")]
        }],
        "review": {}
    });
    normalize(&mut doc);
    assert_eq!(doc["seats"][0]["role"], "写代码");
    assert_eq!(doc["seats"][0]["description"], "负责实现");
    save(dir.path(), &doc).unwrap();
    assert_eq!(read(dir.path()).unwrap().unwrap(), doc);

    // Corrupt file returns Err without overwriting disk.
    fs::write(dir.path().join(FILE), "{broken").unwrap();
    assert!(read(dir.path()).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join(FILE)).unwrap(),
        "{broken"
    );
}

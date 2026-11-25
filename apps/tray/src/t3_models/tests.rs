use super::*;

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value.to_string()).unwrap();
}

#[test]
fn missing_or_empty_home_reports_not_detected_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        read_from(&dir.path().join("absent")),
        json!({"detected": false, "instances": []})
    );
    assert_eq!(
        read_from(dir.path()),
        json!({"detected": false, "instances": []})
    );
}

#[test]
fn whitelist_strips_private_auth_fields_and_filters_instances_models_and_options() {
    let dir = tempfile::tempdir().unwrap();
    let t3 = dir.path();
    let caches = t3.join("caches");

    // Non-instance file (missing models / instanceId / driver) is skipped.
    write_json(
        &caches.join("acp-auth-deadbeef.json"),
        &json!({"authenticated": true, "binding": "x", "email": "leak-auth@example.com"}),
    );
    // Disabled instance is skipped.
    write_json(
        &caches.join("cursor.json"),
        &json!({
            "instanceId": "cursor",
            "driver": "cursor",
            "displayName": "Cursor",
            "enabled": false,
            "status": "disabled",
            "checkedAt": "2026-11-24T10:00:00Z",
            "models": [{"slug": "m", "name": "M"}]
        }),
    );
    // Broken JSON & oversized file are skipped safely.
    fs::write(caches.join("corrupt.json"), "{broken").unwrap();
    fs::write(
        caches.join("oversized.json"),
        vec![b' '; (MAX_FILE_BYTES + 16) as usize],
    )
    .unwrap();

    // Active Claude instance with private auth fields, reasoning effort, and non-reasoning options.
    write_json(
        &caches.join("claudeAgent.json"),
        &json!({
            "instanceId": "claudeAgent",
            "driver": "claudeAgent",
            "displayName": "Claude Code",
            "enabled": true,
            "installed": true,
            "status": "ready",
            "auth": {
                "status": "authenticated",
                "email": "private-user@example.com",
                "workspaceId": "ws-secret-42",
                "accessToken": "tok-never-leak"
            },
            "checkedAt": "2026-11-24T10:00:00Z",
            "models": [
                {
                    "slug": "claude-opus-4-6",
                    "name": "Claude Opus 4.6",
                    "isDefault": true,
                    "isLegacy": false,
                    "capabilities": {
                        "optionDescriptors": [
                            {
                                "id": "effort",
                                "label": "Reasoning",
                                "type": "select",
                                "options": [
                                    {"id": "low", "label": "Low"},
                                    {"id": "high", "label": "High"}
                                ]
                            },
                            {"id": "fastMode", "label": "Fast Mode", "type": "boolean"},
                            {"id": "mode", "label": "Permission", "type": "select", "role": "reasoning", "options": [{"id": "bypass", "label": "Bypass"}]}
                        ]
                    }
                },
                {
                    "slug": "claude-old",
                    "name": "Claude Old",
                    "isLegacy": true
                }
            ]
        }),
    );

    // Antigravity instance using role == "reasoning" (`thought_level`) + hidden model in settings.
    write_json(
        &caches.join("antigravity.json"),
        &json!({
            "instanceId": "antigravity",
            "driver": "antigravity",
            "displayName": "Antigravity",
            "enabled": true,
            "installed": true,
            "status": "ready",
            "auth": {"status": "authenticated", "email": "agy-secret@example.com"},
            "checkedAt": "2026-11-24T09:30:00Z",
            "models": [
                {
                    "slug": "gemini-4-neon",
                    "name": "Gemini 4 Neon",
                    "isDefault": true,
                    "capabilities": {
                        "optionDescriptors": [
                            {
                                "id": "thought_level",
                                "label": "Thinking Level",
                                "type": "select",
                                "role": "reasoning",
                                "options": [
                                    {"id": "low", "label": "Low"},
                                    {"id": "xhigh", "label": "Extra"}
                                ]
                            }
                        ]
                    }
                },
                {
                    "slug": "gemini-hidden",
                    "name": "Hidden Model"
                }
            ]
        }),
    );

    // Unauthenticated instance -> available: false.
    write_json(
        &caches.join("grok.json"),
        &json!({
            "instanceId": "grok",
            "driver": "grok",
            "displayName": "Grok",
            "enabled": true,
            "installed": true,
            "status": "ready",
            "auth": {"status": "unauthenticated", "email": "unauth-secret@example.com"},
            "checkedAt": "2026-11-24T09:00:00Z",
            "models": [{"slug": "grok-4", "name": "Grok 4"}]
        }),
    );

    // Stale instance (> 24h older than newest checkedAt).
    write_json(
        &caches.join("codex_old.json"),
        &json!({
            "instanceId": "codex_old",
            "driver": "codex",
            "displayName": "Old Codex",
            "enabled": true,
            "installed": true,
            "status": "ready",
            "auth": {"status": "authenticated"},
            "checkedAt": "2026-11-23T09:59:59Z",
            "models": [{"slug": "gpt-old", "name": "GPT Old"}]
        }),
    );

    // Settings with hiddenModels.
    write_json(
        &t3.join("userdata/settings.json"),
        &json!({
            "providerModelPreferences": {
                "antigravity": {"hiddenModels": ["gemini-hidden"], "modelOrder": ["gemini-4-neon"]}
            },
            "unrelatedSecret": "must-not-appear"
        }),
    );

    let out = read_from(t3);
    let serialized = out.to_string();
    // Privacy invariant: no email, workspaceId, token, or unrelated settings leak into output.
    for secret in [
        "private-user@example.com",
        "agy-secret@example.com",
        "unauth-secret@example.com",
        "leak-auth@example.com",
        "ws-secret-42",
        "tok-never-leak",
        "must-not-appear",
        "workspaceId",
        "email",
    ] {
        assert!(
            !serialized.contains(secret),
            "leaked {secret}: {serialized}"
        );
    }

    assert_eq!(out["detected"], true);
    let list = out["instances"].as_array().unwrap();
    assert_eq!(list.len(), 4);
    let find = |id: &str| list.iter().find(|item| item["instance"] == id).unwrap();

    let claude = find("claudeAgent");
    assert_eq!(claude["available"], true);
    assert_eq!(claude["stale"], false);
    assert_eq!(claude["name"], "Claude Code");
    let opus = &claude["models"][0];
    assert_eq!(opus["slug"], "claude-opus-4-6");
    assert_eq!(opus["default"], true);
    assert_eq!(opus["legacy"], false);
    assert_eq!(opus["hidden"], false);
    assert_eq!(
        opus["reasoning"],
        json!({
            "id": "effort",
            "label": "Reasoning",
            "options": [
                {"value": "low", "label": "Low"},
                {"value": "high", "label": "High"}
            ]
        })
    );
    assert_eq!(claude["models"][1]["legacy"], true);

    let agy = find("antigravity");
    assert_eq!(agy["available"], true);
    assert_eq!(agy["stale"], false);
    assert_eq!(agy["models"][0]["reasoning"]["id"], "thought_level");
    assert_eq!(agy["models"][1]["hidden"], true);

    assert_eq!(find("grok")["available"], false);
    assert_eq!(find("grok")["stale"], false);

    assert_eq!(find("codex_old")["available"], true);
    assert_eq!(find("codex_old")["stale"], true);
}

#[test]
fn resolve_home_prefers_non_empty_env_and_falls_back_to_user_home() {
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("custom-t3");
    let env = BTreeMap::from([("T3CODE_HOME".into(), custom.display().to_string())]);
    assert_eq!(resolve_home(Some(&env), Some(dir.path())), Some(custom));
    let blank = BTreeMap::from([("T3CODE_HOME".into(), "   ".into())]);
    assert_eq!(
        resolve_home(Some(&blank), Some(dir.path())),
        Some(dir.path().join(".t3"))
    );
}

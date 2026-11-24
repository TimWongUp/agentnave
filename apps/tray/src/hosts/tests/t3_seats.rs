use super::*;

fn setup_t3_reader(with_claude_account: bool) -> (Reader, TempDir, TempDir, PathBuf, PathBuf) {
    let (temp, mut hosts) = fixture();
    // Point T3CODE_HOME at an isolated directory inside temp; never read real ~/.t3.
    let t3_home = hosts.context.home.join("t3-home");
    hosts
        .context
        .env
        .insert("T3CODE_HOME".into(), t3_home.display().to_string());
    // Ensure service version is not considered behind so rules write immediately.
    let site = hosts
        .context
        .home
        .join(".local/share/uv/tools/agentnave/lib/python3.12/site-packages");
    fs::remove_dir_all(site.join("agentnave-0.22.0.dist-info")).unwrap();
    save(
        &site.join(format!("agentnave-{VERSION}.dist-info/entry_points.txt")),
        "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n",
    );
    for id in ["claude", "codex", "grok", "zcode"] {
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
    if with_claude_account {
        let account =
            crate::accounts::create(&hosts.context.data, &hosts.context.home, "工作").unwrap();
        assert_eq!(
            hosts
                .action("connect", &json!({"id": format!("claude@{}", account.id)}))
                .unwrap()["ok"],
            true
        );
    }
    let home = hosts.context.home.clone();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let reader = Reader::new(root.clone());
    reader.use_hosts(hosts);
    (reader, temp, directory, root, home)
}

fn post(reader: &Reader, path: &str, body: &Value) -> crate::response::Response {
    let bytes = serde_json::to_vec(body).unwrap();
    reader.request("POST", path, Some("1"), &bytes)
}

fn valid_doc(enabled: bool) -> Value {
    json!({
        "version": 1,
        "enabled": enabled,
        "random_candidates": false,
        "seats": [{
            "id": "t3-1",
            "kind": "explorer",
            "role": "通用探索",
            "description": "读代码",
            "access": "read",
            "difficulty": "medium",
            "candidates": [{"instance": "codex", "model": "gpt-6.2-astra", "options": []}]
        }],
        "review": {}
    })
}

#[test]
fn t3_seats_route_validates_reads_t3_cache_and_syncs_rules_on_save_disable_and_delete() {
    let (reader, _temp, _dir, root, home) = setup_t3_reader(true);
    let t3_home = home.join("t3-home");
    save(
        &t3_home.join("caches/codex.json"),
        &json!({
            "instanceId": "codex",
            "driver": "codex",
            "displayName": "Codex",
            "enabled": true,
            "installed": true,
            "status": "ready",
            "auth": {"status": "authenticated", "email": "never-return@example.com"},
            "checkedAt": "2026-11-24T10:00:00Z",
            "models": [{"slug": "gpt-6.2-astra", "name": "GPT 6.2 Astra", "isDefault": true}]
        })
        .to_string(),
    );

    // GET returns document=null, error=null, detected T3 list without email.
    let get = reader.request("GET", "/app/t3-seats", None, b"");
    assert_eq!(get.status, 200);
    let body_text = String::from_utf8(get.body).unwrap();
    assert!(!body_text.contains("never-return@example.com"));
    let state: Value = serde_json::from_str(&body_text).unwrap();
    assert_eq!(state["document"], Value::Null);
    assert_eq!(state["error"], Value::Null);
    assert_eq!(state["t3"]["detected"], true);
    assert_eq!(state["t3"]["instances"][0]["instance"], "codex");
    assert!(state.get("rule").is_none());

    // Guardrails: missing action header, wrong method, non-JSON, enabled with 0 candidates.
    assert_eq!(
        reader.request("POST", "/app/t3-seats", None, b"{}").status,
        403
    );
    assert_eq!(
        reader
            .request("DELETE", "/app/t3-seats", Some("1"), b"")
            .status,
        405
    );
    assert_eq!(
        reader
            .request("POST", "/app/t3-seats", Some("1"), b"not json")
            .status,
        400
    );
    assert_eq!(
        post(
            &reader,
            "/app/t3-seats",
            &json!({"version": 1, "enabled": true, "seats": [], "review": {}})
        )
        .status,
        400
    );
    assert!(!root.join("t3-seats.json").exists());

    // Preserve existing user content in CLAUDE.md.
    // While orchestration preset is off, enabling T3 writes no block anywhere.
    save(&home.join(".claude/CLAUDE.md"), "# 我的规则\n");
    let res_off = post(&reader, "/app/t3-seats", &valid_doc(true));
    assert_eq!(
        res_off.status,
        200,
        "{}",
        String::from_utf8_lossy(&res_off.body)
    );
    assert_eq!(
        fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap(),
        "# 我的规则\n"
    );
    assert!(!home.join(".codex/AGENTS.md").exists());

    // Switching orchestration to split activates T3 suffix on Claude (default + named) and Codex only.
    let split_res = post(
        &reader,
        "/app/orchestration",
        &json!({"version": 1, "preset": "split"}),
    );
    assert_eq!(
        split_res.status,
        200,
        "{}",
        String::from_utf8_lossy(&split_res.body)
    );
    let saved_res: Value = serde_json::from_slice(&split_res.body).unwrap();
    assert_eq!(saved_res["saved"], true);
    assert_eq!(saved_res["failures"], json!([]));

    let t3_heading = "### 在 T3 Code 里这样派，不用上面的派法和工位";
    let claude_md = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(claude_md.contains("# 我的规则"));
    assert!(claude_md.contains("### 不在 T3 Code 里时这样派"));
    assert!(claude_md.contains(t3_heading));
    assert_eq!(claude_md.matches(t3_heading).count(), 1);
    let codex_md = fs::read_to_string(home.join(".codex/AGENTS.md")).unwrap();
    assert!(codex_md.contains(t3_heading));
    let named_claude = home.join(".agentnave-accounts/claude/a1/CLAUDE.md");
    assert!(
        fs::read_to_string(&named_claude)
            .unwrap()
            .contains(t3_heading)
    );
    assert!(
        !fs::read_to_string(home.join(".zcode/AGENTS.md"))
            .unwrap()
            .contains(t3_heading)
    );
    assert!(!home.join(".grok/AGENTS.md").exists());

    // Rule files list reports current=true for all target files without false mismatch.
    for row in saved_res["files"].as_array().unwrap() {
        if row["target"] == true {
            assert_eq!(row["current"], true, "{row}");
        }
    }

    // Disabling switch removes T3 sections while keeping the split orchestration block.
    let off = post(&reader, "/app/t3-seats", &valid_doc(false));
    assert_eq!(off.status, 200);
    let claude_off = fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap();
    assert!(
        !claude_off.contains(t3_heading) && !claude_off.contains("### 不在 T3 Code 里时这样派")
    );
    assert!(
        !fs::read_to_string(home.join(".codex/AGENTS.md"))
            .unwrap()
            .contains(t3_heading)
    );
    assert!(
        !fs::read_to_string(&named_claude)
            .unwrap()
            .contains(t3_heading)
    );

    // Turn back on, then delete t3-seats.json and refresh -> removes T3 sections cleanly too.
    assert_eq!(post(&reader, "/app/t3-seats", &valid_doc(true)).status, 200);
    assert!(
        fs::read_to_string(home.join(".codex/AGENTS.md"))
            .unwrap()
            .contains(t3_heading)
    );
    fs::remove_file(root.join("t3-seats.json")).unwrap();
    assert_eq!(
        reader
            .request("POST", "/app/hosts/refresh", Some("1"), b"")
            .status,
        200
    );
    assert!(
        !fs::read_to_string(home.join(".claude/CLAUDE.md"))
            .unwrap()
            .contains(t3_heading)
    );
    assert!(
        !fs::read_to_string(home.join(".codex/AGENTS.md"))
            .unwrap()
            .contains(t3_heading)
    );
}

#[test]
fn broken_t3_seats_file_leaves_rules_alone_and_uninstall_still_cleans_up() {
    let (reader, _temp, _dir, root, home) = setup_t3_reader(false);
    assert_eq!(
        post(
            &reader,
            "/app/orchestration",
            &json!({"version": 1, "preset": "split"}),
        )
        .status,
        200
    );
    assert_eq!(post(&reader, "/app/t3-seats", &valid_doc(true)).status, 200);
    let t3_heading = "### 在 T3 Code 里这样派，不用上面的派法和工位";
    let before = fs::read_to_string(home.join(".codex/AGENTS.md")).unwrap();
    assert!(before.contains(t3_heading));

    // Corrupt t3-seats.json: GET reports error, refresh does NOT modify/delete rules.
    fs::write(root.join("t3-seats.json"), "{broken").unwrap();
    let get = reader.request("GET", "/app/t3-seats", None, b"");
    assert_eq!(get.status, 200);
    let state: Value = serde_json::from_slice(&get.body).unwrap();
    assert!(state["error"].as_str().is_some());
    assert_eq!(
        reader
            .request("POST", "/app/hosts/refresh", Some("1"), b"")
            .status,
        200
    );
    assert_eq!(
        fs::read_to_string(home.join(".codex/AGENTS.md")).unwrap(),
        before
    );

    // Uninstall always removes managed blocks even when t3-seats.json is broken.
    let (_temp2, mut uninstall_hosts) = fixture();
    let codex_bin = uninstall_hosts.context.search[0].join("codex");
    executable_file(&codex_bin, "#!/bin/sh\nexit 99\n");
    let site = uninstall_hosts
        .context
        .home
        .join(".local/share/uv/tools/agentnave/lib/python3.12/site-packages");
    fs::remove_dir_all(site.join("agentnave-0.22.0.dist-info")).unwrap();
    save(
        &site.join(format!("agentnave-{VERSION}.dist-info/entry_points.txt")),
        "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n",
    );
    uninstall_hosts
        .action("connect", &json!({"id": "codex"}))
        .unwrap();
    let data = uninstall_hosts.context.data.clone();
    crate::members::save_orchestration(&data, &json!({"version": 1, "preset": "split"})).unwrap();
    crate::t3_seats::save(&data, &valid_doc(true)).unwrap();
    assert!(sync_orchestration(&data, &uninstall_hosts, &[]).is_empty());
    let codex_rules = uninstall_hosts.context.home.join(".codex/AGENTS.md");
    assert!(
        fs::read_to_string(&codex_rules)
            .unwrap()
            .contains(t3_heading)
    );
    fs::write(data.join("t3-seats.json"), "{broken").unwrap();
    let failures = uninstall_hosts.uninstall();
    assert!(failures.is_empty(), "{failures:?}");
    assert!(!codex_rules.exists());
}

#[test]
fn removing_a_host_removes_its_t3_rules_block() {
    let (reader, _temp, _dir, _root, home) = setup_t3_reader(false);
    assert_eq!(
        post(
            &reader,
            "/app/orchestration",
            &json!({"version": 1, "preset": "split"}),
        )
        .status,
        200
    );
    assert_eq!(post(&reader, "/app/t3-seats", &valid_doc(true)).status, 200);
    assert!(home.join(".codex/AGENTS.md").exists());
    let removed = post(&reader, "/app/hosts/remove", &json!({"id": "codex"}));
    assert_eq!(removed.status, 200);
    assert!(!home.join(".codex/AGENTS.md").exists());
    assert!(home.join(".claude/CLAUDE.md").exists());
}

use super::*;

#[test]
fn uninstall_removes_undetected_hosts_and_does_not_repair_them_again() {
    let (_temp, mut hosts) = fixture();
    assert!(!hosts.detect(&host(&hosts, "zcode")).0);
    let mut configs = Vec::new();
    for target in hosts.registry() {
        let document = Document::parse(&seed(&target), target.format, target.key).unwrap();
        let entry = json!({"command": "agentnave-mcp", "args": []});
        let raw = document.render(Some(&entry)).unwrap();
        save(&target.config, &raw);
        configs.push((target.config, target.format, target.key));
    }
    save(
        &hosts.context.home.join(".gemini/config/hooks.json"),
        r#"{"agentnave-interactive":{},"keep":true}"#,
    );
    // A Skill the user wrote at an App Skill path must survive: uninstall deletes the backups.
    let own = host(&hosts, "codex").optional("agentnave-review").unwrap();
    save(&own.join("SKILL.md"), "mine");
    let failures = hosts.uninstall();
    assert!(failures.is_empty(), "{failures:?}");
    for (path, format, key) in &configs {
        let raw = fs::read_to_string(path).unwrap();
        assert!(
            Document::parse(&raw, *format, key).unwrap().entry.is_none(),
            "{}",
            path.display()
        );
    }
    hosts.refresh_probed();
    for (path, format, key) in configs {
        let raw = fs::read_to_string(path).unwrap();
        assert!(Document::parse(&raw, format, key).unwrap().entry.is_none());
    }
    let hook: Value = serde_json::from_slice(
        &fs::read(hosts.context.home.join(".gemini/config/hooks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(hook, json!({"keep": true}));
    assert_eq!(fs::read_to_string(own.join("SKILL.md")).unwrap(), "mine");
}

#[test]
fn retired_devin_integration_is_removed_once_and_only_what_agentnave_wrote() {
    let (_temp, mut hosts) = fixture();
    let devin = hosts.retired_devin();
    let launcher = launcher_in(&hosts.context.home.join(".local/share/uv/tools"));
    let ours = json!({"command": launcher, "args": []});
    let config = r#"{"keep": true, "mcpServers": {"other": {"command": "keep"}}}"#;
    let document = Document::parse(config, Format::Json, "mcpServers").unwrap();
    save(&devin.config, &document.render(Some(&ours)).unwrap());
    hosts.ensure_skill(&devin, true).unwrap();
    let rules = devin.root.path.join("AGENTS.md");
    save(
        &rules,
        "保留\n<!-- AGENTNAVE:START -->\n旧规则\n<!-- AGENTNAVE:END -->\n",
    );
    // A Skill the user wrote or edited stays.
    let own = devin.optional("agentnave-review").unwrap();
    save(&own.join("SKILL.md"), "mine");

    // An unconfirmed root may hide the real folder: nothing happens and nothing is recorded.
    hosts.shell = Some(Err("unknown".into()));
    assert!(hosts.remove_retired().is_empty());
    assert!(devin.read().unwrap().entry.is_some());
    assert!(hosts.settings["devin"].get("removed").is_none());
    hosts.shell = Some(Ok(BTreeMap::new()));
    hosts.settings = json!({"devin": {"first_seen": "then"}});

    let failures = hosts.remove_retired();
    assert!(failures.is_empty(), "{failures:?}");
    // The record adds to what the file already held for the row.
    assert_eq!(
        hosts.settings["devin"],
        json!({"first_seen": "then", "removed": true})
    );
    let left = devin.read().unwrap();
    assert!(left.entry.is_none());
    assert_eq!(
        left.rest["mcpServers"],
        json!({"other": {"command": "keep"}})
    );
    assert!(!devin.skill.as_ref().unwrap().exists());
    let text = fs::read_to_string(&rules).unwrap();
    assert!(text.contains("保留") && !text.contains("AGENTNAVE"));
    assert_eq!(fs::read_to_string(own.join("SKILL.md")).unwrap(), "mine");

    // Once: an entry added afterwards by hand is not touched at the next start-up.
    save(&devin.config, &document.render(Some(&ours)).unwrap());
    assert!(hosts.remove_retired().is_empty());
    assert!(devin.read().unwrap().entry.is_some());
    // Uninstalling covers it regardless.
    assert!(hosts.uninstall().is_empty());
    assert!(devin.read().unwrap().entry.is_none());
}

#[test]
fn retired_devin_cleanup_keeps_a_foreign_entry_and_names_blocked_rules_files() {
    let (_temp, mut hosts) = fixture();
    let devin = hosts.retired_devin();
    let foreign = json!({"command": "/opt/tools/other-server", "args": []});
    let document = Document::parse("{}", Format::Json, "mcpServers").unwrap();
    save(&devin.config, &document.render(Some(&foreign)).unwrap());
    // A lone marker is refused rather than guessed at.
    let rules = devin.root.path.join("AGENTS.md");
    save(&rules, "<!-- AGENTNAVE:START -->\n保留\n");

    let failures = hosts.remove_retired();
    assert_eq!(devin.read().unwrap().entry, Some(foreign));
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0]["host"], json!(rules));
    assert!(failures[0]["error"].is_string());
    // Not recorded as done, so the next start-up tries again.
    assert!(hosts.settings["devin"].get("removed").is_none());
}

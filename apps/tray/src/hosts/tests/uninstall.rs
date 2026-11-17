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

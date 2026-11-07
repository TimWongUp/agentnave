use super::*;

#[test]
fn refresh_raises_existing_tool_timeouts_and_keeps_other_configurations() {
    for (id, key, minimum) in [
        ("codex", "tool_timeout_sec", 720),
        ("claude", "timeout", 720_000),
        ("antigravity", "timeoutSeconds", 720),
        ("pi", "timeout", 720),
        ("hermes", "timeout", 720),
        ("deepseek", "toolCallTimeoutMs", 720_000),
        ("zcode", "timeoutMs", 720_000),
    ] {
        let (_temp, mut hosts) = fixture();
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        save(&h.config, &seed(&h));
        let before = h.read().unwrap();
        let mut entry = json!({"command":"agentnave-mcp", "env":{"KEEP":"yes"}, "opaque":true});
        entry[key] = json!(60);
        save(&h.config, &before.render(Some(&entry)).unwrap());
        hosts.refresh();
        let updated = h.read().unwrap();
        let entry = updated.entry.as_ref().unwrap();
        assert_eq!(entry[key], minimum, "{id}: {entry}");
        assert_eq!(entry["env"]["KEEP"], "yes");
        assert_eq!(entry["opaque"], true);
        assert_eq!(updated.rest, before.rest, "{id}");
        let bytes = fs::read(&h.config).unwrap();
        hosts.refresh();
        assert_eq!(fs::read(&h.config).unwrap(), bytes, "{id}");
        // Reconnecting must not shorten a value increased by the user.
        let mut longer = entry.clone();
        longer[key] = json!(minimum * 2);
        save(&h.config, &updated.render(Some(&longer)).unwrap());
        let result = hosts.action("connect", &json!({"id":id})).unwrap();
        assert_eq!(result["ok"], true, "{id}: {result}");
        assert_eq!(h.read().unwrap().entry.unwrap()[key], minimum * 2, "{id}");
    }
}

#[test]
fn native_long_defaults_unlimited_and_grok_tool_overrides_are_preserved() {
    let (_temp, hosts) = fixture();
    for id in ["grok", "codebuddy", "workbuddy"] {
        let mut entry = json!({"command":"agentnave-mcp","env":{"KEEP":"yes"}});
        let before = entry.clone();
        super::super::timeouts::ensure(&mut entry, &host(&hosts, id)).unwrap();
        assert_eq!(entry, before, "{id}");
    }
    let mut unlimited = json!({"timeoutSeconds":-1});
    super::super::timeouts::ensure(&mut unlimited, &host(&hosts, "antigravity")).unwrap();
    assert_eq!(unlimited["timeoutSeconds"], -1);
    let mut grok =
        json!({"tool_timeout_sec":1800,"tool_timeouts":{"wait":60,"start_agent":900,"other":30}});
    super::super::timeouts::ensure(&mut grok, &host(&hosts, "grok")).unwrap();
    assert_eq!(
        grok,
        json!({"tool_timeout_sec":1800,"tool_timeouts":{"wait":720,"start_agent":900,"other":30}})
    );
}

#[test]
fn invalid_timeout_is_reported_without_rewriting_the_file() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    executable_file(
        &hosts.context.search[0].join(h.binary),
        "#!/bin/sh\nexit 99\n",
    );
    let text = r#"{"mcpServers":{"agentnave":{"command":"agentnave-mcp","timeout":"custom"}}}"#;
    save(&h.config, text);
    hosts.refresh();
    assert_eq!(fs::read_to_string(&h.config).unwrap(), text);
    let result = hosts.action("connect", &json!({"id":"pi"})).unwrap();
    assert_eq!(result["ok"], false);
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("timeout 必须为数字")
    );
    assert_eq!(fs::read_to_string(&h.config).unwrap(), text);
}

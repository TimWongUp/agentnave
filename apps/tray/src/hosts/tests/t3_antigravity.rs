use super::*;

fn make_t3_profile(t3_home: &Path, name: &str) -> PathBuf {
    let profile = t3_home.join("userdata/providers/antigravity").join(name);
    fs::create_dir_all(profile.join("antigravity-acp")).unwrap();
    fs::create_dir_all(profile.join("config")).unwrap();
    profile
}

#[test]
fn discovers_t3_antigravity_profile_and_connects_with_shared_skills_and_rules() {
    let (_temp, mut hosts) = fixture();
    let home = hosts.context.home.clone();
    // Align the fixture service version with this build so `sync_rules` writes new blocks.
    let old_dist = home.join(
        ".local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave-0.22.0.dist-info",
    );
    let _ = fs::remove_dir_all(&old_dist);
    save(
        &home.join(format!(
            ".local/share/uv/tools/agentnave/lib/python3.12/site-packages/agentnave-{VERSION}.dist-info/entry_points.txt"
        )),
        "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n",
    );
    let t3_home = home.join(".t3");
    let hash = "ac0a3dfd6dddb20962cecff6ee5fe65e19d3923be20e52c5ab52ff877f7e4c32";
    let profile = make_t3_profile(&t3_home, hash);

    // Mirror T3 Code's symlinks for skills and global rules into `~/.gemini`.
    let real_skills = home.join(".gemini/config/skills");
    fs::create_dir_all(&real_skills).unwrap();
    save(&home.join(".gemini/AGENTS.md"), "# User rules\n");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real_skills, profile.join("config/skills")).unwrap();
        std::os::unix::fs::symlink(home.join(".gemini/AGENTS.md"), profile.join("AGENTS.md"))
            .unwrap();
    }

    hosts.t3_antigravity_homes = hosts.other_t3_antigravity_homes();
    let id = "antigravity@t3-ac0a3dfd";
    let row = host(&hosts, id);
    assert_eq!(row.boss, "antigravity");
    assert_eq!(row.label, "Antigravity（T3 Code）");
    assert_eq!(row.config, profile.join("config/mcp_config.json"));
    assert_eq!(row.skill, Some(real_skills.join("agentnave-manager")));

    // Installed without PATH `agy` or `Antigravity.app`, with T3-specific guidance.
    let state = hosts.state(&row);
    assert_eq!(state["installed"], true);
    assert_eq!(state["detect_source"], "t3");
    assert_eq!(state["status"], "available");
    assert_eq!(state["clients"].as_array().unwrap().len(), 0);
    assert_eq!(state["reload_hint"], "在 T3 Code 新建线程或重启 T3 Code");
    assert_eq!(state["reopen"]["app"], "在 T3 Code 新建线程或重启 T3 Code");

    // Connecting writes the isolated profile's mcp_config.json and installs into the shared skill dir.
    let result = hosts.action("connect", &json!({"id": id})).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["host"]["status"], "connected");
    assert_eq!(result["host"]["has_host_env"], true);
    let entry = row.read().unwrap().entry.unwrap();
    assert_eq!(entry["env"]["AGENTNAVE_HOST"], "antigravity");
    assert_eq!(entry["env"]["AGENTNAVE_HOST_APP"], id);
    assert_eq!(entry["timeoutSeconds"], 360);
    assert!(real_skills.join("agentnave-manager/SKILL.md").is_file());
    #[cfg(unix)]
    assert!(profile.join("config/skills").is_symlink());
    // The default `~/.gemini/config/mcp_config.json` was untouched.
    assert!(host(&hosts, "antigravity").read().unwrap().entry.is_none());

    // Global rules write to `~/.gemini/AGENTS.md` without blocking on T3's symlink.
    let doc = json!({"version": 1, "preset": "split"});
    let failures = hosts.sync_rules(&doc, None, &[]);
    assert!(
        failures.is_empty(),
        "unexpected rule failures: {failures:?}"
    );
    let rules = fs::read_to_string(home.join(".gemini/AGENTS.md")).unwrap();
    assert!(rules.contains("<!-- AGENTNAVE:START -->"));
    let effective = hosts.effective_apps(&doc);
    assert_eq!(effective[id], "split");
    #[cfg(unix)]
    assert!(profile.join("AGENTS.md").is_symlink());

    // boss_homes exposes T3's `antigravity-acp` separately from resumable CLI roots.
    let homes = hosts.boss_homes();
    assert!(
        homes
            .other
            .iter()
            .any(|(h, p)| h == "antigravity-acp" && p == &profile.join("antigravity-acp"))
    );
    assert!(
        !homes
            .other
            .iter()
            .any(|(h, p)| h == "antigravity" && p == &profile.join("antigravity-acp"))
    );
}

#[test]
fn removing_t3_antigravity_keeps_main_antigravity_hook_and_shared_skill() {
    let (_temp, mut hosts) = fixture();
    let home = hosts.context.home.clone();
    executable_file(&hosts.context.search[0].join("agy"), "#!/bin/sh\nexit 0\n");
    let profile = make_t3_profile(&home.join(".t3"), "deadbeef12345678");
    hosts.t3_antigravity_homes = hosts.other_t3_antigravity_homes();

    let t3_id = "antigravity@t3-deadbeef";
    assert_eq!(
        hosts
            .action("connect", &json!({"id": "antigravity"}))
            .unwrap()["ok"],
        true
    );
    assert_eq!(
        hosts.action("connect", &json!({"id": t3_id})).unwrap()["ok"],
        true
    );
    let hook = home.join(".gemini/config/hooks.json");
    save(
        &hook,
        &json!({"hooks": {"Stop": [{"command": "agentnave-agy-hook"}]}}).to_string(),
    );

    let removed = hosts.action("remove", &json!({"id": t3_id})).unwrap();
    assert_eq!(removed["ok"], true, "{removed}");
    assert!(host(&hosts, t3_id).read().unwrap().entry.is_none());
    // Main Antigravity entry, shared Skill, and global hook all remain intact.
    assert!(host(&hosts, "antigravity").read().unwrap().entry.is_some());
    assert!(
        home.join(".gemini/config/skills/agentnave-manager/SKILL.md")
            .is_file()
    );
    assert!(
        fs::read_to_string(&hook)
            .unwrap()
            .contains("agentnave-agy-hook")
    );
    let _ = profile;
}

#[test]
fn multiple_t3_antigravity_profiles_and_t3code_home_override() {
    let (_temp, mut hosts) = fixture();
    let custom_t3 = hosts.context.home.join("custom-t3");
    make_t3_profile(&custom_t3, "11111111aaaa");
    make_t3_profile(&custom_t3, "11111111bbbb");
    make_t3_profile(&custom_t3, "22222222cccc");
    // A directory without `antigravity-acp` is ignored.
    fs::create_dir_all(custom_t3.join("userdata/providers/antigravity/ignored/config")).unwrap();

    hosts
        .context
        .env
        .insert("T3CODE_HOME".into(), custom_t3.display().to_string());
    hosts.t3_antigravity_homes = hosts.other_t3_antigravity_homes();

    let first = host(&hosts, "antigravity@t3-11111111aaaa");
    let second = host(&hosts, "antigravity@t3-11111111bbbb");
    let third = host(&hosts, "antigravity@t3-22222222");
    assert_eq!(first.label, "Antigravity（T3 Code · 11111111aaaa）");
    assert_eq!(second.label, "Antigravity（T3 Code · 11111111bbbb）");
    assert_eq!(third.label, "Antigravity（T3 Code · 22222222）");
    assert!(
        !hosts
            .registry()
            .iter()
            .any(|row| row.id.contains("ignored"))
    );
}

#[cfg(unix)]
#[test]
fn unsafe_t3_skills_symlink_stays_blocked() {
    let (_temp, mut hosts) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let profile = make_t3_profile(&hosts.context.home.join(".t3"), "cafebabe9999");
    std::os::unix::fs::symlink(outside.path(), profile.join("config/skills")).unwrap();

    hosts.t3_antigravity_homes = hosts.other_t3_antigravity_homes();
    let row = host(&hosts, "antigravity@t3-cafebabe");
    assert_eq!(hosts.state(&row)["skill"]["state"], "blocked");
}

#[test]
fn uninstall_removes_t3_antigravity_entry() {
    let (_temp, mut hosts) = fixture();
    let profile = make_t3_profile(&hosts.context.home.join(".t3"), "feedface0000");
    hosts.t3_antigravity_homes = hosts.other_t3_antigravity_homes();
    let t3_id = "antigravity@t3-feedface";
    assert_eq!(
        hosts.action("connect", &json!({"id": t3_id})).unwrap()["ok"],
        true
    );
    let failures = hosts.uninstall();
    assert!(failures.is_empty(), "{failures:?}");
    assert!(host(&hosts, t3_id).read().unwrap().entry.is_none());
    let _ = profile;
}

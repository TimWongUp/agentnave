use super::*;

fn with_accounts() -> (TempDir, Hosts, Vec<crate::accounts::Account>) {
    let (temp, hosts) = fixture();
    executable_file(
        &hosts.context.search[0].join("claude"),
        "#!/bin/sh\nexit 99\n",
    );
    let (data, home) = (&hosts.context.data, &hosts.context.home);
    let accounts = vec![
        crate::accounts::create(data, home, "工作").unwrap(),
        crate::accounts::create(data, home, "个人").unwrap(),
    ];
    (temp, hosts, accounts)
}

#[test]
fn each_named_account_is_a_row_read_from_its_own_folder() {
    let (_temp, hosts, accounts) = with_accounts();
    assert_eq!(hosts.registry().len(), IDS.len() + 2);
    for account in &accounts {
        let row = host(&hosts, &format!("claude@{}", account.id));
        assert_eq!(row.boss, "claude");
        assert_eq!(row.config, account.dir.join(".claude.json"));
        assert_eq!(row.root.path, account.dir);
        assert_eq!(
            row.skill,
            Some(account.dir.join("skills/agentnave-manager"))
        );
    }
    assert_eq!(host(&hosts, "claude@a1").label, "Claude（工作）");
    // The default account's row keeps its own folder.
    assert_eq!(
        host(&hosts, "claude").config,
        hosts.context.home.join(".claude.json")
    );
}

#[test]
fn connecting_an_account_writes_its_folder_only_and_names_the_row() {
    let (_temp, mut hosts, accounts) = with_accounts();
    let account = &accounts[0];
    let result = hosts
        .action("connect", &json!({"id": "claude@a1"}))
        .unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["host"]["has_host_env"], true);
    let entry = host(&hosts, "claude@a1").read().unwrap().entry.unwrap();
    assert_eq!(entry["env"]["AGENTNAVE_HOST"], "claude");
    assert_eq!(entry["env"]["AGENTNAVE_HOST_APP"], "claude@a1");
    assert!(account.dir.join(".claude.json").is_file());
    assert!(
        account
            .dir
            .join("skills/agentnave-manager/SKILL.md")
            .is_file()
    );
    // Neither the default account nor the other named one was touched.
    assert!(host(&hosts, "claude").read().unwrap().entry.is_none());
    assert!(host(&hosts, "claude@a2").read().unwrap().entry.is_none());
    assert!(!accounts[1].dir.join(".claude.json").exists());
    // The server reads this row's mode under its own name.
    let effective = hosts.effective_apps(&json!({"version": 1, "preset": "split"}));
    assert_eq!(effective["claude@a1"], "split");
    assert!(effective.get("claude@a2").is_none());
    // Claude Desktop does not read a named folder.
    let state = hosts.state(&host(&hosts, "claude@a1"));
    assert!(
        state["clients"]
            .as_array()
            .unwrap()
            .iter()
            .all(|client| client["kind"] != "desktop")
    );
}

#[test]
fn removing_or_uninstalling_cleans_the_connection_and_keeps_the_folder() {
    let (_temp, mut hosts, accounts) = with_accounts();
    for id in ["claude@a1", "claude@a2"] {
        hosts.action("connect", &json!({"id": id})).unwrap();
    }
    let result = hosts.action("remove", &json!({"id": "claude@a1"})).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert!(host(&hosts, "claude@a1").read().unwrap().entry.is_none());
    assert!(host(&hosts, "claude@a2").read().unwrap().entry.is_some());
    let failures = hosts.uninstall();
    assert!(failures.is_empty(), "{failures:?}");
    assert!(host(&hosts, "claude@a2").read().unwrap().entry.is_none());
    assert!(accounts.iter().all(|account| account.dir.is_dir()));
}

#[test]
fn an_unreadable_accounts_file_adds_no_rows() {
    let (_temp, hosts) = fixture();
    save(&hosts.context.data.join("accounts.json"), "{broken");
    assert_eq!(hosts.registry().len(), IDS.len());
}

#[test]
fn syncing_an_account_connects_it_and_writes_the_rules_into_its_folder() {
    let (_temp, hosts, accounts) = with_accounts();
    let data = hosts.context.data.clone();
    // Rules wait while the installed service is older than this App.
    let site = hosts
        .context
        .home
        .join(".local/share/uv/tools/agentnave/lib/python3.12/site-packages");
    fs::remove_dir_all(site.join("agentnave-0.22.0.dist-info")).unwrap();
    save(
        &site.join(format!("agentnave-{VERSION}.dist-info/entry_points.txt")),
        "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n",
    );
    crate::members::save_orchestration(&data, &json!({"version": 1, "preset": "split"})).unwrap();
    let state = Mutex::new(Some(hosts));
    let synced = sync_account(&data, &state, "a1").unwrap();
    assert!(!synced.is_empty());
    let rules = fs::read_to_string(accounts[0].dir.join("CLAUDE.md")).unwrap();
    assert!(rules.contains("AGENTNAVE:START"));
    assert!(!accounts[1].dir.join("CLAUDE.md").exists());
    // Taking the account away takes the connection out of its folder again.
    unsync_account(&data, &state, "a1").unwrap();
    let guard = state.lock().unwrap();
    let hosts = guard.as_ref().unwrap();
    assert!(host(hosts, "claude@a1").read().unwrap().entry.is_none());
}

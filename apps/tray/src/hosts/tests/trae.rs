use super::*;

#[test]
fn cli_uses_its_own_home_and_native_toml_contract() {
    let (_temp, mut hosts) = fixture();
    let root = hosts.context.home.join("cli-custom");
    hosts
        .context
        .env
        .insert("TRAE_HOME".into(), root.display().to_string());
    let cli = host(&hosts, "trae");
    assert_eq!(cli.label, "TRAE CLI");
    assert_eq!(cli.config, root.join("traecli.toml"));
    assert_eq!(cli.root.source, "env");
    assert_eq!(cli.skill, Some(root.join("skills/agentnave-manager")));
    save(
        &cli.config,
        "# native\n[mcp_servers.fixture]\ncommand = '/fixture'\nargs = ['--test']\n[mcp_servers.fixture.env]\nKEY = 'keep'\n",
    );
    let before = cli.read().unwrap();
    hosts
        .write(
            &cli,
            &before,
            Some(&json!({"command":"/server", "env":host_env(&cli)})),
        )
        .unwrap();
    let after = fs::read_to_string(&cli.config).unwrap();
    assert!(after.contains("# native"));
    let document: toml::Table = after.parse().unwrap();
    assert_eq!(
        document["mcp_servers"]["fixture"]["command"].as_str(),
        Some("/fixture")
    );
    assert_eq!(
        cli.read().unwrap().entry.unwrap()["env"],
        json!({"AGENTNAVE_HOST":"trae", "AGENTNAVE_HOST_APP":"trae"})
    );
    assert!(!backup_paths(&hosts.context.app, CONFIG_BACKUP).is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn desktop_has_independent_paths_and_project_mcp_is_read_only() {
    let (_temp, mut hosts) = fixture();
    let home = hosts.context.home.clone();
    executable_file(
        &hosts.context.applications[0].join("Trae CN.app/Contents/MacOS/Electron"),
        "fake",
    );
    let desktop = host(&hosts, "trae-desktop");
    let cli = host(&hosts, "trae");
    assert!(hosts.detect(&desktop).0);
    assert!(!hosts.detect(&cli).0);
    assert_eq!(desktop.boss, "trae");
    assert_eq!(
        desktop.config,
        home.join("Library/Application Support/Trae CN/User/mcp.json")
    );
    assert_eq!(
        desktop.skill,
        Some(home.join(".trae-cn/skills/agentnave-manager"))
    );
    assert_eq!(
        crate::hosts::trae::rules_root(&desktop),
        home.join(".trae-cn")
    );
    let project = hosts.context.project.join(".trae/mcp.json");
    let original = r#"{"mcpServers":{"agentnave":{"command":"project-only"}}}"#;
    save(&project, original);
    let before = desktop.read().unwrap();
    hosts
        .write(
            &desktop,
            &before,
            Some(&json!({"command":"/server", "env":host_env(&desktop)})),
        )
        .unwrap();
    assert_eq!(fs::read_to_string(&project).unwrap(), original);
    assert_eq!(hosts.state(&desktop)["shadowed_by"], json!(project));
    assert_eq!(
        desktop.read().unwrap().entry.unwrap()["env"],
        json!({"AGENTNAVE_HOST":"trae", "AGENTNAVE_HOST_APP":"trae-desktop"})
    );
    assert!(cli.read().unwrap().entry.is_none());
    hosts.context.env.insert(
        "TRAE_HOME".into(),
        home.join("elsewhere").display().to_string(),
    );
    assert_eq!(host(&hosts, "trae-desktop").config, desktop.config);
}

#[test]
fn cli_root_resolution_honors_shell_env_and_explicit_precedence() {
    let (_temp, mut hosts) = fixture();
    let home = hosts.context.home.clone();
    hosts.shell = Some(Ok(BTreeMap::from([(
        "TRAE_HOME".into(),
        home.join("shell").display().to_string(),
    )])));
    assert_eq!(host(&hosts, "trae").root.source, "shell");
    hosts
        .context
        .env
        .insert("TRAE_HOME".into(), home.join("env").display().to_string());
    assert_eq!(host(&hosts, "trae").root.source, "env");
    hosts.settings["trae"] = json!({"root":home.join("explicit")});
    assert_eq!(host(&hosts, "trae").root.source, "user");
}

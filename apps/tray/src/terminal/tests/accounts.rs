use super::super::account;

#[cfg(unix)]
fn active(dir: &str) -> account::Active {
    account::Active {
        id: "a1".to_owned(),
        label: "工作".to_owned(),
        dir: std::path::PathBuf::from(dir),
    }
}

#[test]
fn a_chat_runs_as_its_account_and_a_removed_or_foreign_one_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let made = crate::accounts::create(root.path(), home.path(), "工作").unwrap();
    let found = account::resolve_in(root.path(), "claude", Some("a1"))
        .unwrap()
        .unwrap();
    assert_eq!((found.id.as_str(), found.dir), ("a1", made.dir));
    // No account named, or an empty name, is the default account.
    assert_eq!(account::resolve_in(root.path(), "claude", None), Ok(None));
    assert_eq!(
        account::resolve_in(root.path(), "claude", Some("")),
        Ok(None)
    );
    assert!(account::resolve_in(root.path(), "claude", Some("a9")).is_err());
    assert!(account::resolve_in(root.path(), "codex", Some("a1")).is_err());
}

#[cfg(unix)]
#[test]
fn a_named_account_starts_through_env_so_a_login_shell_cannot_overwrite_it() {
    use portable_pty::CommandBuilder;
    use std::path::PathBuf;

    let (program, args, env) = account::launch_as(
        &active("/accounts/work"),
        PathBuf::from("/bin/sh"),
        vec![
            "-c".to_owned(),
            "printf 'dir:%s token:%s.' \"$CLAUDE_CONFIG_DIR\" \"$CLAUDE_CODE_OAUTH_TOKEN\""
                .to_owned(),
        ],
    );
    assert_eq!(program, PathBuf::from("/usr/bin/env"));
    let at = args.iter().position(|arg| arg == "/bin/sh").unwrap();
    assert_eq!(args[at - 1], "CLAUDE_CONFIG_DIR=/accounts/work");
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.cwd(std::env::temp_dir());
    // What a shell startup file would have exported before the CLI runs.
    command.env("CLAUDE_CONFIG_DIR", "/from/rc");
    command.env("CLAUDE_CODE_OAUTH_TOKEN", "from-rc");
    for (name, value) in env {
        command.env(name, value);
    }
    let session = super::Session::start(command, 80, 24).unwrap();
    let (text, _) = super::output_until(&session, "dir:/accounts/work token:.");
    assert!(text.contains("dir:/accounts/work token:."), "{text:?}");
}

#[cfg(unix)]
#[test]
fn a_sign_in_terminal_is_no_agent_of_the_office() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let cli = temp.path().join("claude");
    std::fs::write(
        &cli,
        "#!/bin/sh\nprintf 'login:%s:%s' \"$*\" \"$CLAUDE_CONFIG_DIR\"; sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = temp.path().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let terminals = super::super::Terminals::default();
    let id = terminals
        .login("a1", Some(&dir), &cli, temp.path(), (100, 30))
        .unwrap();
    let session = terminals.session(Some(id)).unwrap();
    let wanted = format!("login:auth login:{}", dir.display());
    let (text, _) = super::output_until(&session, &wanted);
    assert!(text.contains(&wanted), "{text:?}");
    // Nothing that lists or wakes agents knows it, and it ends when its page closes it.
    assert!(terminals.lock().open.is_empty());
    assert_eq!(terminals.lock().logins.len(), 1);
    assert_eq!(terminals.close(Some(id)).status, 200);
    assert!(terminals.session(Some(id)).is_none());

    // The default account runs `claude auth login` in the default environment and also stays out
    // of the office.
    let default_id = terminals
        .login("default", None, &cli, temp.path(), (100, 30))
        .unwrap();
    let default_session = terminals.session(Some(default_id)).unwrap();
    let (default_text, _) = super::output_until(&default_session, "login:auth login:");
    assert!(
        default_text.contains("login:auth login:"),
        "{default_text:?}"
    );
    assert!(terminals.lock().open.is_empty());
    assert_eq!(terminals.lock().logins.len(), 1);
    assert_eq!(terminals.close(Some(default_id)).status, 200);
}

#[test]
fn the_workspace_owner_account_is_read_once_the_history_has_the_column() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::db::create(root.path());
    db.execute(
        "INSERT INTO workspaces (id, owner, owner_host) VALUES ('s-a', 's-a', 'claude'), \
         ('s-b', 's-b', 'claude')",
        [],
    )
    .unwrap();
    // A history from before schema 5 names no account.
    let column: bool = db
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('workspaces') WHERE name = 'owner_account')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if !column {
        assert_eq!(super::super::links::owner_account(root.path(), "s-a"), None);
        db.execute("ALTER TABLE workspaces ADD COLUMN owner_account TEXT", [])
            .unwrap();
    }
    db.execute(
        "UPDATE workspaces SET owner_account = 'a1' WHERE id = 's-a'",
        [],
    )
    .unwrap();
    let owner = |workspace| super::super::links::owner_account(root.path(), workspace);
    assert_eq!(owner("s-a").as_deref(), Some("a1"));
    assert_eq!(owner("s-b"), None);
    assert_eq!(owner("s-missing"), None);
}

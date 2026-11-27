use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use super::{
    ParentEnv, invalid_temp_overrides_in, plan_startup_env, resolve_usable_in,
    sanitize_env_map_with, usable,
};

/// Each name set to a value that carries no meaning of its own.
fn named<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<(OsString, OsString)> {
    names
        .into_iter()
        .map(|name| (OsString::from(name), OsString::from("1")))
        .collect()
}

fn is_startup_parent_session(name: &str) -> bool {
    ParentEnv::new(named([])).is_parent_session(name, "1".as_ref())
}

#[test]
fn resolve_usable_in_falls_back_when_current_dir_is_missing() {
    let valid = tempfile::tempdir().unwrap();
    let missing = valid.path().join("gone-tmpdir");
    let also_missing = valid.path().join("also-gone");
    let resolved = resolve_usable_in(missing.clone(), [also_missing, valid.path().to_path_buf()]);
    assert_eq!(resolved, valid.path());

    // Existing current temp dir is preserved as-is.
    let kept = resolve_usable_in(valid.path().to_path_buf(), [PathBuf::from("/tmp")]);
    assert_eq!(kept, valid.path());
    assert!(usable().is_dir());
}

#[test]
fn startup_plan_triggers_temp_reset_only_for_agent_sessions_or_missing_temp() {
    let valid = tempfile::tempdir().unwrap();
    let missing = valid.path().join("gone");

    // Regular terminal vars and persistent tokens (CLAUDE_CODE_OAUTH_TOKEN) are stripped
    // but do NOT reset a valid TMPDIR; user config vars like ANTIGRAVITY_CLI_HOME stay intact.
    let terminal_plan = plan_startup_env(
        named([
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "PWD",
            "OLDPWD",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "T3CODE_HOME",
            "ANTIGRAVITY_CLI_HOME",
            "AGENTNAVE_DATA_DIR",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
            "CODEBUDDY_CONFIG_DIR",
            "ZCODE_DATA_BASE_DIR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "ANTHROPIC_API_KEY",
        ]),
        valid.path(),
    );
    assert!(!terminal_plan.reset_temp);
    assert_eq!(
        terminal_plan.remove_vars,
        vec![
            OsString::from("TERM_PROGRAM"),
            OsString::from("TERM_PROGRAM_VERSION"),
            OsString::from("PWD"),
            OsString::from("OLDPWD"),
            OsString::from("CLAUDE_CODE_OAUTH_TOKEN"),
        ]
    );

    // Missing current temp dir triggers reset even without agent session vars.
    let missing_plan = plan_startup_env(named(["HOME"]), &missing);
    assert!(missing_plan.reset_temp);
    assert!(missing_plan.remove_vars.is_empty());

    // Agent session vars trigger reset and are stripped, while config vars stay intact.
    for trigger in [
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_SSE_PORT",
        "CODEX_THREAD_ID",
        "ANTIGRAVITY_CONVERSATION_ID",
        "HERDR_PANE_ID",
        "T3_ACP_MCP_NODE",
        "T3CODE_THREAD_ID",
        "ELECTRON_RUN_AS_NODE",
        "AGENTNAVE_HOST_SESSION_ID",
        "SERVER__PORT",
    ] {
        let plan = plan_startup_env(
            named([
                trigger,
                "T3CODE_HOME",
                "ANTIGRAVITY_CLI_HOME",
                "AGENTNAVE_DATA_DIR",
                "CLAUDE_CODE_USE_BEDROCK",
            ]),
            valid.path(),
        );
        assert!(plan.reset_temp, "expected temp reset for {trigger}");
        assert_eq!(plan.remove_vars, vec![OsString::from(trigger)]);
    }
}

#[test]
fn startup_parent_session_preserves_user_config_and_gateway_vars() {
    for preserved in [
        "T3CODE_HOME",
        "t3code_home",
        "ANTIGRAVITY_CLI_HOME",
        "antigravity_cli_home",
        "AGENTNAVE_DATA_DIR",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "CODEBUDDY_CONFIG_DIR",
        "ZCODE_DATA_BASE_DIR",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "ANTHROPIC_API_KEY",
        "PATH",
        "HOME",
    ] {
        assert!(
            !is_startup_parent_session(preserved),
            "{preserved} must not be stripped at startup"
        );
    }
    for stripped in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SSE_PORT",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_DESKTOP_APP_VERSION",
        "T3CODE_THREAD_ID",
        "T3_ACP_MCP_ENTRYPOINT",
        "ANTIGRAVITY_LS_ADDRESS",
        "ELECTRON_RUN_AS_NODE",
        "PWD",
        "OLDPWD",
        "TERM_PROGRAM",
    ] {
        assert!(
            is_startup_parent_session(stripped),
            "{stripped} should be stripped at startup"
        );
    }
}

#[test]
fn only_the_values_t3_code_injects_are_stripped() {
    let valid = tempfile::tempdir().unwrap();
    let t3_home = valid.path().join("t3");
    let pairs = |vars: &[(&str, &str)]| -> Vec<(OsString, OsString)> {
        vars.iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    };
    let profile = t3_home.join("userdata/providers/antigravity/p1");
    let user = pairs(&[
        ("SSH_CONNECTION", "10.0.0.2 50000 10.0.0.1 22"),
        ("SSH_CLIENT", "10.0.0.2 50000 22"),
        ("SSH_TTY", "/dev/ttys001"),
        ("BROWSER", "firefox"),
        ("GEMINI_HOME", "/Users/me/.gemini"),
        ("T3CODE_HOME", t3_home.to_str().unwrap()),
    ]);
    let plan = plan_startup_env(user, valid.path());
    assert!(!plan.reset_temp);
    assert!(plan.remove_vars.is_empty(), "{:?}", plan.remove_vars);

    // T3's loopback SSH_CONNECTION and BROWSER stub prove a T3 session; its profile GEMINI_HOME,
    // recognised under a custom T3CODE_HOME, goes with them.
    let t3 = pairs(&[
        ("SSH_CONNECTION", "127.0.0.1 0 127.0.0.1 0"),
        (
            "BROWSER",
            "/bin/sh -c 'echo __T3_ANTIGRAVITY_AUTH_URL__ $1'",
        ),
        ("GEMINI_HOME", profile.to_str().unwrap()),
        ("T3CODE_HOME", t3_home.to_str().unwrap()),
    ]);
    let plan = plan_startup_env(t3, valid.path());
    assert!(plan.reset_temp);
    assert_eq!(
        plan.remove_vars,
        ["SSH_CONNECTION", "BROWSER", "GEMINI_HOME"].map(OsString::from)
    );

    // File-backed credential storage marks T3's ACP profile whatever GEMINI_HOME names.
    let plan = plan_startup_env(
        pairs(&[
            ("AGY_ACP_FORCE_FILE_STORAGE", "1"),
            ("GEMINI_HOME", "/elsewhere"),
        ]),
        valid.path(),
    );
    assert!(plan.reset_temp);
    assert_eq!(
        plan.remove_vars,
        ["AGY_ACP_FORCE_FILE_STORAGE", "GEMINI_HOME"].map(OsString::from)
    );

    // The map handed to a reopened host is filtered the same way.
    let mut env = BTreeMap::new();
    env.insert(
        "ANTIGRAVITY_APP_DATA_DIR".to_owned(),
        "/p/antigravity-acp".to_owned(),
    );
    env.insert("GEMINI_HOME".to_owned(), "/p".to_owned());
    env.insert("BROWSER".to_owned(), "open".to_owned());
    let clean = sanitize_env_map_with(&env, || valid.path().to_path_buf());
    assert_eq!(clean.keys().collect::<Vec<_>>(), ["BROWSER"]);
}

#[test]
fn invalid_temp_overrides_only_replaces_missing_dirs() {
    let valid = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    let missing = valid.path().join("deleted-tmpdir");

    // Unset -> no override.
    let none = invalid_temp_overrides_in(|_| None, || fallback.path().to_path_buf());
    assert!(none.is_empty());

    // Valid existing directory -> no override.
    let ok = invalid_temp_overrides_in(
        |_| Some(valid.path().as_os_str().to_os_string()),
        || fallback.path().to_path_buf(),
    );
    assert!(ok.is_empty());

    // Non-existent directory -> overridden with fallback.
    let fixed = invalid_temp_overrides_in(
        |_| Some(missing.as_os_str().to_os_string()),
        || fallback.path().to_path_buf(),
    );
    assert!(!fixed.is_empty());
    for (_, dir) in &fixed {
        assert_eq!(dir, fallback.path());
    }

    // Applying the overrides to a Command sets the expected env entry.
    let mut cmd = Command::new("true");
    for (k, v) in fixed {
        cmd.env(k, v);
    }
    let envs: Vec<_> = cmd.get_envs().collect();
    assert!(!envs.is_empty());
    assert_eq!(envs[0].1, Some(fallback.path().as_os_str()));
}

#[test]
fn sanitize_env_map_strips_parent_session_and_repairs_broken_temp() {
    let valid = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    let missing = valid.path().join("vanished");
    let mut env = BTreeMap::new();
    env.insert("TMPDIR".into(), missing.display().to_string());
    env.insert("TEMP".into(), missing.display().to_string());
    env.insert("TMP".into(), valid.path().display().to_string());
    env.insert("tmpdir".into(), "not-a-temp-dir".into());
    env.insert("CLAUDECODE".into(), "1".into());
    env.insert("T3_ACP_MCP_NODE".into(), "/node".into());
    env.insert("T3CODE_HOME".into(), "/keep/t3".into());
    env.insert("ANTIGRAVITY_CLI_HOME".into(), "/keep/agy".into());
    env.insert("CLAUDE_CODE_USE_BEDROCK".into(), "1".into());
    env.insert("AGENTNAVE_DATA_DIR".into(), "/keep/data".into());

    let clean = sanitize_env_map_with(&env, || fallback.path().to_path_buf());
    let expected = fallback.path().to_string_lossy();
    assert_eq!(
        clean.get("TMPDIR").map(String::as_str),
        Some(expected.as_ref())
    );
    assert_eq!(
        clean.get("TEMP").map(String::as_str),
        Some(expected.as_ref())
    );
    assert_eq!(
        clean.get("TMP").map(String::as_str),
        Some(valid.path().to_string_lossy().as_ref())
    );
    #[cfg(unix)]
    assert_eq!(
        clean.get("tmpdir").map(String::as_str),
        Some("not-a-temp-dir")
    );
    #[cfg(windows)]
    assert_eq!(
        clean.get("tmpdir").map(String::as_str),
        Some(expected.as_ref())
    );
    assert!(!clean.contains_key("CLAUDECODE"));
    assert!(!clean.contains_key("T3_ACP_MCP_NODE"));
    assert_eq!(
        clean.get("T3CODE_HOME").map(String::as_str),
        Some("/keep/t3")
    );
    assert_eq!(
        clean.get("ANTIGRAVITY_CLI_HOME").map(String::as_str),
        Some("/keep/agy")
    );
    assert_eq!(
        clean.get("CLAUDE_CODE_USE_BEDROCK").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        clean.get("AGENTNAVE_DATA_DIR").map(String::as_str),
        Some("/keep/data")
    );
}

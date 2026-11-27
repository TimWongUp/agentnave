//! What `claude auth status --json` says about one account folder. The answer goes to the page
//! that asked and nowhere else: the e-mail and plan are never written to a file or the log.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(15);
/// The variable that points Claude at its folder.
pub(crate) const CONFIG_VARIABLE: &str = "CLAUDE_CONFIG_DIR";

/// The installed Claude CLI.
pub(crate) fn program() -> Option<PathBuf> {
    let detected = crate::detect::detected(false, crate::members::codex_source);
    detected[super::PROVIDER]["installed"]
        .as_bool()
        .filter(|installed| *installed)
        .and(detected[super::PROVIDER]["path"].as_str())
        .map(PathBuf::from)
}

/// The sign-in state shown on an account's row: `dir` is its folder, or None for the default
/// account, which keeps the environment as it is.
pub(crate) fn status(dir: Option<&Path>) -> Value {
    let Some(program) = program() else {
        return json!({"error": "这个 CLI 没有安装"});
    };
    status_with(&program, dir)
}

pub(super) fn status_with(program: &Path, dir: Option<&Path>) -> Value {
    let mut command = std::process::Command::new(program);
    command.args(["auth", "status", "--json"]);
    command.stdin(std::process::Stdio::null());
    command.current_dir(crate::temp_dir::usable());
    for name in crate::terminal::parent_session_names(std::env::vars_os()) {
        command.env_remove(name);
    }
    crate::temp_dir::apply_to_command(&mut command);
    if let Some(path) = crate::detect::search_path(program) {
        command.env("PATH", path);
    }
    if let Some(dir) = dir {
        command.env(CONFIG_VARIABLE, dir);
    }
    crate::process::hide_window(&mut command);
    let Ok(output) = crate::process::output(command, LIMIT) else {
        return json!({"error": "无法读取登录状态"});
    };
    // A folder nobody signed in to answers with a failure status and still names its state.
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(reported) => shown(&reported),
        Err(_) => json!({"error": "无法读取登录状态"}),
    }
}

/// Only what the row shows, as plain strings.
fn shown(reported: &Value) -> Value {
    let text = |key: &str| {
        reported[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    json!({
        "logged_in": reported["loggedIn"] == true,
        "method": text("authMethod"),
        "email": text("email"),
        "plan": text("subscriptionType"),
        "error": null,
    })
}

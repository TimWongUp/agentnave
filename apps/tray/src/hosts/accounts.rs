//! What the accounts page asks of the host connections (ADR-0107): a named Claude account's
//! folder carries AgentNave's connection, Skills and rules like any other boss row, and nothing of
//! the user's own configuration.
use super::Hosts;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn row_id(account: &str) -> String {
    format!("claude@{account}")
}

fn locked<T>(
    host_state: &Mutex<Option<Hosts>>,
    run: impl FnOnce(&mut Hosts) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
    let hosts = guard.as_mut().ok_or("宿主接入尚未初始化")?;
    if hosts.uninstalling {
        return Err("卸载已开始，请完成卸载或重新打开 App".into());
    }
    run(hosts)
}

/// The folder Claude uses when no account is named: the `claude` row's, which follows
/// `CLAUDE_CONFIG_DIR` from the app's or the login shell's environment.
pub(crate) fn default_claude_dir(host_state: &Mutex<Option<Hosts>>) -> Option<PathBuf> {
    let guard = host_state.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .as_ref()?
        .registry()
        .into_iter()
        .find(|host| host.id == "claude")
        .map(|host| host.root.path)
}

/// Connects the account's row: the MCP entry, the Skills and the rules block in its folder.
pub(crate) fn sync_account(
    root: &Path,
    host_state: &Mutex<Option<Hosts>>,
    account: &str,
) -> Result<String, String> {
    locked(host_state, |hosts| {
        let result = hosts.action("connect", &json!({"id": row_id(account)}))?;
        let _ = super::sync_orchestration(root, hosts, &[]);
        let message = result["message"].as_str().unwrap_or_default().to_owned();
        if result["ok"] == true {
            Ok(message)
        } else {
            Err(message)
        }
    })
}

/// Takes the connection out of the account's folder, which stays otherwise as it is. Called
/// while the account is still registered.
pub(crate) fn unsync_account(
    root: &Path,
    host_state: &Mutex<Option<Hosts>>,
    account: &str,
) -> Result<(), String> {
    locked(host_state, |hosts| {
        let id = row_id(account);
        let result = hosts.action("remove", &json!({"id": id}));
        let _ = super::sync_orchestration(root, hosts, &[]);
        let result: Value = result?;
        if result["ok"] == true {
            Ok(())
        } else {
            Err(result["message"].as_str().unwrap_or_default().to_owned())
        }
    })
}

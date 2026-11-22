//! Terminals that run a named account (ADR-0108): the account the page picked or the workspace
//! belongs to, the way its CLI is started, and the sign-in terminal that belongs to no workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use super::session::Session;

/// A named account a CLI runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Active {
    pub id: String,
    pub label: String,
    pub dir: PathBuf,
}

/// A sign-in terminal: `claude auth login` for one account. It is no agent of the office, so it
/// is neither listed beside the others nor given wake-ups, and nothing can be continued from it.
pub(super) struct Login {
    pub id: u64,
    pub account: String,
    pub session: Arc<Session>,
}

/// What `resolve` says of an account that is not registered (any more).
const REMOVED: &str = "这个账号已被移除";

/// The account a page asked for: none when left out or `null`; anything but a name is refused.
pub(super) fn requested(payload: &Value) -> Result<Option<&str>, String> {
    match &payload["account"] {
        Value::Null => Ok(None),
        Value::String(id) => Ok(Some(id)),
        _ => Err("账号需为账号 ID".into()),
    }
}

/// Why the account a chat began as cannot run it: `removed` when that account is gone, else what
/// is wrong with the accounts file.
pub(super) fn chat_refusal<'a>(error: &'a str, removed: &'a str) -> &'a str {
    if error == REMOVED { removed } else { error }
}

/// The account named by `id`, None for the default account.
pub(super) fn resolve(provider: &str, id: Option<&str>) -> Result<Option<Active>, String> {
    resolve_in(&crate::data_files::data_root(), provider, id)
}

pub(super) fn resolve_in(
    root: &Path,
    provider: &str,
    id: Option<&str>,
) -> Result<Option<Active>, String> {
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    if provider != crate::accounts::PROVIDER {
        return Err("这个 CLI 没有账号".into());
    }
    let accounts = crate::accounts::read(root)?;
    accounts
        .into_iter()
        .find(|account| account.id == id)
        .map(|account| {
            Some(Active {
                id: account.id,
                label: account.label,
                dir: account.dir,
            })
        })
        .ok_or_else(|| REMOVED.to_owned())
}

/// What a shell startup file may export that would sign a named account's CLI in as someone else
/// or make it act as part of another Claude session; the account's own folder decides instead.
const SHELL_OVERRIDES: [&str; 4] = [
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
    "CLAUDE_CODE_ACCOUNT_UUID",
];

/// The program, arguments and extra environment that start `program` as `account`. On Unix the
/// variable goes through `/usr/bin/env`, which runs after the login shell's startup files and so
/// cannot be overwritten by them, and drops `SHELL_OVERRIDES` they may have set; Windows has no
/// login shell and sets it directly.
pub(super) fn launch_as(
    account: &Active,
    program: PathBuf,
    args: Vec<String>,
    derived: bool,
) -> (PathBuf, Vec<String>, Vec<(String, String)>) {
    let variable = crate::accounts::CONFIG_VARIABLE;
    if cfg!(unix) {
        let path = crate::detect::search_path(&program)
            .map(|path| vec![("PATH".to_owned(), path.to_string_lossy().into_owned())])
            .unwrap_or_default();
        let mut wrapped: Vec<String> = SHELL_OVERRIDES
            .iter()
            .flat_map(|name| ["-u".to_owned(), (*name).to_owned()])
            .collect();
        wrapped.push(format!("{variable}={}", account.dir.display()));
        if derived {
            wrapped.push("AGENTNAVE_DERIVED_SESSION=1".to_owned());
        }
        wrapped.push(program.display().to_string());
        wrapped.extend(args);
        (PathBuf::from("/usr/bin/env"), wrapped, path)
    } else {
        let mut env = vec![(variable.to_owned(), account.dir.display().to_string())];
        if derived {
            env.push(("AGENTNAVE_DERIVED_SESSION".to_owned(), "1".to_owned()));
        }
        (program, args, env)
    }
}

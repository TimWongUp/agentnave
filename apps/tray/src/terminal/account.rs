//! Terminals that run a named account (ADR-0106): the account the page picked or the workspace
//! belongs to, the way its CLI is started, and the sign-in terminal that belongs to no workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        .ok_or_else(|| "这个账号已被移除".to_owned())
}

/// The program, arguments and extra environment that start `program` as `account`. On Unix the
/// variable goes through `/usr/bin/env`, which runs after the login shell's startup files and so
/// cannot be overwritten by them; Windows has no login shell and sets it directly.
pub(super) fn launch_as(
    account: &Active,
    program: PathBuf,
    args: Vec<String>,
) -> (PathBuf, Vec<String>, Vec<(String, String)>) {
    let variable = crate::accounts::CONFIG_VARIABLE;
    if cfg!(unix) {
        let path = crate::detect::search_path(&program)
            .map(|path| vec![("PATH".to_owned(), path.to_string_lossy().into_owned())])
            .unwrap_or_default();
        let mut wrapped = vec![
            format!("{variable}={}", account.dir.display()),
            program.display().to_string(),
        ];
        wrapped.extend(args);
        (PathBuf::from("/usr/bin/env"), wrapped, path)
    } else {
        (
            program,
            args,
            vec![(variable.to_owned(), account.dir.display().to_string())],
        )
    }
}

//! The messages route of a derived session: the turns its native log holds beyond the tasks the
//! boss dispatched (ADR-0128).

use std::path::{Path, PathBuf};

use serde_json::json;

#[cfg(test)]
mod tests;

use super::derived::native_logs::turns::{
    TaskMatch, find_transcript_path, read_user_turns_at_path,
};
use super::{Terminals, account, lifecycle, thread_desk_key};
use crate::data_files::valid_id;
use crate::response::Response;

/// A live derived terminal that holds the desk's session.
struct LiveOwner {
    provider: String,
    native: String,
    account_dir: Option<PathBuf>,
    alive: bool,
}

/// The session whose log is read, with what the service history knows of it.
struct Target {
    provider: String,
    native: String,
    account_id: Option<String>,
    live_account_dir: Option<PathBuf>,
    tasks: Vec<TaskMatch>,
    is_live: bool,
}

fn empty_turns(error: Option<&str>) -> Response {
    match error {
        Some(error) => Response::json(&json!({"turns": [], "error": error})),
        None => Response::json(&json!({"turns": []})),
    }
}

impl Terminals {
    pub(super) fn derived_turns(&self, workspace: &str, desk: &str) -> Response {
        self.derived_turns_in(&crate::data_files::data_root(), workspace, desk)
    }

    pub(super) fn derived_turns_in(&self, root: &Path, workspace: &str, desk: &str) -> Response {
        if !valid_id(workspace) || desk.is_empty() {
            return Response::error(400, "需要有效的工作区 ID 和工位");
        }
        let target = match self.derived_target(root, workspace, desk) {
            Ok(target) => target,
            Err(response) => return response,
        };
        let account_dir = match target.live_account_dir {
            Some(dir) => Some(dir),
            None => match account::resolve_in(root, &target.provider, target.account_id.as_deref())
            {
                Ok(active) => active.map(|a| a.dir),
                Err(err) => {
                    return empty_turns(Some(&format!("账号不可用，无法读取原生日志：{err}")));
                }
            },
        };
        let Some(path) =
            find_transcript_path(&target.provider, account_dir.as_deref(), &target.native)
        else {
            if target.is_live && target.tasks.is_empty() {
                return empty_turns(None);
            }
            return empty_turns(Some("找不到原生会话日志（可能已被 CLI 清理）"));
        };
        match read_user_turns_at_path(&target.provider, &path, &target.tasks) {
            Ok(turns) => Response::json(&json!({"turns": turns})),
            Err(err) => empty_turns(Some(&err)),
        }
    }

    /// The live derived terminal this workspace holds for `desk`, if any.
    fn live_owner(&self, workspace: &str, desk: &str) -> Option<LiveOwner> {
        let state = self.lock();
        let running = state.open.iter().find(|r| {
            r.derived
                .as_ref()
                .is_some_and(|d| d.parent_workspace_id.as_deref() == Some(workspace))
                && r.native
                    .as_deref()
                    .is_some_and(|n| !n.is_empty() && thread_desk_key(&r.provider, n) == desk)
        })?;
        Some(LiveOwner {
            provider: running.provider.clone(),
            native: running.native.clone().unwrap_or_default(),
            account_dir: running.account.as_ref().map(|a| a.dir.clone()),
            alive: running.session.exit().is_none(),
        })
    }

    /// The session behind `desk` from the service history, else from its live terminal; a desk
    /// without either answers with the response to give.
    fn derived_target(&self, root: &Path, workspace: &str, desk: &str) -> Result<Target, Response> {
        let live = self.live_owner(workspace, desk);
        match lifecycle::open::find_session_with_tasks(root, workspace, desk) {
            Ok((found, tasks)) => Ok(Target {
                provider: found.provider,
                native: found.native,
                account_id: found.account,
                live_account_dir: live.as_ref().and_then(|l| l.account_dir.clone()),
                tasks,
                is_live: live.as_ref().is_some_and(|l| l.alive),
            }),
            Err(reason) => {
                if let Some(live) = live {
                    let tasks = lifecycle::open::find_pending_terminal_tasks(
                        root,
                        workspace,
                        &live.provider,
                        &live.native,
                    );
                    return Ok(Target {
                        provider: live.provider,
                        native: live.native,
                        account_id: None,
                        live_account_dir: live.account_dir,
                        tasks,
                        is_live: live.alive,
                    });
                }
                if reason != "读不到服务历史" && desk.starts_with("task:") {
                    return Err(empty_turns(None));
                }
                Err(empty_turns(Some(reason)))
            }
        }
    }
}

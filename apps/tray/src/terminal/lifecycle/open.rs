//! The user opens a derived session from its desk (ADR-0124): the session the service history last
//! ran in an App terminal for that desk, taken up again in its row, or in a new one when it has
//! none here (after the App restarted, its boss closed, or when it began elsewhere). Nothing is
//! submitted to it.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};

use super::super::account::{self, Active};
use super::super::derived::native_logs::turns::TaskMatch;
use super::super::{
    Attach, DerivedInfo, NewDerived, Terminals, derived, links, thread_desk_key, wakeup,
};
use super::revive::{PENDING_WAKE_ERR, Reviving, assumed_config, start};
use super::{Life, Relaunched, WakePlan};
use crate::isolated::leases::HeldLease;
use crate::response::Response;
use crate::time_format::timestamp;

const PENDING_STEP: std::time::Duration = std::time::Duration::from_millis(50);
const PENDING_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The latest call of a desk that ran in an App terminal, as the service history has it.
#[derive(Debug)]
pub(in crate::terminal) struct Found {
    pub(in crate::terminal) provider: String,
    pub(in crate::terminal) native: String,
    pub(in crate::terminal) cwd: PathBuf,
    pub(in crate::terminal) model: Option<String>,
    pub(in crate::terminal) effort: Option<String>,
    pub(in crate::terminal) account: Option<String>,
    pub(in crate::terminal) isolation_id: Option<String>,
    pub(in crate::terminal) workspace: String,
}

fn task_bodies(root: &Path, workspace: &str) -> Result<Vec<Value>, &'static str> {
    let connection = crate::db::read(root)
        .ok()
        .flatten()
        .ok_or("读不到服务历史")?;
    let mut query = connection
        .prepare(
            "SELECT t.body FROM tasks t JOIN conversations c ON c.id = t.conversation_id \
             WHERE c.manager_session = ?1",
        )
        .map_err(|_| "读不到服务历史")?;
    let bodies = query
        .query_map([workspace], |row| row.get::<_, String>(0))
        .map_err(|_| "读不到服务历史")?;
    Ok(bodies
        .filter_map(|body| serde_json::from_str::<Value>(&body.ok()?).ok())
        .collect())
}

fn non_empty_task_field(task: &Value, key: &str) -> Option<String> {
    task[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn to_task_match(task: &Value) -> TaskMatch {
    TaskMatch {
        prompt: task["prompt"].as_str().unwrap_or_default().to_owned(),
        native_anchor: task.get("native_anchor").filter(|v| v.is_object()).cloned(),
        has_forward: task["forward"]
            .as_array()
            .is_some_and(|arr| !arr.is_empty()),
    }
}

/// The latest task of `desk` in `workspace` that ran in an App terminal on a native session,
/// alongside all matching tasks for that native session in chronological order.
pub(in crate::terminal) fn find_session_with_tasks(
    root: &Path,
    workspace: &str,
    desk: &str,
) -> Result<(Found, Vec<TaskMatch>), &'static str> {
    const MISSING: &str = "服务历史里没有这个工位在 App 终端里运行过的会话";
    let bodies = task_bodies(root, workspace)?;
    let mut terminal_matches: Vec<(String, String, Value)> = bodies
        .iter()
        .filter(|task| task["execution_mode"] == "terminal")
        .filter_map(|task| {
            let provider = non_empty_task_field(task, "provider")?;
            let native = non_empty_task_field(task, "session_id")?;
            (thread_desk_key(&provider, &native) == desk).then_some((
                provider,
                native,
                task.clone(),
            ))
        })
        .collect();
    terminal_matches
        .sort_by_key(|(_, _, task)| timestamp(task["time"].as_str().unwrap_or_default()));
    let (provider, native, latest) = terminal_matches.last().cloned().ok_or(MISSING)?;
    let session_tasks = session_task_matches(&bodies, &provider, &native);
    let found = Found {
        cwd: PathBuf::from(non_empty_task_field(&latest, "cwd").unwrap_or_default()),
        model: non_empty_task_field(&latest, "model"),
        effort: non_empty_task_field(&latest, "effort"),
        account: non_empty_task_field(&latest, "account"),
        isolation_id: non_empty_task_field(&latest, "isolation_id"),
        workspace: workspace.to_owned(),
        provider,
        native,
    };
    Ok((found, session_tasks))
}

/// The tasks that ran on one native session, in chronological order. The session's log holds the
/// turns of every one of them, whichever mode it ran in.
fn session_task_matches(bodies: &[Value], provider: &str, native: &str) -> Vec<TaskMatch> {
    let mut matched: Vec<(i64, TaskMatch)> = bodies
        .iter()
        .filter(|task| {
            non_empty_task_field(task, "provider").as_deref() == Some(provider)
                && non_empty_task_field(task, "session_id").as_deref() == Some(native)
        })
        .map(|task| {
            (
                timestamp(task["time"].as_str().unwrap_or_default()).unwrap_or_default(),
                to_task_match(task),
            )
        })
        .collect();
    matched.sort_by_key(|(at, _)| *at);
    matched.into_iter().map(|(_, m)| m).collect()
}

pub(in crate::terminal) fn find_pending_terminal_tasks(
    root: &Path,
    workspace: &str,
    provider: &str,
    native: &str,
) -> Vec<TaskMatch> {
    task_bodies(root, workspace)
        .map(|bodies| session_task_matches(&bodies, provider, native))
        .unwrap_or_default()
}

/// The latest task of `desk` in `workspace` that ran in an App terminal on a native session.
pub(super) fn find(root: &Path, workspace: &str, desk: &str) -> Result<Found, &'static str> {
    find_session_with_tasks(root, workspace, desk).map(|(found, _)| found)
}

/// Whose terminal a session goes into: its row, or a new one with the session held meanwhile.
enum Opening {
    Row(u64),
    New(Reviving),
}

/// What a new row is started with, and the boss it belongs to.
struct Launch {
    plan: WakePlan,
    workspace: Option<String>,
    boss_key: String,
}

impl Terminals {
    pub(in crate::terminal) fn open_derived(
        &self,
        workspace: &str,
        desk: &str,
        attach: &Attach,
    ) -> Response {
        self.open_derived_in(
            &crate::data_files::data_root(),
            workspace,
            desk,
            |provider, native| self.unless_derived_elsewhere(provider, native, attach),
            |plan| derived::relaunch(self, plan),
        )
    }

    pub(super) fn open_derived_in(
        &self,
        root: &Path,
        workspace: &str,
        desk: &str,
        elsewhere: impl FnOnce(&str, &str) -> Option<Response>,
        launch: impl FnOnce(&WakePlan) -> Result<Relaunched, String>,
    ) -> Response {
        if !crate::data_files::valid_id(workspace) || desk.is_empty() {
            return Response::error(400, "需要有效的工作区 ID 和工位");
        }
        let found = match find(root, workspace, desk) {
            Ok(found) => found,
            Err(reason) => return Response::error(404, reason),
        };
        let held = match self.begin_open(&found.provider, &found.native) {
            Ok(Opening::Row(id)) => return self.wake_in(root, Some(id), elsewhere, launch),
            Ok(Opening::New(held)) => held,
            Err(refusal) => return refusal,
        };
        if let Some(refusal) = elsewhere(&found.provider, &found.native) {
            return refusal;
        }
        let planned = match self.plan_for(root, found) {
            Ok(planned) => planned,
            Err(error) => return Response::error(409, &error),
        };
        let (lease, started) = match start(root, &planned.plan, launch) {
            Ok(pair) => pair,
            Err(error) => return Response::error(409, &error),
        };
        let id = self.install_opened(planned, lease, started);
        drop(held);
        wakeup::sync_derived_pids(self);
        let state = self.lock();
        let described = state
            .open
            .iter()
            .find(|r| r.id == id)
            .map(|running| state.describe_one(running, root));
        Response::json(&json!({"session": described}))
    }

    /// The session's row, or the session held for a new one. A session another click or a
    /// re-dispatch is starting a process for is waited for.
    fn begin_open(&self, provider: &str, native: &str) -> Result<Opening, Response> {
        let deadline = Instant::now() + PENDING_WAIT;
        loop {
            {
                let mut state = self.lock();
                if let Some(running) = state.derived_row(provider, native) {
                    return Ok(Opening::Row(running.id));
                }
                if let Some(held) = Reviving::session(self, &mut state, provider, native) {
                    return Ok(Opening::New(held));
                }
            }
            if Instant::now() >= deadline {
                return Err(Response::error(409, PENDING_WAKE_ERR));
            }
            thread::sleep(PENDING_STEP);
        }
    }

    /// The saved launch of the session, else what the service history has of it with the CLI's
    /// own permissions.
    fn plan_for(&self, root: &Path, found: Found) -> Result<Launch, String> {
        let Found {
            provider, native, ..
        } = &found;
        if let Some(saved) = self.saved_launch(provider, native) {
            return Ok(Launch {
                plan: WakePlan {
                    account: saved.account(root, provider),
                    config: Some(saved.config()),
                    cwd: saved.cwd.clone(),
                    fallback_mode: saved.fallback_mode.clone(),
                    isolation_id: saved.isolation_id.clone(),
                    assumed: false,
                    provider: found.provider,
                    native: found.native,
                },
                workspace: saved.parent_workspace_id,
                boss_key: saved.parent_boss_key,
            });
        }
        let account: Option<Active> = account::resolve_in(root, provider, found.account.as_deref())
            .map_err(|error| format!("账号不可用：{error}"))?;
        let boss_key = links::chat(root, &found.workspace)
            .map_or_else(|| found.workspace.clone(), |(_, key, _)| key);
        Ok(Launch {
            plan: WakePlan {
                config: Some(assumed_config(&found.cwd, found.model, found.effort)),
                cwd: found.cwd,
                account,
                fallback_mode: None,
                isolation_id: found.isolation_id,
                assumed: true,
                provider: found.provider,
                native: found.native,
            },
            workspace: Some(found.workspace),
            boss_key,
        })
    }

    /// Adds the row of a session opened without one; it belongs to the boss that dispatched it.
    fn install_opened(
        &self,
        planned: Launch,
        lease: Option<HeldLease>,
        started: Relaunched,
    ) -> u64 {
        let Launch {
            plan,
            workspace,
            boss_key,
        } = planned;
        let life = Life {
            revived: true,
            // Opened by the user: in use, even when its boss is gone.
            outlived_boss: true,
            fallback_mode: plan.fallback_mode,
            config_assumed: plan.assumed,
            ..Life::default()
        };
        self.register_derived(NewDerived {
            provider: plan.provider,
            cwd: plan.cwd,
            session: started.session,
            native: Some(plan.native),
            account: plan.account,
            #[cfg(unix)]
            codex: started.codex,
            info: DerivedInfo {
                parent_boss_key: boss_key,
                parent_workspace_id: workspace,
                busy: false,
                blocked_kind: None,
                isolation_id: plan.isolation_id,
                _lease: lease,
                _log_dir: started.log_dir,
                _hook_dir: started.hook_dir,
                codex_ready_file: None,
                active_invocation_id: None,
                launch_config: plan.config,
                life,
            },
        })
    }
}

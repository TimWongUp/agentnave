//! Codex derived-session execution, rollout reconciliation, and safe turn interruption.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::codex_server::{self, InterruptRpcOutcome, ThreadStatus, TurnStartConfig};
use super::super::session::Session;
use super::super::spawn_requests::{
    BlockedReceipt, Coordinator, ParentRef, Receipt, RunConfig, RunHandle, SpawnRequestEnvelope,
    now_unix_secs, truncate_short,
};
use super::super::{DerivedHandle, DerivedInfo, NewDerived, Terminals, installed_path};
use super::{
    acquire_isolation_for_terminal, cleanup_derived_startup, detect_screen_blocker, native_logs,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const THREAD_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_STEP: Duration = Duration::from_millis(80);
const RESUME_RETRY_GAP: Duration = Duration::from_millis(250);
const INTERRUPT_RETRY_TIMEOUT: Duration = Duration::from_secs(4);
const INTERRUPT_RETRY_GAP: Duration = Duration::from_millis(200);
const INTERRUPT_CONFIRM_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(800)
} else {
    Duration::from_secs(5)
};

#[cfg(test)]
mod tests;

struct ReadyFileGuard(Option<PathBuf>);

impl Drop for ReadyFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::write(&path, b"1");
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_codex(
    coordinator: &Coordinator,
    terminals: &Terminals,
    env: &SpawnRequestEnvelope,
    cfg: &RunConfig,
    parent: &ParentRef,
    cwd: &Path,
    account: Option<super::super::account::Active>,
    prompt: &str,
    handle: &Arc<RunHandle>,
) {
    let req_id = &env.request_id;
    let open = open_or_launch_codex(
        coordinator,
        terminals,
        env,
        cfg,
        parent,
        cwd,
        account,
        handle,
    );
    let (launched, reused) = match open {
        Ok(pair) => pair,
        Err(err) => {
            let status = if handle.is_pre_start_cancelled() {
                "cancelled"
            } else {
                "failed"
            };
            coordinator.mark_finished(req_id, status, Some(&err), env.session_id.as_deref());
            return;
        }
    };
    let Some(codex) = launched.codex.clone() else {
        cleanup_derived_startup(terminals, &launched, reused, launched.native.as_deref());
        coordinator.mark_finished(req_id, "failed", Some("缺少 Codex app-server 客户端"), None);
        return;
    };

    if !wait_codex_connected(&codex, &launched.session, handle) {
        cleanup_derived_startup(terminals, &launched, reused, launched.native.as_deref());
        let status = if handle.is_pre_start_cancelled() {
            "cancelled"
        } else {
            "failed"
        };
        coordinator.mark_finished(
            req_id,
            status,
            Some(&startup_failure_reason(
                &launched.session,
                "等待 Codex app-server 连接失败",
            )),
            launched.native.as_deref(),
        );
        return;
    }

    if let Some(ready_file) = terminals.take_codex_ready_file(launched.id) {
        let _guard = ReadyFileGuard(Some(ready_file));
        if let Err(err) = codex.write_project_trust(cwd) {
            eprintln!("写入隔离目录 Codex 信任失败：{err}");
        }
    }

    if handle.is_pre_start_cancelled() {
        cleanup_derived_startup(terminals, &launched, reused, launched.native.as_deref());
        coordinator.mark_finished(
            req_id,
            "cancelled",
            Some("启动前已取消"),
            launched.native.as_deref(),
        );
        return;
    }

    let Some(thread_id) = wait_codex_thread(
        coordinator,
        terminals,
        &launched,
        &codex,
        env.session_id.as_deref(),
        req_id,
        handle,
    ) else {
        cleanup_derived_startup(terminals, &launched, reused, launched.native.as_deref());
        let status = if handle.is_pre_start_cancelled() {
            "cancelled"
        } else {
            "failed"
        };
        coordinator.mark_finished(
            req_id,
            status,
            Some(&startup_failure_reason(
                &launched.session,
                "等待 Codex 线程就绪失败",
            )),
            launched.native.as_deref(),
        );
        return;
    };

    terminals.set_derived_blocker(launched.id, Some(&thread_id), None);
    coordinator.set_session_id(req_id, &thread_id);

    if handle.is_pre_start_cancelled() {
        cleanup_derived_startup(terminals, &launched, reused, Some(&thread_id));
        coordinator.mark_finished(req_id, "cancelled", Some("启动前已取消"), Some(&thread_id));
        return;
    }

    let turn_cfg = build_turn_config(prompt, &env.invocation_id, cwd, cfg);
    let turn_id = match codex.start_turn(&thread_id, &turn_cfg) {
        Ok(id) => id,
        Err(err) => {
            cleanup_derived_startup(terminals, &launched, reused, Some(&thread_id));
            coordinator.mark_finished(req_id, "failed", Some(&err), Some(&thread_id));
            return;
        }
    };

    coordinator.mark_started(
        req_id,
        Some(&thread_id),
        Some(json!({
            "session_id": thread_id,
            "turn_id": turn_id,
            "client_user_message_id": env.invocation_id,
        })),
    );

    monitor_codex_turn(
        coordinator,
        terminals,
        &launched,
        &codex,
        req_id,
        &thread_id,
        &turn_id,
    );
}

#[allow(clippy::too_many_arguments)]
fn open_or_launch_codex(
    coordinator: &Coordinator,
    terminals: &Terminals,
    env: &SpawnRequestEnvelope,
    cfg: &RunConfig,
    parent: &ParentRef,
    cwd: &Path,
    account: Option<super::super::account::Active>,
    handle: &Arc<RunHandle>,
) -> Result<(DerivedHandle, bool), String> {
    if handle.is_pre_start_cancelled() {
        return Err("启动前已取消".to_owned());
    }
    if account.is_some() {
        return Err("Codex 不支持多账号".to_owned());
    }
    if let Some(sid) = env.session_id.as_deref() {
        terminals.remove_exited_derived("codex", sid);
        if let Some(existing) = terminals.find_derived_session("codex", sid) {
            if existing.session.has_draft()
                || matches!(
                    existing.session.osc_snapshot().1.as_deref(),
                    Some("working" | "blocked")
                )
                || existing.codex.as_ref().is_some_and(|c| {
                    c.thread_status(sid)
                        .is_some_and(|s| matches!(s, ThreadStatus::Active { .. }))
                })
            {
                return Err("Codex 派生终端正在运行中".to_owned());
            }
            let (iso_id, lease) = acquire_isolation_for_terminal(
                coordinator,
                terminals,
                &env.request_id,
                cwd,
                cfg.isolation.as_ref(),
                true,
            )?;
            if handle.is_pre_start_cancelled() {
                return Err("启动前已取消".to_owned());
            }
            if let Some(lease) = lease {
                terminals.attach_derived_lease(existing.id, iso_id.as_deref(), lease);
            }
            let claimed =
                terminals.try_claim_derived_turn(existing.id, &env.invocation_id, None)?;
            return Ok((claimed, true));
        }
    }

    let (isolation_id, lease) = acquire_isolation_for_terminal(
        coordinator,
        terminals,
        &env.request_id,
        cwd,
        cfg.isolation.as_ref(),
        false,
    )?;
    if handle.is_pre_start_cancelled() {
        return Err("启动前已取消".to_owned());
    }
    let Some(program) = installed_path("codex") else {
        return Err("Codex CLI 没有安装".to_owned());
    };
    let terminal_id = terminals.reserve_id();
    let sock = codex_server::socket_path(terminal_id);
    let ready_file = cfg
        .isolation
        .as_ref()
        .map(|_| std::env::temp_dir().join(format!("agentnave-codex-ready-{terminal_id}.flag")));
    if let Some(path) = &ready_file {
        let _ = fs::remove_file(path);
    }
    let (launch_program, launch_args) =
        codex_server::launch_args(&program, &sock, cwd, env.session_id.as_deref(), &[]);
    let extra_env = ready_file
        .as_ref()
        .map(|p| {
            vec![(
                "AGENTNAVE_CODEX_READY_FILE".to_owned(),
                p.display().to_string(),
            )]
        })
        .unwrap_or_default();
    let session = Session::spawn(&super::super::session::Launch {
        program: launch_program,
        args: launch_args,
        env: extra_env,
        cwd: cwd.to_path_buf(),
        cols: 100,
        rows: 30,
        derived: true,
    })?;
    let client = codex_server::Client::connect_in_background(sock, session.clone());
    terminals.insert_reserved_derived(
        terminal_id,
        NewDerived {
            provider: "codex".to_owned(),
            cwd: cwd.to_path_buf(),
            session: session.clone(),
            native: env.session_id.clone(),
            account: None,
            codex: Some(client.clone()),
            info: DerivedInfo {
                parent_boss_key: parent.boss_key.clone(),
                parent_workspace_id: parent.workspace_id.clone(),
                busy: true,
                blocked_kind: None,
                isolation_id,
                _lease: lease,
                _log_dir: None,
                codex_ready_file: ready_file,
                active_invocation_id: Some(env.invocation_id.clone()),
            },
        },
    );
    Ok((
        DerivedHandle {
            id: terminal_id,
            provider: "codex".to_owned(),
            session,
            native: env.session_id.clone(),
            account: None,
            codex: Some(client),
        },
        false,
    ))
}

fn wait_codex_connected(
    codex: &codex_server::Client,
    session: &Session,
    handle: &Arc<RunHandle>,
) -> bool {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    while Instant::now() < deadline {
        if codex.is_connected() {
            return true;
        }
        if handle.is_pre_start_cancelled() || session.exit().is_some() {
            return false;
        }
        thread::sleep(POLL_STEP);
    }
    codex.is_connected()
}

fn wait_codex_thread(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    codex: &codex_server::Client,
    expected_tid: Option<&str>,
    req_id: &str,
    handle: &Arc<RunHandle>,
) -> Option<String> {
    let mut deadline = Instant::now() + THREAD_WAIT_TIMEOUT;
    let mut trust_blocked = false;
    while Instant::now() < deadline {
        if handle.is_pre_start_cancelled() || launched.session.exit().is_some() {
            return None;
        }
        let threads = codex.loaded_threads();
        let found = match expected_tid {
            Some(tid) => threads.iter().find(|t| t.as_str() == tid).cloned(),
            None => threads.into_iter().next(),
        };
        if let Some(tid) = found {
            if trust_blocked {
                terminals.set_derived_blocker(launched.id, Some(&tid), None);
                coordinator.set_blocked(req_id, None);
            }
            return Some(tid);
        }
        if let Some((kind, reason)) = detect_screen_blocker(&launched.session) {
            if !trust_blocked {
                trust_blocked = true;
                terminals.set_derived_blocker(launched.id, launched.native.as_deref(), Some(kind));
                coordinator.set_blocked(
                    req_id,
                    Some(BlockedReceipt {
                        kind: kind.to_owned(),
                        since: now_unix_secs(),
                        reason: reason.to_owned(),
                    }),
                );
            }
            deadline = Instant::now() + THREAD_WAIT_TIMEOUT;
        }
        thread::sleep(POLL_STEP);
    }
    None
}

fn build_turn_config(
    prompt: &str,
    client_id: &str,
    cwd: &Path,
    cfg: &RunConfig,
) -> TurnStartConfig {
    let perms = &cfg.permissions;
    let mut approval_policy = None;
    let mut approvals_reviewer = None;
    let mut sandbox_policy = None;
    let cwd_str = cwd.display().to_string();

    if perms
        .get("dangerously_bypass_approvals_and_sandbox")
        .and_then(Value::as_bool)
        == Some(true)
    {
        approval_policy = Some("never".to_owned());
        sandbox_policy = Some(json!({"type": "dangerFullAccess"}));
    } else if perms.get("approve_for_me").and_then(Value::as_bool) == Some(true) {
        approval_policy = Some("on-request".to_owned());
        approvals_reviewer = Some("guardian_subagent".to_owned());
        sandbox_policy = Some(json!({
            "type": "workspaceWrite",
            "writableRoots": [cwd_str],
            "networkAccess": true,
        }));
    } else if let Some(sandbox) = perms.get("sandbox").and_then(Value::as_str) {
        sandbox_policy = match sandbox {
            "read-only" => Some(json!({"type": "readOnly"})),
            "workspace-write" => Some(json!({
                "type": "workspaceWrite",
                "writableRoots": [cwd_str],
                "networkAccess": true,
            })),
            "danger-full-access" => Some(json!({"type": "dangerFullAccess"})),
            _ => None,
        };
    }

    TurnStartConfig {
        prompt: prompt.to_owned(),
        client_id: client_id.to_owned(),
        model: cfg.model.clone().filter(|s| !s.is_empty()),
        effort: cfg.effort.clone().filter(|s| !s.is_empty()),
        approval_policy,
        approvals_reviewer,
        sandbox_policy,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CodexTurnOutcome {
    Running,
    Completed,
    Interrupted,
    Failed,
}

pub(super) fn check_turn_reconciled(
    codex: &codex_server::Client,
    thread_id: &str,
    turn_id: &str,
    rollout_path: &mut Option<PathBuf>,
    codex_homes: &[PathBuf],
) -> (CodexTurnOutcome, Option<String>) {
    if let Some(ev) = codex.turn_event(thread_id, turn_id)
        && let Some(status) = ev.completed_status.as_deref()
    {
        return map_codex_turn_status(status, ev.error);
    }
    if let Ok((path_opt, turns)) = codex.read_thread(thread_id) {
        if rollout_path.is_none() {
            *rollout_path = path_opt;
        }
        if let Some(info) = turns.iter().find(|t| t.id == turn_id) {
            // `thread/read` also reports a turn that is still running in the rollout as
            // `interrupted` whenever the thread is not active (right after `turn/start`, or on a
            // restarted app-server), so only its final states count; interruption is proven by the
            // live notification or the rollout's `turn_aborted`.
            let res = map_codex_turn_status(&info.status, info.error.clone());
            if matches!(
                res.0,
                CodexTurnOutcome::Completed | CodexTurnOutcome::Failed
            ) {
                return res;
            }
        }
    }
    if rollout_path.is_none() {
        *rollout_path = native_logs::codex_rollout_path_for_thread(codex_homes, thread_id);
    }
    if let Some(path) = rollout_path.as_deref() {
        match native_logs::codex_rollout_turn_state(path, turn_id) {
            native_logs::CodexRolloutTurn::Completed => return (CodexTurnOutcome::Completed, None),
            native_logs::CodexRolloutTurn::Interrupted => {
                return (CodexTurnOutcome::Interrupted, None);
            }
            native_logs::CodexRolloutTurn::Failed(err) => return (CodexTurnOutcome::Failed, err),
            native_logs::CodexRolloutTurn::Started | native_logs::CodexRolloutTurn::Unknown => {}
        }
    }
    (CodexTurnOutcome::Running, None)
}

fn map_codex_turn_status(
    status: &str,
    error: Option<String>,
) -> (CodexTurnOutcome, Option<String>) {
    match status {
        "completed" => (CodexTurnOutcome::Completed, None),
        "interrupted" => (CodexTurnOutcome::Interrupted, None),
        "failed" => (CodexTurnOutcome::Failed, error),
        _ => (CodexTurnOutcome::Running, None),
    }
}

#[allow(clippy::too_many_arguments)]
fn monitor_codex_turn(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    codex: &codex_server::Client,
    req_id: &str,
    thread_id: &str,
    turn_id: &str,
) {
    let homes = native_logs::codex_homes();
    let mut rollout_path = None;
    let mut resumed = false;
    let mut last_resume_try = Instant::now() - RESUME_RETRY_GAP;
    let mut waiting_blocked = false;

    loop {
        if !resumed && last_resume_try.elapsed() >= RESUME_RETRY_GAP {
            last_resume_try = Instant::now();
            if codex.resume_thread(thread_id).is_ok() {
                resumed = true;
            }
        }

        let approval_blocked = codex
            .thread_status(thread_id)
            .is_some_and(|s| s.is_waiting_on_approval())
            || launched.session.osc_snapshot().1.as_deref() == Some("blocked");
        if approval_blocked && !waiting_blocked {
            waiting_blocked = true;
            terminals.set_derived_blocker(launched.id, Some(thread_id), Some("permission"));
            coordinator.set_blocked(
                req_id,
                Some(BlockedReceipt {
                    kind: "permission".to_owned(),
                    since: now_unix_secs(),
                    reason: "Codex 等待权限或输入确认".to_owned(),
                }),
            );
        } else if !approval_blocked && waiting_blocked {
            waiting_blocked = false;
            terminals.set_derived_blocker(launched.id, Some(thread_id), None);
            coordinator.set_blocked(req_id, None);
        }

        let (outcome, err_msg) =
            check_turn_reconciled(codex, thread_id, turn_id, &mut rollout_path, &homes);
        let mut exit = None;
        if outcome == CodexTurnOutcome::Running {
            let Some(code) = launched.session.exit() else {
                thread::sleep(POLL_STEP);
                continue;
            };
            exit = Some(code);
        }
        let (outcome, err_msg) = match exit {
            Some(_) => {
                // The terminal is gone: look once more, since the app-server may have just
                // written the turn's end.
                thread::sleep(Duration::from_millis(120));
                check_turn_reconciled(codex, thread_id, turn_id, &mut rollout_path, &homes)
            }
            None => (outcome, err_msg),
        };
        terminals.clear_derived_turn(launched.id, Some(thread_id));
        if exit.is_some() {
            terminals.release_derived_lease(launched.id);
        }
        let (status, reason) = settle_turn(outcome, err_msg, exit);
        coordinator.mark_finished(req_id, status, reason.as_deref(), Some(thread_id));
        return;
    }
}

/// How a run ends: a finished turn succeeds, a proven interruption is a cancel, and a terminal
/// that exited without either proof is a failure, never a cancel.
pub(super) fn settle_turn(
    outcome: CodexTurnOutcome,
    error: Option<String>,
    exit: Option<u32>,
) -> (&'static str, Option<String>) {
    match outcome {
        CodexTurnOutcome::Completed => ("succeeded", None),
        CodexTurnOutcome::Interrupted => ("cancelled", Some("Codex 当前轮已中断".to_owned())),
        CodexTurnOutcome::Failed => (
            "failed",
            Some(error.unwrap_or_else(|| "Codex 执行失败".to_owned())),
        ),
        CodexTurnOutcome::Running => ("failed", Some(early_exit_reason(exit.unwrap_or_default()))),
    }
}

fn startup_failure_reason(session: &Session, fallback: &str) -> String {
    session
        .exit()
        .map_or_else(|| fallback.to_owned(), early_exit_reason)
}

fn early_exit_reason(code: u32) -> String {
    format!("Codex 终端提前退出（exit {code}）")
}

pub(super) fn cancel_codex(
    coordinator: &Coordinator,
    env: &SpawnRequestEnvelope,
    run_receipt: &Receipt,
    handle: &DerivedHandle,
) {
    let req_id = &env.request_id;
    let sid = run_receipt.session_id.as_deref();
    let turn_id = run_receipt
        .anchor
        .as_ref()
        .and_then(|a| a["turn_id"].as_str());
    let (Some(tid), Some(turn_id), Some(codex)) = (sid, turn_id, handle.codex.as_ref()) else {
        coordinator.mark_finished(
            req_id,
            "not_safe_to_interrupt",
            Some("缺少 Codex turn 锚点"),
            sid,
        );
        return;
    };

    let homes = native_logs::codex_homes();
    let mut rollout_path = None;
    let deadline = Instant::now() + INTERRUPT_RETRY_TIMEOUT;

    loop {
        if let Ok((path_opt, turns)) = codex.read_thread(tid) {
            if rollout_path.is_none() {
                rollout_path = path_opt;
            }
            let has_later_turn = turns
                .iter()
                .position(|t| t.id == turn_id)
                .is_some_and(|idx| idx + 1 < turns.len());
            if has_later_turn {
                coordinator.mark_finished(
                    req_id,
                    "not_safe_to_interrupt",
                    Some("会话已进入后续轮次"),
                    Some(tid),
                );
                return;
            }
        }

        let (outcome, _) = check_turn_reconciled(codex, tid, turn_id, &mut rollout_path, &homes);
        match outcome {
            CodexTurnOutcome::Completed | CodexTurnOutcome::Failed => {
                coordinator.mark_finished(req_id, "already_finished", None, Some(tid));
                return;
            }
            CodexTurnOutcome::Interrupted => {
                coordinator.mark_finished(req_id, "interrupted", None, Some(tid));
                return;
            }
            CodexTurnOutcome::Running => {}
        }

        match codex.interrupt_turn(tid, turn_id) {
            Ok(InterruptRpcOutcome::Issued) => {
                wait_codex_interrupt_confirmed(
                    coordinator,
                    req_id,
                    codex,
                    tid,
                    turn_id,
                    &mut rollout_path,
                    &homes,
                );
                return;
            }
            Ok(InterruptRpcOutcome::NoActiveTurn) => {
                if Instant::now() >= deadline {
                    coordinator.mark_finished(
                        req_id,
                        "already_finished",
                        Some("当前无活跃轮次可中断"),
                        Some(tid),
                    );
                    return;
                }
                thread::sleep(INTERRUPT_RETRY_GAP);
            }
            Err(err) => {
                coordinator.mark_finished(
                    req_id,
                    "not_safe_to_interrupt",
                    Some(&truncate_short(&err)),
                    Some(tid),
                );
                return;
            }
        }
    }
}

fn wait_codex_interrupt_confirmed(
    coordinator: &Coordinator,
    req_id: &str,
    codex: &codex_server::Client,
    tid: &str,
    turn_id: &str,
    rollout_path: &mut Option<PathBuf>,
    homes: &[PathBuf],
) {
    let confirm_deadline = Instant::now() + INTERRUPT_CONFIRM_TIMEOUT;
    loop {
        let (outcome, _) = check_turn_reconciled(codex, tid, turn_id, rollout_path, homes);
        match outcome {
            CodexTurnOutcome::Interrupted => {
                coordinator.mark_finished(req_id, "interrupted", None, Some(tid));
                return;
            }
            CodexTurnOutcome::Completed | CodexTurnOutcome::Failed => {
                coordinator.mark_finished(req_id, "already_finished", None, Some(tid));
                return;
            }
            CodexTurnOutcome::Running => {}
        }
        if Instant::now() >= confirm_deadline {
            coordinator.mark_finished(
                req_id,
                "not_safe_to_interrupt",
                Some("等待 Codex 中断确认超时"),
                Some(tid),
            );
            return;
        }
        thread::sleep(POLL_STEP);
    }
}

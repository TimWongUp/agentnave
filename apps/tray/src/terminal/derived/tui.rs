//! Claude & Antigravity interactive TUI derived launch, readiness gating, and turn monitors.

mod agy;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::account::{self, Active};
use super::super::session::{self, Launch, Session};
use super::super::spawn_requests::{
    Coordinator, ParentRef, RunConfig, RunHandle, SpawnRequestEnvelope,
};
use super::super::{
    DerivedHandle, DerivedInfo, DerivedLaunchConfig, NewDerived, Terminals, installed_path,
};
use super::{
    POLL_STEP, claude, claude_hook, cleanup_derived_startup, detect_screen_blocker, native_logs,
    prepare_open_or_launch, update_blocker_if_changed,
};
#[cfg(all(test, unix))]
pub(super) use agy::{AgyMonitorConfig, monitor_agy_turn_with_config};
use agy::{highest_agy_user_index, monitor_agy_turn};

const READY_TIMEOUT: Duration = Duration::from_secs(45);
const PASTE_MODE_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(600)
} else {
    Duration::from_secs(3)
};
const PASTE_COMMIT_DELAY: Duration = Duration::from_millis(300);
pub(super) const MISSING_ANCHOR_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(600)
} else {
    Duration::from_secs(10)
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiKind {
    Claude,
    Antigravity,
}

pub(super) struct TurnBaseline {
    pub(super) claude_homes: Vec<PathBuf>,
    pub(super) pre_offset: u64,
    pub(super) hook_events_offset: u64,
    pub(super) agy_baseline_cids: Vec<String>,
    pub(super) agy_prev_idx: Option<i64>,
    pub(super) seq_before: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_tui_cli(
    coordinator: &Coordinator,
    terminals: &Terminals,
    env: &SpawnRequestEnvelope,
    cfg: &RunConfig,
    parent: &ParentRef,
    cwd: &Path,
    account: Option<Active>,
    prompt: &str,
    handle: &Arc<RunHandle>,
    kind: TuiKind,
) {
    if kind == TuiKind::Claude {
        claude::run_claude_cli(
            coordinator,
            terminals,
            env,
            cfg,
            parent,
            cwd,
            account,
            prompt,
            handle,
        );
        return;
    }
    let req_id = &env.request_id;
    let preset_sid = env.session_id.clone();
    let mut agy_log_path: Option<PathBuf> = None;
    let open = prepare_open_or_launch(
        coordinator,
        terminals,
        env,
        cfg,
        cwd,
        account.as_ref(),
        handle,
        |lease, isolation_id| {
            let (launched, log_path) = spawn_tui_terminal(
                coordinator,
                terminals,
                env,
                cfg,
                cfg,
                parent,
                cwd,
                account.clone(),
                preset_sid.clone(),
                TuiKind::Antigravity,
                lease,
                isolation_id,
                None,
            )?;
            agy_log_path = log_path;
            Ok(launched)
        },
    );
    let (launched, reused) = match open {
        Ok(pair) => pair,
        Err(err) => {
            let status = if handle.is_pre_start_cancelled() {
                "cancelled"
            } else {
                "failed"
            };
            coordinator.mark_finished(req_id, status, Some(&err), preset_sid.as_deref());
            return;
        }
    };
    if let Some(sid) = &preset_sid {
        coordinator.set_session_id(req_id, sid);
    }
    if handle.is_pre_start_cancelled()
        || !wait_tui_ready(coordinator, terminals, &launched, req_id, handle, None)
    {
        finish_pre_ready(coordinator, terminals, &launched, reused, req_id, handle);
        return;
    }

    let baseline = capture_baseline(
        TuiKind::Antigravity,
        account.as_ref(),
        preset_sid.as_deref(),
        env.session_id.as_deref(),
        agy_log_path.as_deref(),
        None,
        &launched.session,
    );
    let Some(sent_prompt) = submit_prompt(
        coordinator,
        terminals,
        &launched,
        reused,
        req_id,
        prompt,
        handle,
    ) else {
        return;
    };
    monitor_agy_turn(
        coordinator,
        terminals,
        &launched,
        req_id,
        env.session_id.as_deref(),
        agy_log_path.as_deref(),
        &baseline,
        &sent_prompt,
        handle,
    );
}

pub(super) fn finish_pre_ready(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    reused: bool,
    req_id: &str,
    handle: &Arc<RunHandle>,
) {
    finish_pre_submit(
        coordinator,
        terminals,
        launched,
        reused,
        req_id,
        if handle.is_pre_start_cancelled() {
            "cancelled"
        } else {
            "failed"
        },
        if handle.is_pre_start_cancelled() {
            "就绪前已取消"
        } else {
            "等待终端就绪失败或超时"
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_tui_terminal(
    coordinator: &Coordinator,
    terminals: &Terminals,
    env: &SpawnRequestEnvelope,
    cfg: &RunConfig,
    recorded_cfg: &RunConfig,
    parent: &ParentRef,
    cwd: &Path,
    account: Option<Active>,
    preset_sid: Option<String>,
    kind: TuiKind,
    lease: Option<crate::isolated::leases::HeldLease>,
    isolation_id: Option<String>,
    hook_dir: Option<tempfile::TempDir>,
) -> Result<(DerivedHandle, Option<PathBuf>), String> {
    let hook_dir_path = hook_dir.as_ref().map(|d| d.path().to_path_buf());
    let TuiProcess {
        session,
        log_dir,
        log_path,
    } = launch_tui(
        cfg,
        env.session_id.as_deref(),
        preset_sid.as_deref(),
        cwd,
        account.as_ref(),
        kind,
        hook_dir_path.as_deref(),
    )?;
    let launch_config = Some(DerivedLaunchConfig::from_run_config(recorded_cfg));
    let tid = terminals.register_derived(NewDerived {
        provider: cfg.provider.clone(),
        cwd: cwd.to_path_buf(),
        session: session.clone(),
        native: preset_sid.clone(),
        account: account.clone(),
        #[cfg(unix)]
        codex: None,
        info: DerivedInfo {
            parent_boss_key: parent.boss_key.clone(),
            parent_workspace_id: parent.workspace_id.clone(),
            busy: true,
            blocked_kind: None,
            isolation_id,
            _lease: lease,
            _log_dir: log_dir,
            _hook_dir: hook_dir,
            codex_ready_file: None,
            life: Default::default(),
            active_invocation_id: Some(env.invocation_id.clone()),
            launch_config: launch_config.clone(),
        },
    });
    let _ = coordinator.sync_derived_pids(terminals);
    Ok((
        DerivedHandle {
            id: tid,
            provider: cfg.provider.clone(),
            #[cfg(unix)]
            cwd: cwd.to_path_buf(),
            session,
            native: preset_sid,
            account,
            launch_config,
            hook_dir: hook_dir_path,
            #[cfg(unix)]
            codex: None,
        },
        log_path,
    ))
}

pub(super) struct TuiProcess {
    pub(super) session: Arc<Session>,
    pub(super) log_dir: Option<tempfile::TempDir>,
    pub(super) log_path: Option<PathBuf>,
}

/// Starts the CLI on the session it resumes, or (Claude) on a new one under the ID `fresh`.
pub(super) fn launch_tui(
    cfg: &RunConfig,
    resume: Option<&str>,
    fresh: Option<&str>,
    cwd: &Path,
    account: Option<&Active>,
    kind: TuiKind,
    hook_dir: Option<&Path>,
) -> Result<TuiProcess, String> {
    let Some(program) = installed_path(&cfg.provider) else {
        return Err("这个 CLI 没有安装".to_owned());
    };
    let (args, mut env, log_dir, log_path) = match kind {
        TuiKind::Claude => {
            let fresh = resume.is_none().then_some(fresh.unwrap_or_default());
            let args = claude::build_claude_args(cfg, resume, fresh, hook_dir);
            (args, Vec::new(), None, None)
        }
        TuiKind::Antigravity => {
            let dir = tempfile::Builder::new()
                .prefix("agentnave-agy-")
                .tempdir()
                .map_err(|e| format!("无法创建 agy 日志目录：{e}"))?;
            let log_path = dir.path().join("agy.log");
            let args = build_agy_args(cfg, resume, cwd, &log_path);
            let env = vec![
                ("AGY_CLI_OSC7501".to_owned(), "on".to_owned()),
                ("AGY_CLI_OSC7501_MESSAGES".to_owned(), "brief".to_owned()),
            ];
            (args, env, Some(dir), Some(log_path))
        }
    };
    let (program, args) = match account {
        Some(acc) => {
            let (program, args, more) = account::launch_as(acc, program, args, true);
            env.extend(more);
            (program, args)
        }
        None => (program, args),
    };
    let session = Session::spawn(&Launch {
        program,
        args,
        env,
        cwd: cwd.to_path_buf(),
        cols: 100,
        rows: 30,
        derived: true,
    })?;
    Ok(TuiProcess {
        session,
        log_dir,
        log_path,
    })
}

pub(super) fn capture_baseline(
    kind: TuiKind,
    account: Option<&Active>,
    preset_sid: Option<&str>,
    requested_sid: Option<&str>,
    agy_log_path: Option<&Path>,
    hook_dir: Option<&Path>,
    session: &Session,
) -> TurnBaseline {
    let account_dir = account.map(|a| a.dir.as_path());
    let claude_homes = native_logs::claude_homes(account_dir);
    let pre_offset = match (kind, preset_sid) {
        (TuiKind::Claude, Some(sid)) => native_logs::claude_transcript_path(&claude_homes, sid)
            .map_or(0, |p| native_logs::file_len(&p)),
        _ => 0,
    };
    let hook_events_offset = hook_dir
        .map(|dir| native_logs::file_len(&claude_hook::events_path(dir)))
        .unwrap_or(0);
    let agy_baseline_cids = if kind == TuiKind::Antigravity && requested_sid.is_none() {
        agy_log_path
            .map(native_logs::agy_log_conversation_ids)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let agy_prev_idx = match (kind, requested_sid, native_logs::agy_home()) {
        (TuiKind::Antigravity, Some(cid), Some(home)) => {
            highest_agy_user_index(&native_logs::agy_transcript_path(&home, cid))
        }
        _ => None,
    };
    let (seq_before, _, _) = session.osc_snapshot();
    TurnBaseline {
        claude_homes,
        pre_offset,
        hook_events_offset,
        agy_baseline_cids,
        agy_prev_idx,
        seq_before,
    }
}

pub(super) fn finish_pre_submit(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    reused: bool,
    req_id: &str,
    status: &str,
    reason: &str,
) {
    cleanup_derived_startup(terminals, launched, reused, launched.native.as_deref());
    coordinator.mark_finished(req_id, status, Some(reason), launched.native.as_deref());
}

fn wait_bracketed_paste(session: &Session, handle: &Arc<RunHandle>) {
    let deadline = Instant::now() + PASTE_MODE_WAIT;
    while Instant::now() < deadline {
        if session.bracketed_paste_enabled()
            || handle.is_pre_start_cancelled()
            || session.exit().is_some()
        {
            return;
        }
        thread::sleep(POLL_STEP);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn submit_prompt(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    reused: bool,
    req_id: &str,
    prompt: &str,
    handle: &Arc<RunHandle>,
) -> Option<String> {
    if handle.is_pre_start_cancelled() {
        finish_pre_submit(
            coordinator,
            terminals,
            launched,
            reused,
            req_id,
            "cancelled",
            "提交前已取消",
        );
        return None;
    }
    if session::sanitize_paste_text(prompt).contains('\n')
        && !launched.session.bracketed_paste_enabled()
    {
        wait_bracketed_paste(&launched.session, handle);
        if handle.is_pre_start_cancelled() || launched.session.exit().is_some() {
            let status = if handle.is_pre_start_cancelled() {
                "cancelled"
            } else {
                "closed"
            };
            finish_pre_submit(
                coordinator,
                terminals,
                launched,
                reused,
                req_id,
                status,
                "提交前取消或关闭",
            );
            return None;
        }
    }
    let sent = match launched.session.send_paste(prompt) {
        Ok(clean) => clean,
        Err(err) => {
            finish_pre_submit(
                coordinator,
                terminals,
                launched,
                reused,
                req_id,
                "failed",
                &err,
            );
            return None;
        }
    };
    thread::sleep(PASTE_COMMIT_DELAY);
    if handle.is_pre_start_cancelled() || launched.session.exit().is_some() {
        let status = if handle.is_pre_start_cancelled() {
            "cancelled"
        } else {
            "closed"
        };
        finish_pre_submit(
            coordinator,
            terminals,
            launched,
            reused,
            req_id,
            status,
            "提交前取消或关闭",
        );
        return None;
    }
    if launched.session.osc_snapshot().1.as_deref() == Some("blocked")
        && !wait_unblocked(coordinator, terminals, launched, req_id, handle)
    {
        finish_pre_submit(
            coordinator,
            terminals,
            launched,
            reused,
            req_id,
            "cancelled",
            "阻塞期间取消",
        );
        return None;
    }
    if handle.is_pre_start_cancelled() {
        finish_pre_submit(
            coordinator,
            terminals,
            launched,
            reused,
            req_id,
            "cancelled",
            "提交前已取消",
        );
        return None;
    }
    if let Err(err) = launched.session.write(b"\r") {
        finish_pre_submit(
            coordinator,
            terminals,
            launched,
            reused,
            req_id,
            "failed",
            &err,
        );
        return None;
    }
    Some(sent)
}

pub(super) fn build_agy_args(
    cfg: &RunConfig,
    resume: Option<&str>,
    cwd: &Path,
    log_path: &Path,
) -> Vec<String> {
    let mut args = session::cli_args("antigravity", resume, None);
    args.push("--log-file".to_owned());
    args.push(log_path.display().to_string());
    let canonical = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut trusted = vec![canonical.display().to_string()];
    let raw = cwd.display().to_string();
    if raw != trusted[0] {
        trusted.push(raw);
    }
    trusted.sort();
    args.push("--settings".to_owned());
    args.push(json!({"trustedWorkspaces": trusted}).to_string());
    if let Some(model) = cfg.model.as_deref().filter(|s| !s.is_empty()) {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    if let Some(effort) = cfg.effort.as_deref().filter(|s| !s.is_empty()) {
        args.push("--effort".to_owned());
        args.push(effort.to_owned());
    }
    if let Some(agent) = cfg.permissions.get("agent").and_then(Value::as_str) {
        args.push("--agent".to_owned());
        args.push(agent.to_owned());
    }
    if let Some(mode) = cfg.permissions.get("mode").and_then(Value::as_str) {
        args.push("--mode".to_owned());
        args.push(mode.to_owned());
    }
    if let Some(sandbox) = cfg.permissions.get("sandbox").and_then(Value::as_bool) {
        args.push(format!("--sandbox={sandbox}"));
    }
    if let Some(skip) = cfg
        .permissions
        .get("dangerously_skip_permissions")
        .and_then(Value::as_bool)
    {
        args.push(format!("--dangerously-skip-permissions={skip}"));
    }
    args
}

pub(super) fn wait_tui_ready(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    handle: &Arc<RunHandle>,
    trust_cwd: Option<&Path>,
) -> bool {
    let mut blocker: Option<(String, f64)> = None;
    let mut deadline = Instant::now() + READY_TIMEOUT;
    let mut claude_startup = trust_cwd.map(claude::ClaudeStartup::new);
    while Instant::now() < deadline {
        if handle.is_pre_start_cancelled() || launched.session.exit().is_some() {
            return false;
        }
        let (_, osc_state, osc_kind) = launched.session.osc_snapshot();
        if matches!(osc_state.as_deref(), Some("idle" | "done")) {
            update_blocker_if_changed(
                coordinator,
                terminals,
                launched.id,
                req_id,
                launched.native.as_deref(),
                &mut blocker,
                None,
            );
            return true;
        }
        if let Some(startup) = claude_startup.as_mut() {
            match startup.step(&launched.session, osc_state.as_deref(), osc_kind.as_deref()) {
                claude::StartupStep::Wait => {}
                claude::StartupStep::SentKey => {
                    deadline = Instant::now() + READY_TIMEOUT;
                    update_blocker_if_changed(
                        coordinator,
                        terminals,
                        launched.id,
                        req_id,
                        launched.native.as_deref(),
                        &mut blocker,
                        None,
                    );
                }
                claude::StartupStep::Blocker(next) => {
                    deadline = Instant::now() + READY_TIMEOUT;
                    update_blocker_if_changed(
                        coordinator,
                        terminals,
                        launched.id,
                        req_id,
                        launched.native.as_deref(),
                        &mut blocker,
                        Some(next),
                    );
                }
                claude::StartupStep::WriteFailed => return false,
            }
        } else {
            let next = if osc_state.as_deref() == Some("blocked") {
                Some((
                    osc_kind.as_deref().unwrap_or("permission"),
                    "终端启动等待用户确认",
                ))
            } else {
                detect_screen_blocker(&launched.session)
            };
            if next.is_some() {
                deadline = Instant::now() + READY_TIMEOUT;
            }
            update_blocker_if_changed(
                coordinator,
                terminals,
                launched.id,
                req_id,
                launched.native.as_deref(),
                &mut blocker,
                next,
            );
        }
        thread::sleep(POLL_STEP);
    }
    false
}

fn wait_unblocked(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    handle: &Arc<RunHandle>,
) -> bool {
    let mut blocker: Option<(String, f64)> = None;
    while !handle.is_pre_start_cancelled() && launched.session.exit().is_none() {
        let (_, osc_state, osc_kind) = launched.session.osc_snapshot();
        if osc_state.as_deref() != Some("blocked") {
            update_blocker_if_changed(
                coordinator,
                terminals,
                launched.id,
                req_id,
                launched.native.as_deref(),
                &mut blocker,
                None,
            );
            return true;
        }
        update_blocker_if_changed(
            coordinator,
            terminals,
            launched.id,
            req_id,
            launched.native.as_deref(),
            &mut blocker,
            Some((osc_kind.as_deref().unwrap_or("permission"), "等待用户确认")),
        );
        thread::sleep(POLL_STEP);
    }
    false
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_turn_blocker(
    coordinator: &Coordinator,
    terminals: &Terminals,
    terminal_id: u64,
    req_id: &str,
    native: Option<&str>,
    osc_state: Option<&str>,
    osc_kind: Option<&str>,
    blocker: &mut Option<(String, f64)>,
    settled_at: &mut Option<Instant>,
) {
    if osc_state == Some("blocked") {
        let kind = osc_kind.unwrap_or("permission");
        update_blocker_if_changed(
            coordinator,
            terminals,
            terminal_id,
            req_id,
            native,
            blocker,
            Some((kind, "等待用户确认")),
        );
        *settled_at = None;
    } else {
        update_blocker_if_changed(
            coordinator,
            terminals,
            terminal_id,
            req_id,
            native,
            blocker,
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_settled(
    coordinator: &Coordinator,
    terminals: &Terminals,
    terminal_id: u64,
    req_id: &str,
    sid: Option<&str>,
    is_error: bool,
    cancelled: bool,
    err_reason: &str,
) {
    finish_settled_with(
        coordinator,
        terminals,
        terminal_id,
        req_id,
        sid,
        is_error,
        cancelled,
        err_reason,
        None,
        Vec::new(),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_settled_with(
    coordinator: &Coordinator,
    terminals: &Terminals,
    terminal_id: u64,
    req_id: &str,
    sid: Option<&str>,
    is_error: bool,
    cancelled: bool,
    err_reason: &str,
    anchor_patch: Option<Value>,
    warnings: Vec<String>,
) {
    let (status, reason) = if is_error {
        ("failed", Some(err_reason))
    } else if cancelled {
        (
            "cancelled",
            Some(if err_reason.is_empty() {
                "收到取消并结束当前轮"
            } else {
                err_reason
            }),
        )
    } else {
        ("succeeded", None)
    };
    terminals.clear_derived_turn(terminal_id, sid);
    coordinator.mark_finished_with(req_id, status, reason, sid, anchor_patch, warnings);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish_exited(
    coordinator: &Coordinator,
    terminals: &Terminals,
    terminal_id: u64,
    req_id: &str,
    sid: Option<&str>,
    code: u32,
    had_settled: bool,
    cancelled: bool,
) {
    // The lease goes first: once the turn is cleared, the row may be started again with a new one.
    terminals.release_derived_lease(terminal_id);
    terminals.clear_derived_turn(terminal_id, sid);
    let (status, reason) = if cancelled {
        ("cancelled", "派生终端已中断退出".to_owned())
    } else if code == 0 && had_settled {
        ("succeeded", String::new())
    } else {
        ("failed", format!("派生终端提前退出（exit {code}）"))
    };
    coordinator.mark_finished(
        req_id,
        status,
        (!reason.is_empty()).then_some(reason.as_str()),
        sid,
    );
}

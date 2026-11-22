//! Claude & Antigravity interactive TUI derived launch, readiness gating, and turn monitors.

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
    POLL_STEP, claude, cleanup_derived_startup, detect_screen_blocker, native_logs,
    prepare_open_or_launch, update_blocker_if_changed,
};

const READY_TIMEOUT: Duration = Duration::from_secs(45);
const PASTE_COMMIT_DELAY: Duration = Duration::from_millis(300);
pub(super) const TRANSCRIPT_FLUSH_WAIT: Duration = Duration::from_millis(1800);
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
    hook_dir: Option<&Path>,
) -> Result<(DerivedHandle, Option<PathBuf>), String> {
    let Some(program) = installed_path(&cfg.provider) else {
        return Err("这个 CLI 没有安装".to_owned());
    };
    let (mut args, mut extra_env, log_dir_opt, log_file_opt) = match kind {
        TuiKind::Claude => {
            let sid = preset_sid.as_deref().unwrap_or_default();
            let args = claude::build_claude_args(
                cfg,
                env.session_id.as_deref(),
                env.session_id.is_none().then_some(sid),
                hook_dir,
            );
            (args, Vec::new(), None, None)
        }
        TuiKind::Antigravity => {
            let dir = tempfile::Builder::new()
                .prefix("agentnave-agy-")
                .tempdir()
                .map_err(|e| format!("无法创建 agy 日志目录：{e}"))?;
            let log_path = dir.path().join("agy.log");
            let args = build_agy_args(cfg, env.session_id.as_deref(), cwd, &log_path);
            let extra = vec![
                ("AGY_CLI_OSC7501".to_owned(), "on".to_owned()),
                ("AGY_CLI_OSC7501_MESSAGES".to_owned(), "brief".to_owned()),
            ];
            (args, extra, Some(dir), Some(log_path))
        }
    };
    let (launch_program, launch_args) = match &account {
        Some(acc) => {
            let (p, a, more) = account::launch_as(acc, program, args, true);
            extra_env.extend(more);
            (p, a)
        }
        None => (program, std::mem::take(&mut args)),
    };
    let session = Session::spawn(&Launch {
        program: launch_program,
        args: launch_args,
        env: extra_env,
        cwd: cwd.to_path_buf(),
        cols: 100,
        rows: 30,
        derived: true,
    })?;
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
            _log_dir: log_dir_opt,
            codex_ready_file: None,
            active_invocation_id: Some(env.invocation_id.clone()),
            launch_config: launch_config.clone(),
        },
    });
    coordinator.sync_derived_pids(terminals);
    Ok((
        DerivedHandle {
            id: tid,
            provider: cfg.provider.clone(),
            session,
            native: preset_sid,
            account,
            launch_config,
            #[cfg(unix)]
            codex: None,
        },
        log_file_opt,
    ))
}

pub(super) fn capture_baseline(
    kind: TuiKind,
    account: Option<&Active>,
    preset_sid: Option<&str>,
    requested_sid: Option<&str>,
    agy_log_path: Option<&Path>,
    session: &Session,
) -> TurnBaseline {
    let account_dir = account.map(|a| a.dir.as_path());
    let claude_homes = native_logs::claude_homes(account_dir);
    let pre_offset = match (kind, preset_sid) {
        (TuiKind::Claude, Some(sid)) => native_logs::claude_transcript_path(&claude_homes, sid)
            .map_or(0, |p| native_logs::file_len(&p)),
        _ => 0,
    };
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
fn monitor_agy_turn(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    existing_cid: Option<&str>,
    log_path: Option<&Path>,
    baseline: &TurnBaseline,
    prompt: &str,
    handle: &Arc<RunHandle>,
) {
    let agy_home = native_logs::agy_home();
    let mut confirmed_cid = existing_cid.map(str::to_owned);
    let mut user_input_index: Option<i64> = None;
    let mut blocker: Option<(String, f64)> = None;
    let mut saw_working = false;
    let mut settled_at: Option<Instant> = None;

    loop {
        if user_input_index.is_none()
            && let Some(home) = agy_home.as_deref()
            && let Some((cid, idx)) = locate_agy_turn(
                home,
                confirmed_cid.as_deref(),
                log_path,
                &baseline.agy_baseline_cids,
                baseline.agy_prev_idx,
                prompt,
            )
        {
            confirmed_cid = Some(cid.clone());
            user_input_index = Some(idx);
            terminals.set_derived_blocker(launched.id, Some(&cid), None);
            coordinator.mark_started(
                req_id,
                Some(&cid),
                Some(json!({"session_id": cid, "user_input_index": idx})),
            );
        }

        let (seq, osc_state, osc_kind) = launched.session.osc_snapshot();
        let state_str = osc_state.as_deref();
        if seq > baseline.seq_before {
            if state_str == Some("working") {
                saw_working = true;
                settled_at = None;
            }
            sync_turn_blocker(
                coordinator,
                terminals,
                launched.id,
                req_id,
                confirmed_cid.as_deref(),
                state_str,
                osc_kind.as_deref(),
                &mut blocker,
                &mut settled_at,
            );
            if is_osc_settled(state_str, saw_working) {
                let at = *settled_at.get_or_insert_with(Instant::now);
                let has_anchor = confirmed_cid.is_some() && user_input_index.is_some();
                let planner_done = matches!(
                    (agy_home.as_deref(), confirmed_cid.as_deref(), user_input_index),
                    (Some(home), Some(cid), Some(idx))
                        if native_logs::agy_turn_state(
                            &native_logs::agy_transcript_path(home, cid),
                            idx,
                        ) != native_logs::AgyTurnState::Waiting
                );
                let flush_done = if has_anchor {
                    at.elapsed() >= TRANSCRIPT_FLUSH_WAIT
                } else {
                    at.elapsed() >= MISSING_ANCHOR_WAIT
                };
                if !handle.interrupt_pending()
                    && (state_str == Some("error") || planner_done || flush_done)
                {
                    let missing_anchor = state_str != Some("error")
                        && !handle.is_interrupt_confirmed()
                        && !has_anchor;
                    finish_settled(
                        coordinator,
                        terminals,
                        launched.id,
                        req_id,
                        confirmed_cid.as_deref(),
                        state_str == Some("error") || missing_anchor,
                        handle.is_interrupt_confirmed(),
                        if missing_anchor {
                            "缺少本轮锚点（未在 Antigravity 转录中匹配到本次输入）"
                        } else {
                            "Antigravity 返回错误状态"
                        },
                    );
                    return;
                }
            }
        }

        if let Some(code) = launched.session.exit() {
            let has_anchor = confirmed_cid.is_some() && user_input_index.is_some();
            finish_exited(
                coordinator,
                terminals,
                launched.id,
                req_id,
                confirmed_cid.as_deref(),
                code,
                settled_at.is_some() && has_anchor,
                handle.is_interrupt_confirmed(),
            );
            return;
        }
        thread::sleep(POLL_STEP);
    }
}

fn locate_agy_turn(
    home: &Path,
    existing: Option<&str>,
    log_path: Option<&Path>,
    baseline_cids: &[String],
    prev_idx: Option<i64>,
    prompt: &str,
) -> Option<(String, i64)> {
    let candidates: Vec<String> = if let Some(cid) = existing {
        vec![cid.to_owned()]
    } else if let Some(log) = log_path {
        native_logs::agy_log_conversation_ids(log)
            .into_iter()
            .filter(|cid| !baseline_cids.iter().any(|b| b == cid))
            .collect()
    } else {
        Vec::new()
    };
    for cid in candidates {
        let transcript = native_logs::agy_transcript_path(home, &cid);
        if let Some(idx) = native_logs::agy_match_user_input(&transcript, prev_idx, prompt) {
            return Some((cid, idx));
        }
    }
    None
}

pub(super) fn is_osc_settled(state: Option<&str>, saw_working: bool) -> bool {
    matches!(state, Some("done" | "error")) || (saw_working && state == Some("idle"))
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
    let (status, reason) = if is_error {
        ("failed", Some(err_reason))
    } else if cancelled {
        ("cancelled", Some("收到取消并结束当前轮"))
    } else {
        ("succeeded", None)
    };
    terminals.clear_derived_turn(terminal_id, sid);
    coordinator.mark_finished(req_id, status, reason, sid);
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
    terminals.clear_derived_turn(terminal_id, sid);
    terminals.release_derived_lease(terminal_id);
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

fn highest_agy_user_index(path: &Path) -> Option<i64> {
    let text = fs::read_to_string(path).ok()?;
    let mut best = None;
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row["type"].as_str() == Some("USER_INPUT")
            && row["source"].as_str() == Some("USER_EXPLICIT")
            && let Some(step) = row["step_index"].as_i64()
        {
            best = Some(best.map_or(step, |cur: i64| cur.max(step)));
        }
    }
    best
}

//! Claude Code interactive TUI derived launch, `permission_fallback` retry, and turn monitor.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};

use super::super::account::{self, Active};
use super::super::session::{self, Launch, Session};
use super::super::spawn_requests::{
    Coordinator, ParentRef, RunConfig, RunHandle, SpawnRequestEnvelope,
};
use super::super::{DerivedHandle, Terminals, installed_path, theme};
use super::tui::{
    TRANSCRIPT_FLUSH_WAIT, TuiKind, TurnBaseline, capture_baseline, finish_exited,
    finish_pre_ready, finish_pre_submit, finish_settled, is_osc_settled, spawn_tui_terminal,
    submit_prompt, sync_turn_blocker, wait_tui_ready,
};
use super::{POLL_STEP, claude_hook, native_logs, prepare_open_or_launch};

enum ClaudeTurnResult {
    Finished,
    ModeBlocked,
}

pub(super) fn build_claude_args(
    cfg: &RunConfig,
    resume: Option<&str>,
    fresh: Option<&str>,
    hook_dir: Option<&Path>,
) -> Vec<String> {
    let mut args = session::cli_args("claude", resume, fresh);
    let mut settings = serde_json::Map::new();
    let base_theme = theme::for_cli("claude", false);
    if base_theme.args.len() == 2
        && base_theme.args[0] == "--settings"
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&base_theme.args[1])
    {
        settings.extend(obj);
    }
    if let Some(mode) = cfg
        .permissions
        .get("permission_mode")
        .and_then(Value::as_str)
    {
        if matches!(
            mode,
            "auto" | "bypassPermissions" | "default" | "plan" | "acceptEdits" | "dontAsk"
        ) {
            args.push("--permission-mode".to_owned());
            args.push(mode.to_owned());
        }
        if mode == "bypassPermissions" {
            settings.insert(
                "skipDangerousModePermissionPrompt".to_owned(),
                Value::Bool(true),
            );
        } else if mode == "auto" {
            settings.insert("skipAutoPermissionPrompt".to_owned(), Value::Bool(true));
        }
    }
    if let Some(dir) = hook_dir
        && cfg
            .permissions
            .get("permission_fallback")
            .and_then(Value::as_str)
            .is_some()
    {
        settings.insert(
            "hooks".to_owned(),
            json!({"UserPromptSubmit": claude_hook::hook_settings(dir)}),
        );
    }
    if !settings.is_empty() {
        args.push("--settings".to_owned());
        args.push(Value::Object(settings).to_string());
    }
    if let Some(model) = cfg.model.as_deref().filter(|s| !s.is_empty()) {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    if let Some(effort) = cfg.effort.as_deref().filter(|s| !s.is_empty()) {
        args.push("--effort".to_owned());
        args.push(effort.to_owned());
    }
    args
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_claude_cli(
    coordinator: &Coordinator,
    terminals: &Terminals,
    env: &SpawnRequestEnvelope,
    cfg: &RunConfig,
    parent: &ParentRef,
    cwd: &Path,
    account: Option<Active>,
    prompt: &str,
    handle: &Arc<RunHandle>,
) {
    let req_id = &env.request_id;
    let (req_mode, fallback) = match claude_hook::claude_permission_modes(&cfg.permissions) {
        Ok(pair) => pair,
        Err(err) => {
            coordinator.mark_finished(req_id, "failed", Some(&err), env.session_id.as_deref());
            return;
        }
    };
    let attempts: Vec<Option<&str>> = match (req_mode, fallback) {
        (Some(req), Some(fb)) => vec![Some(req), Some(fb)],
        _ => vec![req_mode],
    };
    let mut preset_sid = env
        .session_id
        .clone()
        .unwrap_or_else(session::fresh_session_id);
    let mut launched_opt: Option<DerivedHandle> = None;

    for (idx, mode_opt) in attempts.iter().enumerate() {
        let mut attempt_cfg = cfg.clone();
        if let Some(mode) = mode_opt
            && let Some(obj) = attempt_cfg.permissions.as_object_mut()
        {
            obj.insert("permission_mode".to_owned(), json!(mode));
        }
        let mut hook_dir: Option<tempfile::TempDir> = None;
        let launched = if idx == 0 {
            match prepare_open_or_launch(
                coordinator,
                terminals,
                env,
                &attempt_cfg,
                cwd,
                account.as_ref(),
                |lease, isolation_id| {
                    if fallback.is_some() {
                        hook_dir = Some(claude_hook::prepare_mode_hook()?);
                    }
                    let (h, _) = spawn_tui_terminal(
                        terminals,
                        env,
                        &attempt_cfg,
                        parent,
                        cwd,
                        account.clone(),
                        Some(preset_sid.clone()),
                        TuiKind::Claude,
                        lease,
                        isolation_id,
                        hook_dir.as_ref().map(|d| d.path()),
                    )?;
                    Ok(h)
                },
            ) {
                Ok(h) => h,
                Err(err) => {
                    coordinator.mark_finished(req_id, "failed", Some(&err), Some(&preset_sid));
                    return;
                }
            }
        } else {
            let prev = launched_opt.take().expect("previous attempt handle");
            let Some((next_handle, dir)) = prepare_fallback_attempt(
                coordinator,
                terminals,
                env,
                &attempt_cfg,
                cwd,
                account.as_ref(),
                prev,
                &mut preset_sid,
                handle,
            ) else {
                return;
            };
            hook_dir = Some(dir);
            next_handle
        };

        coordinator.set_session_id(req_id, &preset_sid);
        if !wait_tui_ready(coordinator, terminals, &launched, req_id, handle) {
            finish_pre_ready(coordinator, terminals, &launched, req_id, handle);
            return;
        }
        let baseline = capture_baseline(
            TuiKind::Claude,
            account.as_ref(),
            Some(&preset_sid),
            env.session_id.as_deref(),
            None,
            &launched.session,
        );
        if !submit_prompt(coordinator, terminals, &launched, req_id, prompt, handle) {
            return;
        }
        match monitor_claude_turn(
            coordinator,
            terminals,
            &launched,
            req_id,
            &preset_sid,
            &baseline,
            prompt,
            handle,
            hook_dir.as_ref().map(|d| d.path()),
        ) {
            ClaudeTurnResult::Finished => return,
            ClaudeTurnResult::ModeBlocked => {
                launched.session.stop();
                if env.session_id.is_none() {
                    claude_hook::drop_empty_claude_session(&baseline.claude_homes, &preset_sid);
                }
                if idx + 1 < attempts.len() {
                    launched_opt = Some(launched);
                    continue;
                }
                terminals.set_derived_state(launched.id, None, Some(&preset_sid), false, None);
                let msg = format!(
                    "Claude Code started in manual mode because neither {} nor {} mode is available here; enable one of them in Claude Code's settings",
                    req_mode.unwrap_or_default(),
                    fallback.unwrap_or_default()
                );
                coordinator.mark_finished(req_id, "failed", Some(&msg), Some(&preset_sid));
                return;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_fallback_attempt(
    coordinator: &Coordinator,
    terminals: &Terminals,
    env: &SpawnRequestEnvelope,
    attempt_cfg: &RunConfig,
    cwd: &Path,
    account: Option<&Active>,
    mut prev: DerivedHandle,
    preset_sid: &mut String,
    handle: &Arc<RunHandle>,
) -> Option<(DerivedHandle, tempfile::TempDir)> {
    let req_id = &env.request_id;
    if handle.is_cancelled() {
        finish_pre_submit(
            coordinator,
            terminals,
            &prev,
            req_id,
            "cancelled",
            "切换回退权限模式前已取消",
        );
        return None;
    }
    if env.session_id.is_none() {
        *preset_sid = session::fresh_session_id();
    }
    let dir = match claude_hook::prepare_mode_hook() {
        Ok(dir) => dir,
        Err(err) => {
            finish_pre_submit(coordinator, terminals, &prev, req_id, "failed", &err);
            return None;
        }
    };
    let Some(program) = installed_path(&attempt_cfg.provider) else {
        finish_pre_submit(
            coordinator,
            terminals,
            &prev,
            req_id,
            "failed",
            "这个 CLI 没有安装",
        );
        return None;
    };
    let new_session = match spawn_claude_session(
        attempt_cfg,
        program,
        cwd,
        account,
        env.session_id.as_deref(),
        preset_sid,
        Some(dir.path()),
    ) {
        Ok(s) => s,
        Err(err) => {
            finish_pre_submit(coordinator, terminals, &prev, req_id, "failed", &err);
            return None;
        }
    };
    terminals.replace_derived_session(prev.id, new_session.clone(), Some(preset_sid));
    prev.session = new_session;
    prev.native = Some(preset_sid.clone());
    Some((prev, dir))
}

fn spawn_claude_session(
    cfg: &RunConfig,
    program: PathBuf,
    cwd: &Path,
    account: Option<&Active>,
    resume: Option<&str>,
    preset_sid: &str,
    hook_dir: Option<&Path>,
) -> Result<Arc<Session>, String> {
    let args = build_claude_args(
        cfg,
        resume,
        resume.is_none().then_some(preset_sid),
        hook_dir,
    );
    let mut extra_env = Vec::new();
    let (launch_program, launch_args) = match account {
        Some(acc) => {
            let (p, a, more) = account::launch_as(acc, program, args, true);
            extra_env.extend(more);
            (p, a)
        }
        None => (program, args),
    };
    Session::spawn(&Launch {
        program: launch_program,
        args: launch_args,
        env: extra_env,
        cwd: cwd.to_path_buf(),
        cols: 100,
        rows: 30,
        derived: true,
    })
}

#[allow(clippy::too_many_arguments)]
fn monitor_claude_turn(
    coordinator: &Coordinator,
    terminals: &Terminals,
    launched: &DerivedHandle,
    req_id: &str,
    sid: &str,
    baseline: &TurnBaseline,
    prompt: &str,
    handle: &Arc<RunHandle>,
    hook_dir: Option<&Path>,
) -> ClaudeTurnResult {
    let mut started = false;
    let mut mode_confirmed = hook_dir.is_none();
    let mut prompt_id: Option<String> = None;
    let mut blocker: Option<(String, f64)> = None;
    let mut saw_working = false;
    let mut settled_at: Option<Instant> = None;

    loop {
        if !mode_confirmed
            && let Some(dir) = hook_dir
            && let Some(actual) = claude_hook::read_observed_mode(dir)
        {
            if actual == "default" {
                return ClaudeTurnResult::ModeBlocked;
            }
            mode_confirmed = true;
        }
        if mode_confirmed
            && prompt_id.is_none()
            && let Some(path) = native_logs::claude_transcript_path(&baseline.claude_homes, sid)
            && let Some(pid) = native_logs::claude_prompt_anchor(&path, baseline.pre_offset, prompt)
        {
            prompt_id = Some(pid.clone());
            let anchor = json!({"session_id": sid, "prompt_id": pid});
            if !started {
                started = true;
                coordinator.mark_started(req_id, Some(sid), Some(anchor));
            } else {
                coordinator.update_anchor(req_id, anchor);
            }
        }

        let (seq, osc_state, osc_kind) = launched.session.osc_snapshot();
        let state_str = osc_state.as_deref();
        if seq > baseline.seq_before {
            if state_str == Some("working") {
                saw_working = true;
                settled_at = None;
                if mode_confirmed && !started {
                    started = true;
                    coordinator.mark_started(req_id, Some(sid), Some(json!({"session_id": sid})));
                }
            }
            sync_turn_blocker(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                state_str,
                osc_kind.as_deref(),
                &mut blocker,
                &mut settled_at,
            );
            if is_osc_settled(state_str, saw_working) {
                if mode_confirmed && !started {
                    started = true;
                    coordinator.mark_started(req_id, Some(sid), Some(json!({"session_id": sid})));
                }
                let at = *settled_at.get_or_insert_with(Instant::now);
                let transcript_ready = prompt_id.as_deref().is_some_and(|pid| {
                    native_logs::claude_transcript_path(&baseline.claude_homes, sid).is_some_and(
                        |p| {
                            native_logs::claude_turn_state(&p, pid)
                                != native_logs::ClaudeTurnState::Active
                        },
                    )
                });
                if state_str == Some("error")
                    || (mode_confirmed && transcript_ready)
                    || at.elapsed() >= TRANSCRIPT_FLUSH_WAIT
                {
                    if !mode_confirmed
                        && let Some(dir) = hook_dir
                        && claude_hook::read_observed_mode(dir).as_deref() == Some("default")
                    {
                        return ClaudeTurnResult::ModeBlocked;
                    }
                    finish_settled(
                        coordinator,
                        terminals,
                        launched.id,
                        req_id,
                        Some(sid),
                        state_str == Some("error"),
                        handle.is_cancelled(),
                        "CLI 返回错误状态",
                    );
                    return ClaudeTurnResult::Finished;
                }
            }
        }

        if let Some(code) = launched.session.exit() {
            if !mode_confirmed
                && let Some(dir) = hook_dir
                && claude_hook::read_observed_mode(dir).as_deref() == Some("default")
            {
                return ClaudeTurnResult::ModeBlocked;
            }
            finish_exited(
                coordinator,
                terminals,
                launched.id,
                req_id,
                Some(sid),
                code,
                settled_at.is_some(),
                handle.is_cancelled(),
            );
            return ClaudeTurnResult::Finished;
        }
        thread::sleep(POLL_STEP);
    }
}

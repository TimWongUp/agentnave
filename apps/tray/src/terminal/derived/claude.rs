//! Claude Code interactive TUI derived launch, `permission_fallback` retry, and turn monitor.

mod monitor;
mod screen;
mod turn_state;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::super::account::{self, Active};
use super::super::session::{self, Launch, Session};
use super::super::spawn_requests::{
    Coordinator, ParentRef, RunConfig, RunHandle, SpawnRequestEnvelope,
};
use super::super::{DerivedHandle, Terminals, installed_path, theme};
use super::tui::{
    TuiKind, capture_baseline, finish_pre_ready, finish_pre_submit, spawn_tui_terminal,
    submit_prompt, wait_tui_ready,
};
use super::{
    claude_hook, cleanup_derived_startup, detect_auth_blocker_in_text, prepare_open_or_launch,
};
pub(super) use monitor::monitor_claude_turn;
#[cfg(test)]
pub(super) use monitor::{claude_turn_ended, monitor_claude_turn_with_config};
pub(super) use screen::ClaudeScreen;
pub(super) use turn_state::BG_WAIT_REMINDER_DELAY;
#[cfg(test)]
pub(super) use turn_state::ClaudeMonitorConfig;

const TRUST_SETTLE: Duration = Duration::from_millis(300);
const TRUST_MAX_TRIES: usize = 6;
const TRUST_AFFIRMATIVE: &str = "yes, i trust this folder";
const TRUST_NEGATIVES: [&str; 2] = ["no, exit", "no, continue without these permissions"];
const KEY_DOWN: &[u8] = b"\x1b[B";
const KEY_ENTER: &[u8] = b"\r";

pub(super) enum StartupStep<'a> {
    Wait,
    SentKey,
    Blocker((&'a str, &'static str)),
    WriteFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TrustDecision {
    Press(&'static [u8]),
    Block,
}

pub(super) struct ClaudeStartup {
    cwd: PathBuf,
    screen: ClaudeScreen,
    raw_offset: u64,
    previous: String,
    stable_since: Instant,
    tries: usize,
}

impl ClaudeStartup {
    pub(super) fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            screen: ClaudeScreen::new(),
            raw_offset: 0,
            previous: String::new(),
            stable_since: Instant::now(),
            tries: 0,
        }
    }

    pub(super) fn step<'a>(
        &mut self,
        session: &Session,
        osc_state: Option<&str>,
        osc_kind: Option<&'a str>,
    ) -> StartupStep<'a> {
        let (next_offset, bytes) = session.raw_output_from(self.raw_offset);
        self.raw_offset = next_offset;
        if !bytes.is_empty() {
            self.screen.feed(&bytes);
        }
        let now = Instant::now();
        let current = self.screen.text();
        if current != self.previous {
            self.previous = current;
            self.stable_since = now;
        }
        if now.duration_since(self.stable_since) < TRUST_SETTLE {
            return StartupStep::Wait;
        }
        match evaluate_trust_dialog(&self.previous, &self.cwd) {
            Some(TrustDecision::Press(key)) => {
                self.tries += 1;
                if self.tries > TRUST_MAX_TRIES {
                    return StartupStep::Blocker(("trust", "等待目录信任确认"));
                }
                if session.write(key).is_err() {
                    return StartupStep::WriteFailed;
                }
                self.stable_since = Instant::now();
                StartupStep::SentKey
            }
            Some(TrustDecision::Block) => StartupStep::Blocker(("trust", "等待目录信任确认")),
            None => {
                let region = dialog_region(&self.previous);
                if let Some(auth) = detect_auth_blocker_in_text(&region) {
                    return StartupStep::Blocker(auth);
                }
                if osc_state == Some("blocked") {
                    return StartupStep::Blocker((
                        osc_kind.unwrap_or("permission"),
                        "终端启动等待用户确认",
                    ));
                }
                StartupStep::Wait
            }
        }
    }
}

pub(super) fn evaluate_trust_dialog(screen: &str, cwd: &Path) -> Option<TrustDecision> {
    let region = dialog_region(screen);
    let lowered = region.to_ascii_lowercase();
    if lowered.contains("allow external claude.md file imports?") {
        return None;
    }
    if !(lowered.contains("trust this folder")
        || lowered.contains("do you trust")
        || lowered.contains("trust this workspace")
        || lowered.contains("yes, i trust"))
    {
        return None;
    }
    if in_input_box(screen) || !workspace_matches(&region, cwd) {
        return Some(TrustDecision::Block);
    }
    let choices = bottom_choices(&region);
    let labels: Vec<String> = choices
        .to_ascii_lowercase()
        .lines()
        .map(|line| line.replace('❯', "").trim().to_owned())
        .collect();
    let has_affirmative = labels.iter().any(|l| l == TRUST_AFFIRMATIVE);
    let has_negative = labels.iter().any(|l| TRUST_NEGATIVES.contains(&l.as_str()));
    let selected_count = labels.iter().filter(|l| selected(&choices, l)).count();
    if labels.len() != 2 || !has_affirmative || !has_negative || selected_count != 1 {
        return Some(TrustDecision::Block);
    }
    if selected(&choices, TRUST_AFFIRMATIVE) {
        Some(TrustDecision::Press(KEY_ENTER))
    } else {
        Some(TrustDecision::Press(KEY_DOWN))
    }
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 20 && trimmed.chars().all(|c| c == '─')
}

pub(super) fn dialog_region(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let cursor = lines
        .iter()
        .rposition(|l| l.contains('❯'))
        .unwrap_or(lines.len());
    let start = lines[..cursor]
        .iter()
        .rposition(|l| is_rule(l))
        .map_or(0, |idx| idx + 1);
    lines[start..].join("\n")
}

pub(super) fn in_input_box(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let Some(cursor) = lines.iter().rposition(|l| l.contains('❯')) else {
        return false;
    };
    lines[cursor + 1..].iter().any(|l| is_rule(l))
}

pub(super) fn bottom_choices(region: &str) -> String {
    let lines: Vec<&str> = region
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 3
        || !lines
            .last()
            .is_some_and(|l| l.to_ascii_lowercase().starts_with("enter to confirm"))
    {
        return String::new();
    }
    format!("{}\n{}", lines[lines.len() - 3], lines[lines.len() - 2])
}

pub(super) fn selected(text: &str, label: &str) -> bool {
    text.to_ascii_lowercase()
        .lines()
        .any(|line| line.contains('❯') && line.replace('❯', "").trim() == label)
}

pub(super) fn workspace_matches(region: &str, cwd: &Path) -> bool {
    let lowered = region.to_ascii_lowercase();
    let cutoff = ["yes, i trust this folder", "no, exit"]
        .into_iter()
        .filter_map(|needle| lowered.find(needle))
        .min()
        .unwrap_or(region.len());
    let before_choices = &region[..cutoff];
    let mut paths = Vec::new();
    for line in before_choices.lines() {
        let trimmed = line.trim();
        let candidate = trimmed
            .strip_prefix("Accessing workspace:")
            .unwrap_or(trimmed)
            .trim();
        let expanded = expand_tilde(candidate);
        if expanded.is_absolute() {
            paths.push(expanded);
        }
    }
    paths.len() == 1 && same_file(&paths[0], cwd)
}

fn expand_tilde(path: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir);
    if path == "~"
        && let Some(home) = home
    {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
            return false;
        };
        ma.dev() == mb.dev() && ma.ino() == mb.ino()
    }
    #[cfg(not(unix))]
    {
        let (Ok(ca), Ok(cb)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) else {
            return false;
        };
        ca == cb
    }
}

pub(super) enum ClaudeTurnResult {
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
    if let Some(dir) = hook_dir {
        let include_mode_fallback = cfg
            .permissions
            .get("permission_fallback")
            .and_then(Value::as_str)
            .is_some();
        settings.insert(
            "hooks".to_owned(),
            claude_hook::build_claude_hooks(dir, include_mode_fallback),
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
    let mut reused = false;

    for (idx, mode_opt) in attempts.iter().enumerate() {
        let mut attempt_cfg = cfg.clone();
        if let Some(mode) = mode_opt
            && let Some(obj) = attempt_cfg.permissions.as_object_mut()
        {
            obj.insert("permission_mode".to_owned(), json!(mode));
        }
        let launched = if idx == 0 {
            match prepare_open_or_launch(
                coordinator,
                terminals,
                env,
                cfg,
                cwd,
                account.as_ref(),
                handle,
                |lease, isolation_id| {
                    let hook_dir = claude_hook::prepare_claude_hook(fallback.is_some())?;
                    let (h, _) = spawn_tui_terminal(
                        coordinator,
                        terminals,
                        env,
                        &attempt_cfg,
                        cfg,
                        parent,
                        cwd,
                        account.clone(),
                        Some(preset_sid.clone()),
                        TuiKind::Claude,
                        lease,
                        isolation_id,
                        Some(hook_dir),
                    )?;
                    Ok(h)
                },
            ) {
                Ok((h, was_reused)) => {
                    reused = was_reused;
                    h
                }
                Err(err) => {
                    let status = if handle.is_pre_start_cancelled() {
                        "cancelled"
                    } else {
                        "failed"
                    };
                    coordinator.mark_finished(req_id, status, Some(&err), Some(&preset_sid));
                    return;
                }
            }
        } else {
            let prev = launched_opt.take().expect("previous attempt handle");
            let Some(next_handle) = prepare_fallback_attempt(
                coordinator,
                terminals,
                env,
                &attempt_cfg,
                cwd,
                account.as_ref(),
                prev,
                reused,
                &mut preset_sid,
                handle,
            ) else {
                return;
            };
            next_handle
        };

        coordinator.set_session_id(req_id, &preset_sid);
        if handle.is_pre_start_cancelled()
            || !wait_tui_ready(coordinator, terminals, &launched, req_id, handle, Some(cwd))
        {
            finish_pre_ready(coordinator, terminals, &launched, reused, req_id, handle);
            return;
        }
        let baseline = capture_baseline(
            TuiKind::Claude,
            account.as_ref(),
            Some(&preset_sid),
            env.session_id.as_deref(),
            None,
            launched.hook_dir.as_deref(),
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
        let check_mode_hook = !reused && fallback.is_some();
        match monitor_claude_turn(
            coordinator,
            terminals,
            &launched,
            req_id,
            &preset_sid,
            &baseline,
            &sent_prompt,
            handle,
            launched.hook_dir.as_deref(),
            check_mode_hook,
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
                cleanup_derived_startup(terminals, &launched, reused, Some(&preset_sid));
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
    reused: bool,
    preset_sid: &mut String,
    handle: &Arc<RunHandle>,
) -> Option<DerivedHandle> {
    let req_id = &env.request_id;
    if handle.is_pre_start_cancelled() {
        finish_pre_submit(
            coordinator,
            terminals,
            &prev,
            reused,
            req_id,
            "cancelled",
            "切换回退权限模式前已取消",
        );
        return None;
    }
    if env.session_id.is_none() {
        *preset_sid = session::fresh_session_id();
    }
    let dir = match claude_hook::prepare_claude_hook(true) {
        Ok(dir) => dir,
        Err(err) => {
            finish_pre_submit(
                coordinator,
                terminals,
                &prev,
                reused,
                req_id,
                "failed",
                &err,
            );
            return None;
        }
    };
    let hook_path = dir.path().to_path_buf();
    let Some(program) = installed_path(&attempt_cfg.provider) else {
        finish_pre_submit(
            coordinator,
            terminals,
            &prev,
            reused,
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
        Some(&hook_path),
    ) {
        Ok(s) => s,
        Err(err) => {
            finish_pre_submit(
                coordinator,
                terminals,
                &prev,
                reused,
                req_id,
                "failed",
                &err,
            );
            return None;
        }
    };
    terminals.replace_derived_session(prev.id, new_session.clone(), Some(preset_sid), Some(dir));
    let _ = coordinator.sync_derived_pids(terminals);
    prev.session = new_session;
    prev.native = Some(preset_sid.clone());
    prev.hook_dir = Some(hook_path);
    Some(prev)
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

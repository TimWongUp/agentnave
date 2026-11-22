//! Claude Code interactive TUI derived launch, `permission_fallback` retry, and turn monitor.

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
use super::super::{DerivedHandle, Terminals, installed_path, theme};
use super::tui::{
    TRANSCRIPT_FLUSH_WAIT, TuiKind, TurnBaseline, capture_baseline, finish_exited,
    finish_pre_ready, finish_pre_submit, finish_settled, is_osc_settled, spawn_tui_terminal,
    submit_prompt, sync_turn_blocker, wait_tui_ready,
};
use super::{
    POLL_STEP, claude_hook, detect_auth_blocker_in_text, native_logs, prepare_open_or_launch,
};

const SCREEN_ROWS: usize = 50;
const SCREEN_COLS: usize = 200;
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

enum AnsiMatch {
    Complete(usize),
    Incomplete,
    Invalid,
}

fn match_ansi(buf: &[u8]) -> AnsiMatch {
    if buf.len() < 2 {
        return AnsiMatch::Incomplete;
    }
    match buf[1] {
        b'[' => {
            let mut i = 2;
            while i < buf.len() && (0x30..=0x3f).contains(&buf[i]) {
                i += 1;
            }
            while i < buf.len() && (0x20..=0x2f).contains(&buf[i]) {
                i += 1;
            }
            if i >= buf.len() {
                AnsiMatch::Incomplete
            } else if (0x40..=0x7e).contains(&buf[i]) {
                AnsiMatch::Complete(i + 1)
            } else {
                AnsiMatch::Invalid
            }
        }
        b']' => match_string_escape(buf, true),
        b'P' => match_string_escape(buf, false),
        _ => {
            let mut i = 1;
            while i < buf.len() && (0x20..=0x2f).contains(&buf[i]) {
                i += 1;
            }
            if i >= buf.len() {
                AnsiMatch::Incomplete
            } else if (0x30..=0x7e).contains(&buf[i]) {
                AnsiMatch::Complete(i + 1)
            } else {
                AnsiMatch::Invalid
            }
        }
    }
}

fn match_string_escape(buf: &[u8], allow_bel: bool) -> AnsiMatch {
    let mut i = 2;
    while i < buf.len() {
        if allow_bel && buf[i] == 0x07 {
            return AnsiMatch::Complete(i + 1);
        }
        if buf[i] == 0x1b {
            if i + 1 >= buf.len() {
                return AnsiMatch::Incomplete;
            }
            if buf[i + 1] == b'\\' {
                return AnsiMatch::Complete(i + 2);
            }
            return AnsiMatch::Invalid;
        }
        i += 1;
    }
    AnsiMatch::Incomplete
}

/// Minimal VT screen view for Ink cursor positioning and erases.
pub(super) struct ClaudeScreen {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    pending: Vec<u8>,
}

impl ClaudeScreen {
    pub(super) fn new() -> Self {
        Self {
            lines: vec![vec![' '; SCREEN_COLS]; SCREEN_ROWS],
            row: 0,
            col: 0,
            pending: Vec::new(),
        }
    }

    pub(super) fn feed(&mut self, data: &[u8]) {
        self.pending.extend_from_slice(data);
        let mut pos = 0;
        while pos < self.pending.len() {
            if self.pending[pos] == 0x1b {
                match match_ansi(&self.pending[pos..]) {
                    AnsiMatch::Complete(len) => {
                        let code =
                            String::from_utf8_lossy(&self.pending[pos..pos + len]).into_owned();
                        self.escape(&code);
                        pos += len;
                    }
                    AnsiMatch::Incomplete if self.pending.len() - pos < 256 => break,
                    AnsiMatch::Incomplete | AnsiMatch::Invalid => pos += 1,
                }
                continue;
            }
            let next_esc = self.pending[pos..]
                .iter()
                .position(|&b| b == 0x1b)
                .map_or(self.pending.len(), |off| pos + off);
            if !self.feed_utf8_segment(pos, next_esc, &mut pos) {
                break;
            }
        }
        if pos > 0 {
            self.pending.drain(..pos);
        }
    }

    fn feed_utf8_segment(&mut self, start: usize, end: usize, pos: &mut usize) -> bool {
        match std::str::from_utf8(&self.pending[start..end]) {
            Ok(valid) => {
                let chars: Vec<char> = valid.chars().collect();
                for ch in chars {
                    self.character(ch);
                }
                *pos = end;
                true
            }
            Err(err) => {
                let valid_len = err.valid_up_to();
                if valid_len > 0 {
                    let chars: Vec<char> =
                        std::str::from_utf8(&self.pending[start..start + valid_len])
                            .expect("valid UTF-8 prefix")
                            .chars()
                            .collect();
                    for ch in chars {
                        self.character(ch);
                    }
                    *pos = start + valid_len;
                }
                match err.error_len() {
                    None if end == self.pending.len() => false,
                    None => {
                        self.character('\u{FFFD}');
                        *pos = end;
                        true
                    }
                    Some(bad_len) => {
                        self.character('\u{FFFD}');
                        *pos += bad_len;
                        true
                    }
                }
            }
        }
    }

    fn character(&mut self, ch: char) {
        match ch {
            '\r' => self.col = 0,
            '\n' => self.row += 1,
            '\x08' => self.col = self.col.saturating_sub(1),
            c if c >= ' ' && c != '\x7f' => {
                if self.col >= SCREEN_COLS {
                    self.col = 0;
                    self.row += 1;
                }
                self.scroll();
                self.lines[self.row][self.col] = c;
                self.col += 1;
            }
            _ => {}
        }
        self.scroll();
    }

    fn scroll(&mut self) {
        if self.row >= SCREEN_ROWS {
            self.lines.remove(0);
            self.lines.push(vec![' '; SCREEN_COLS]);
            self.row = SCREEN_ROWS - 1;
        }
    }

    fn escape(&mut self, code: &str) {
        let Some(rest) = code.strip_prefix("\x1b[") else {
            return;
        };
        let Some(command) = rest.chars().last() else {
            return;
        };
        let params = &rest[..rest.len() - command.len_utf8()];
        if !params.is_empty() && !params.bytes().all(|b| b.is_ascii_digit() || b == b';') {
            return;
        }
        let values: Vec<usize> = params
            .split(';')
            .map(|v| {
                if v.is_empty() {
                    0
                } else {
                    v.parse().unwrap_or(0)
                }
            })
            .collect();
        let n = values[0].max(1);
        match command {
            'H' | 'f' => {
                let col_val = values.get(1).copied().unwrap_or(1).max(1);
                self.row = (n - 1).min(SCREEN_ROWS - 1);
                self.col = (col_val - 1).min(SCREEN_COLS - 1);
            }
            'A' => self.row = self.row.saturating_sub(n),
            'B' => self.row = (self.row + n).min(SCREEN_ROWS - 1),
            'C' => self.col = (self.col + n).min(SCREEN_COLS - 1),
            'D' => self.col = self.col.saturating_sub(n),
            'G' => self.col = (n - 1).min(SCREEN_COLS - 1),
            'J' => {
                let p = values[0];
                let start = if matches!(p, 1..=3) { 0 } else { self.row };
                let end = if p == 1 { self.row + 1 } else { SCREEN_ROWS };
                for row in start..end {
                    self.lines[row].fill(' ');
                }
            }
            'K' => {
                let p = values[0];
                let start = if matches!(p, 1 | 2) {
                    0
                } else {
                    self.col.min(SCREEN_COLS)
                };
                let end = if p == 1 {
                    (self.col + 1).min(SCREEN_COLS)
                } else {
                    SCREEN_COLS
                };
                self.lines[self.row][start..end].fill(' ');
            }
            _ => {}
        }
    }

    pub(super) fn text(&self) -> String {
        self.lines
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    }
}

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
        if !wait_tui_ready(coordinator, terminals, &launched, req_id, handle, Some(cwd)) {
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
    let mut last_transcript_check = Instant::now();

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
                let transcript_ready = prompt_id
                    .as_deref()
                    .is_some_and(|pid| claude_turn_ended(baseline, sid, pid));
                if !handle.interrupt_pending()
                    && (state_str == Some("error")
                        || (mode_confirmed && transcript_ready)
                        || at.elapsed() >= TRANSCRIPT_FLUSH_WAIT)
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

        // Background shell tasks keep the TUI reporting "working" after this
        // prompt's turn has ended; the transcript is authoritative for the turn.
        if mode_confirmed
            && !handle.interrupt_pending()
            && last_transcript_check.elapsed() >= TRANSCRIPT_POLL
            && let Some(pid) = prompt_id.as_deref()
        {
            last_transcript_check = Instant::now();
            if claude_turn_ended(baseline, sid, pid) {
                finish_settled(
                    coordinator,
                    terminals,
                    launched.id,
                    req_id,
                    Some(sid),
                    false,
                    handle.is_cancelled(),
                    "CLI 返回错误状态",
                );
                return ClaudeTurnResult::Finished;
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

const TRANSCRIPT_POLL: Duration = Duration::from_millis(500);

/// This prompt's turn has a `turn_duration` row or a later prompt in the transcript.
fn claude_turn_ended(baseline: &TurnBaseline, sid: &str, prompt_id: &str) -> bool {
    native_logs::claude_transcript_path(&baseline.claude_homes, sid).is_some_and(|p| {
        native_logs::claude_turn_state(&p, prompt_id) != native_logs::ClaudeTurnState::Active
    })
}

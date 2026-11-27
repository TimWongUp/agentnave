//! One CLI running in a pseudo-terminal, and the recent output a page replays when it attaches.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use super::{input_gate, modes, status};

/// The output kept for a page that attaches later: enough for a full screen and some scrollback.
pub(super) const KEPT: usize = 1 << 20;
const READ_CHUNK: usize = 16 * 1024;

/// What to start: the CLI's executable, its arguments, where, and the page's size in cells.
pub(crate) struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub derived: bool,
}

/// The CLI's arguments, each its own argv entry (ADR-0103): Windows Codex runs without the
/// shared daemon (Unix Codex launches via `codex_server`), and `resume` names the chat to
/// continue, for the CLIs that can. A new Claude or Grok chat may be given the ID it will have
/// (`fresh`), so its session record can be found before the chat has a workspace.
pub(super) fn cli_args(provider: &str, resume: Option<&str>, fresh: Option<&str>) -> Vec<String> {
    let args: Vec<&str> = match (provider, resume, fresh) {
        ("codex", Some(id), _) => vec!["resume", "--no-daemon", id],
        ("codex", None, _) => vec!["--no-daemon"],
        ("claude" | "grok", Some(id), _) => vec!["--resume", id],
        ("claude" | "grok", None, Some(id)) => vec!["--session-id", id],
        ("antigravity", Some(id), _) => vec!["--conversation", id],
        _ => Vec::new(),
    };
    args.into_iter().map(str::to_owned).collect()
}

/// A random version 4 UUID in the lower-case form Claude Code names its session records by.
pub(super) fn fresh_session_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No randomness to be had: the time is unique enough for one terminal.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        bytes.copy_from_slice(&nanos.to_le_bytes());
    }
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Whether `cli_args` can continue a chat of this CLI.
pub(super) fn resumes(provider: &str) -> bool {
    matches!(provider, "codex" | "claude" | "grok" | "antigravity")
}

/// Output from `from` on: the bytes start at `start`, which is later than `from` when older
/// output was already dropped, and `end` is where the next read continues.
pub(crate) struct Chunk {
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub exit: Option<u32>,
    /// The bytes were already delivered to a page before, so a page replaying them must not
    /// answer the terminal queries in them. A replay ends at the watermark, never past it.
    pub replay: bool,
    /// Whether the CLI currently subscribes to appearance changes (mode 2031).
    pub appearance: bool,
}

/// Maximum number of distinct view IDs remembered per session.
const MAX_VIEWS: usize = 64;

#[derive(Default)]
pub(super) struct Output {
    kept: VecDeque<u8>,
    /// The offset of the first kept byte in everything the CLI wrote.
    pub(super) start: u64,
    exit: Option<u32>,
    wrote: Option<Instant>,
    /// Where the output handed to a page so far ends: the watermark between replay and new.
    delivered: u64,
    /// Ordered view IDs seen so far; only the back (latest) advances `delivered`.
    views: VecDeque<String>,
    modes: modes::Tracker,
    status: status::Tracker,
}

impl Output {
    pub(super) fn end(&self) -> u64 {
        self.start + self.kept.len() as u64
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.modes.feed(bytes);
        self.kept.extend(bytes);
        let over = self.kept.len().saturating_sub(KEPT);
        self.kept.drain(..over);
        self.start += over as u64;
    }

    pub(super) fn register_view(&mut self, view: Option<&str>) {
        let Some(id) = view.filter(|v| !v.is_empty()) else {
            return;
        };
        if self.views.iter().any(|seen| seen == id) {
            return;
        }
        if self.views.len() >= MAX_VIEWS {
            self.views.pop_front();
        }
        self.views.push_back(id.to_owned());
    }

    fn is_current(&self, view: Option<&str>) -> bool {
        match (view.filter(|v| !v.is_empty()), self.views.back()) {
            (Some(id), Some(latest)) => id == latest,
            (None, None) => true,
            _ => false,
        }
    }

    /// Output after `from`. What lies before the watermark was delivered already and comes back
    /// as a replay that stops at it; anything after is new, and handing it over to the current
    /// view moves the watermark.
    pub(super) fn take_for(&mut self, from: u64, view: Option<&str>) -> Chunk {
        let start = from.clamp(self.start, self.end());
        let replay = start < self.delivered;
        let end = if replay { self.delivered } else { self.end() };
        let skip = usize::try_from(start - self.start).unwrap_or(usize::MAX);
        let len = usize::try_from(end - start).unwrap_or(usize::MAX);
        if self.is_current(view) {
            self.delivered = self.delivered.max(end);
        }
        Chunk {
            start,
            end,
            bytes: self.kept.iter().skip(skip).take(len).copied().collect(),
            exit: self.exit,
            replay,
            appearance: self.modes.appearance(),
        }
    }

    #[cfg(test)]
    pub(super) fn take(&mut self, from: u64) -> Chunk {
        self.take_for(from, None)
    }
}

pub(crate) struct Session {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    output: Mutex<Output>,
    changed: Condvar,
    gate: Mutex<input_gate::Gate>,
    #[cfg(test)]
    writes: Mutex<Option<Vec<Vec<u8>>>>,
}

impl Session {
    pub(crate) fn spawn(launch: &Launch) -> Result<Arc<Self>, String> {
        let mut command = command(&launch.program, &launch.args, &launch.cwd);
        if launch.derived {
            // Added after `command()` strips parent-session variables so derived status survives.
            command.env("AGENTNAVE_DERIVED_SESSION", "1");
        }
        for (name, value) in &launch.env {
            command.env(name, value);
        }
        Self::start(command, launch.cols, launch.rows)
    }

    pub(super) fn start(
        command: CommandBuilder,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<Self>, String> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|error| format!("无法创建终端：{error}"))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("无法启动 CLI：{error}"))?;
        // Only the CLI holds the terminal's other end, so reading ends once it and its children exit.
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let session = Arc::new(Self {
            killer: Mutex::new(child.clone_killer()),
            pid: child.process_id(),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            output: Mutex::default(),
            changed: Condvar::new(),
            gate: Mutex::default(),
            #[cfg(test)]
            writes: Mutex::default(),
        });
        let pump = session.clone();
        thread::spawn(move || pump.pump(reader, child));
        Ok(session)
    }

    /// Keeps what the CLI writes until it closes the terminal, then records how it exited.
    fn pump(&self, mut reader: Box<dyn Read + Send>, mut child: Box<dyn portable_pty::Child>) {
        let mut buffer = vec![0; READ_CHUNK];
        // macOS reports the closed terminal as an error rather than end of file.
        while let Ok(read @ 1..) = reader.read(&mut buffer) {
            let mut output = self.output();
            let queries = output.status.feed(&buffer[..read]);
            output.push(&buffer[..read]);
            output.wrote = Some(Instant::now());
            drop(output);
            for _ in 0..queries {
                let _ = self.write(status::REPLY);
            }
            self.changed.notify_all();
        }
        let code = child.wait().map_or(1, |status| status.exit_code());
        self.output().exit = Some(code);
        self.changed.notify_all();
    }

    fn output(&self) -> MutexGuard<'_, Output> {
        self.output
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Output after `from` for a specific page `view`; only the latest view advances the watermark.
    pub(crate) fn read_view(&self, from: u64, wait: Duration, view: Option<&str>) -> Chunk {
        let deadline = Instant::now() + wait;
        let mut output = self.output();
        output.register_view(view);
        while output.end() <= from && output.exit.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            output = self
                .changed
                .wait_timeout(output, left)
                .unwrap_or_else(|poison| poison.into_inner())
                .0;
        }
        output.take_for(from, view)
    }

    /// Whether the CLI wrote anything within `span`, which the office shows as working.
    pub(crate) fn wrote_within(&self, span: Duration) -> bool {
        let output = self.output();
        output.exit.is_none() && output.wrote.is_some_and(|at| at.elapsed() < span)
    }

    pub(crate) fn exit(&self) -> Option<u32> {
        self.output().exit
    }

    /// The CLI's own pid: the login shell execs it in place.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub(crate) fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let mut writer = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| format!("无法写入终端：{error}"))?;
        #[cfg(test)]
        if let Some(record) = &mut *self.writes.lock().unwrap_or_else(|p| p.into_inner()) {
            record.push(bytes.to_vec());
        }
        Ok(())
    }

    pub(crate) fn user_input(&self, bytes: &[u8], paste: bool) -> Result<(), String> {
        self.gate
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .input(bytes, paste);
        self.write(bytes)
    }

    pub(crate) fn osc_snapshot(&self) -> (u64, Option<String>, Option<String>) {
        let output = self.output();
        (
            output.status.seq,
            output.status.state.clone(),
            output.status.kind.clone(),
        )
    }

    pub(crate) fn osc_activity_seqs(&self) -> (u64, u64, u64) {
        let output = self.output();
        (
            output.status.last_working_seq,
            output.status.last_done_seq,
            output.status.last_blocked_seq,
        )
    }

    pub(crate) fn has_draft(&self) -> bool {
        let gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        gate.has_draft()
    }

    pub(crate) fn bracketed_paste_enabled(&self) -> bool {
        self.output().modes.paste()
    }

    pub(crate) fn send_paste(&self, text: &str) -> Result<String, String> {
        let clean = sanitize_paste_text(text);
        let use_paste = self.output().modes.paste();
        if !use_paste && clean.contains('\n') {
            return Err(
                "终端未进入可粘贴状态（未启用括号粘贴模式），无法安全提交多行任务".to_owned(),
            );
        }
        let payload = if use_paste {
            format!("\x1b[200~{clean}\x1b[201~")
        } else {
            clean.clone()
        };
        self.write(payload.as_bytes())?;
        Ok(clean)
    }

    pub(crate) fn recent_screen_text(&self, max_bytes: usize) -> String {
        let raw: Vec<u8> = {
            let output = self.output();
            let skip = output.kept.len().saturating_sub(max_bytes);
            output.kept.iter().skip(skip).copied().collect()
        };
        strip_ansi(&raw)
    }

    /// Read raw PTY bytes appended since `from` without moving UI replay watermarks.
    pub(crate) fn raw_output_from(&self, from: u64) -> (u64, Vec<u8>) {
        let output = self.output();
        let start = from.clamp(output.start, output.end());
        let skip = usize::try_from(start - output.start).unwrap_or(usize::MAX);
        let bytes: Vec<u8> = output.kept.iter().skip(skip).copied().collect();
        (output.end(), bytes)
    }

    #[cfg(all(test, unix))]
    pub(super) fn record_writes(&self) {
        *self.writes.lock().unwrap_or_else(|p| p.into_inner()) = Some(Vec::new());
    }

    #[cfg(all(test, unix))]
    pub(super) fn recorded_writes(&self) -> Vec<Vec<u8>> {
        self.writes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or_default()
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let master = self.master.lock().unwrap_or_else(|p| p.into_inner());
        master
            .resize(size(cols, rows))
            .map_err(|error| error.to_string())
    }

    /// Ends the CLI with its whole process group: hang up first, as closing a terminal would.
    pub(crate) fn stop(&self) {
        if self.exit().is_some() {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|pid| libc::pid_t::try_from(pid).ok()) {
            // The CLI leads its own session and process group; the negative pid names the group.
            unsafe {
                libc::kill(-pid, libc::SIGHUP);
            }
            return;
        }
        let _ = self.killer.lock().unwrap_or_else(|p| p.into_inner()).kill();
    }
}

pub(crate) fn sanitize_paste_text(text: &str) -> String {
    text.replace("\x1b[200~", "")
        .replace("\x1b[201~", "")
        .chars()
        .filter(|&c| c == '\t' || c == '\n' || !('\u{0000}'..='\u{001f}').contains(&c))
        .collect()
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 1000),
        cols: cols.clamp(2, 1000),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// What a parent session leaves in the App's environment when the App was started from it, such
/// as a terminal or an agent running the preview script. A CLI that inherits these takes itself for
/// that session's child (Claude Code then stops saving its transcript) or for one AgentNave
/// dispatched. The terminal starts a session of its own, like a new terminal window; the login
/// shell sets the user's own variables again.
///
/// Shares the canonical startup parent-session list in [`crate::temp_dir`] and additionally strips
/// `CLAUDE_CODE_*` and `T3CODE_HOME` for isolated interactive terminals.
pub(crate) fn parent_session(name: &str) -> bool {
    // Windows environment names are case-insensitive.
    let upper = name.to_ascii_uppercase();
    crate::temp_dir::is_startup_parent_session(name)
        || upper.starts_with("CLAUDE_CODE_")
        || upper == "T3CODE_HOME"
}

pub(super) fn strip_parent_session(
    command: &mut CommandBuilder,
    vars: impl IntoIterator<Item = std::ffi::OsString>,
) {
    for name in vars {
        if name.to_str().is_some_and(parent_session) {
            command.env_remove(name);
        }
    }
}

/// The CLI started through the user's login shell, as in their own terminal, so it sees the PATH
/// and keys set in shell startup files that an app started from the Dock lacks.
pub(super) fn command(program: &Path, args: &[String], cwd: &Path) -> CommandBuilder {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| cfg!(unix) && shell.is_absolute());
    let login = shell.and_then(|shell| crate::detect::login_exec(&shell).map(|s| (shell, s)));
    let mut command = match login {
        Some((shell, script)) => {
            let mut command = CommandBuilder::new(shell);
            command.args(["-l", "-i", "-c", script]);
            command.arg(program);
            command
        }
        None => CommandBuilder::new(program),
    };
    command.args(args);
    command.cwd(cwd);
    strip_parent_session(&mut command, std::env::vars_os().map(|(name, _)| name));
    crate::temp_dir::apply_to_pty(&mut command);
    if let Some(path) = crate::detect::search_path(program) {
        command.env("PATH", path);
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    if std::env::var_os("LANG").is_none() && std::env::var_os("LC_ALL").is_none() {
        command.env("LANG", "en_US.UTF-8");
    }
    command
}

fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') => {
                    let kind = chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                        if kind == Some(']') && c == '\n' {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        if ch == '\r' || ch == '\n' || !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

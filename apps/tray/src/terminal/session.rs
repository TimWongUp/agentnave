//! One CLI running in a pseudo-terminal, and the recent output a page replays when it attaches.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// The output kept for a page that attaches later: enough for a full screen and some scrollback.
pub(super) const KEPT: usize = 1 << 20;
const READ_CHUNK: usize = 16 * 1024;

/// What to start: the CLI's executable, its arguments, where, and the page's size in cells.
pub(crate) struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(&'static str, &'static str)>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

/// The CLI's arguments, each its own argv entry (ADR-0103): Codex runs without the shared daemon,
/// whose MCP servers sit outside this terminal's process tree, and `resume` names the chat to
/// continue, for the CLIs that can.
pub(super) fn cli_args(provider: &str, resume: Option<&str>) -> Vec<String> {
    let args: Vec<&str> = match (provider, resume) {
        ("codex", Some(id)) => vec!["resume", "--no-daemon", id],
        ("codex", None) => vec!["--no-daemon"],
        ("claude", Some(id)) => vec!["--resume", id],
        _ => Vec::new(),
    };
    args.into_iter().map(str::to_owned).collect()
}

/// Whether `cli_args` can continue a chat of this CLI.
pub(super) fn resumes(provider: &str) -> bool {
    matches!(provider, "codex" | "claude")
}

/// Output from `from` on: the bytes start at `start`, which is later than `from` when older
/// output was already dropped, and `end` is where the next read continues.
pub(crate) struct Chunk {
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub exit: Option<u32>,
}

#[derive(Default)]
pub(super) struct Output {
    kept: VecDeque<u8>,
    /// The offset of the first kept byte in everything the CLI wrote.
    pub(super) start: u64,
    exit: Option<u32>,
    wrote: Option<Instant>,
}

impl Output {
    pub(super) fn end(&self) -> u64 {
        self.start + self.kept.len() as u64
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.kept.extend(bytes);
        let over = self.kept.len().saturating_sub(KEPT);
        self.kept.drain(..over);
        self.start += over as u64;
    }
}

pub(crate) struct Session {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    output: Mutex<Output>,
    changed: Condvar,
}

impl Session {
    pub(crate) fn spawn(launch: &Launch) -> Result<Arc<Self>, String> {
        let mut command = command(&launch.program, &launch.args, &launch.cwd);
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
            output.push(&buffer[..read]);
            output.wrote = Some(Instant::now());
            drop(output);
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

    /// Output after `from`, waiting up to `wait` for some when there is none yet.
    pub(crate) fn read(&self, from: u64, wait: Duration) -> Chunk {
        let deadline = Instant::now() + wait;
        let mut output = self.output();
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
        let start = from.clamp(output.start, output.end());
        let skip = usize::try_from(start - output.start).unwrap_or(usize::MAX);
        Chunk {
            start,
            end: output.end(),
            bytes: output.kept.iter().skip(skip).copied().collect(),
            exit: output.exit,
        }
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
            .map_err(|error| format!("无法写入终端：{error}"))
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
const PARENT_SESSION: [&str; 19] = [
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_PROJECT_DIR",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "AGENTNAVE_HOST",
    "AGENTNAVE_HOST_APP",
    "AGENTNAVE_HOST_SESSION_ID",
    "AGENTNAVE_EXCLUDED_PROVIDERS",
    "AGENTNAVE_MANAGER_SESSION",
    "AGENTNAVE_MANAGER_STARTED_AT",
    "AGENTNAVE_DISPATCHED",
    // WorkBuddy and CodeBuddy point these at their own live listener.
    "SERVER__PORT",
    "SERVER__HOST",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
];

pub(super) fn parent_session(name: &str) -> bool {
    name.starts_with("CLAUDE_CODE_") || PARENT_SESSION.contains(&name)
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
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(parent_session) {
            command.env_remove(name);
        }
    }
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

//! Shared bounded waits. Callers retain command setup, pipe readers and error messages.
use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::Duration;

/// Poll without changing the caller's deadline, polling interval or error cleanup policy.
pub(crate) fn wait(
    child: &mut Child,
    pending: impl Fn() -> bool,
    interval: Duration,
) -> io::Result<Option<ExitStatus>> {
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None if pending() => thread::sleep(interval),
            None => return Ok(None),
        }
    }
}

#[cfg(unix)]
fn kill_posix_group(id: u32) {
    if let Ok(group) = libc::pid_t::try_from(id) {
        // SAFETY: kill(2) only sends a signal; a negative ID names the probe's process group.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

pub(crate) fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    kill_posix_group(child.id());
    #[cfg(windows)]
    {
        kill_tree(child.id());
        let _ = child.kill();
    }
    #[cfg(not(any(unix, windows)))]
    let _ = child.kill();
}

/// Stop a timed-out probe and reap its leader. Shell probes also kill the child directly.
pub(crate) fn stop(child: &mut Child, direct: bool) {
    kill_group(child);
    #[cfg(unix)]
    if direct {
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    let _ = direct;
    let _ = child.wait();
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Suppress console window creation for headless child processes on Windows.
pub(crate) fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn kill_tree(id: u32) {
    #[cfg(unix)]
    kill_posix_group(id);
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/PID", &id.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(crate::temp_dir::usable());
        hide_window(&mut command);
        crate::temp_dir::apply_to_command(&mut command);
        let _ = command.status();
    }
}

#[derive(Debug)]
pub(crate) enum OutputError {
    Io(io::Error),
    Timeout,
    Reader,
}

/// Bound wait_with_output, including its pipe readers, and stop the tree before joining.
pub(crate) fn output(mut command: Command, limit: Duration) -> Result<Output, OutputError> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    hide_window(&mut command);
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(OutputError::Io)?;
    let id = child.id();
    let (done, finished) = std::sync::mpsc::channel();
    let waiter = thread::spawn(move || {
        let output = child.wait_with_output();
        let _ = done.send(());
        output
    });
    if finished.recv_timeout(limit).is_err() {
        kill_tree(id);
        let _ = waiter.join();
        return Err(OutputError::Timeout);
    }
    waiter
        .join()
        .map_err(|_| OutputError::Reader)?
        .map_err(OutputError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_runs_hidden_headless_command_within_timeout() {
        #[cfg(windows)]
        let mut command = {
            let mut cmd = Command::new("cmd");
            cmd.args(["/C", "echo", "ok"]);
            cmd
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "echo ok"]);
            cmd
        };
        command.stdin(Stdio::null());
        let out = output(command, Duration::from_secs(5)).expect("command should succeed");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }
}

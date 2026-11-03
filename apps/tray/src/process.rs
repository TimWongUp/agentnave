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
    #[cfg(not(unix))]
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

fn kill_tree(id: u32) {
    #[cfg(unix)]
    kill_posix_group(id);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &id.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
}

pub(crate) enum OutputError {
    Io(io::Error),
    Timeout,
    Reader,
}

/// Bound wait_with_output, including its pipe readers, and stop the tree before joining.
pub(crate) fn output(mut command: Command, limit: Duration) -> Result<Output, OutputError> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
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

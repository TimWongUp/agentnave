//! NSIS waits here before any plugin can turn a second instance into a successful exit.
use super::{channel, cleanup};
use ::windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use ::windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};

pub(super) const ENTRY: &str = "--agentnave-uninstall";
const MARKER: &str = ".agentnave-uninstall-approved";

pub(super) fn entry() -> Option<i32> {
    if !std::env::args().any(|arg| arg == ENTRY) {
        return None;
    }
    Some(match wait_for_confirmation() {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(error) => {
            eprintln!("无法等待卸载确认：{error}");
            2
        }
    })
}

fn wait_for_confirmation() -> Result<bool, String> {
    if crate::PREVIEW {
        return Err("预览版不能卸载 AgentNave".into());
    }
    let waiter = channel::Waiter::new().map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // A new App instance shows the page itself; an existing one receives these same arguments
    // through the plugin. This launcher's lifetime is independent of the plugin's exit code.
    let mut command = std::process::Command::new(exe);
    command
        .args([channel::REQUEST, &waiter.argument()])
        .current_dir(crate::temp_dir::usable());
    crate::temp_dir::apply_to_command(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let (mut stream, pid) = waiter.accept().map_err(|e| e.to_string())?;
    // Open before waiting for the answer so PID reuse cannot make us wait for another process.
    let process = Process(
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.map_err(|e| e.to_string())?,
    );
    let confirmed = channel::outcome(&mut stream).map_err(|e| e.to_string())?;
    if confirmed && unsafe { WaitForSingleObject(process.0, u32::MAX) } != WAIT_OBJECT_0 {
        return Err("无法确认 App 已退出".into());
    }
    // Reap a plugin-forwarding child if it has already exited; never wait for the user's
    // still-running App after cancellation.
    let _ = child.try_wait();
    Ok(confirmed)
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn finish(plan: &cleanup::Plan, external: bool) -> Result<(), String> {
    if external {
        return cleanup::spawn(plan, None);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let installed = exe.parent().ok_or("无法定位安装目录")?;
    let uninstaller = installed.join("uninstall.exe");
    if !uninstaller.is_file() {
        return Err("找不到 uninstall.exe；请按手动安装的卸载步骤移除程序".into());
    }
    let token = channel::token()?;
    let marker = installed.join(MARKER);
    // NSIS FileRead expects one raw line; write a private, token-matched marker instead of
    // putting host config edits in the installer. A stale marker alone never authorizes uninstall.
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(installed).map_err(|e| e.to_string())?;
    temporary
        .write_all(token.as_bytes())
        .map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(&marker).map_err(|e| e.to_string())?;
    if let Err(error) = cleanup::spawn(plan, Some((&uninstaller, &token))) {
        let _ = std::fs::remove_file(marker);
        return Err(error);
    }
    Ok(())
}

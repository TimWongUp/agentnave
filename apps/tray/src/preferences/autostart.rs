//! Keep platform-specific login entries within the current user's account.
use tauri::AppHandle;
#[cfg(not(windows))]
use tauri_plugin_autostart::ManagerExt;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub fn is_enabled(app: &AppHandle) -> Result<bool, String> {
    if crate::PREVIEW {
        return Ok(false);
    }
    #[cfg(windows)]
    return windows::is_enabled(&app.package_info().name);
    #[cfg(not(windows))]
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

pub fn set(app: &AppHandle, enabled: bool) -> Result<(), String> {
    super::general::reject_autostart(
        crate::PREVIEW,
        &serde_json::json!({"general": {"autostart": enabled}}),
    )?;
    #[cfg(windows)]
    return windows::set(&app.package_info().name, enabled);
    #[cfg(not(windows))]
    {
        let launcher = app.autolaunch();
        if enabled {
            launcher.enable()
        } else {
            launcher.disable()
        }
        .map_err(|error| error.to_string())
    }
}

/// Never create an entry at startup; only repair an already-enabled macOS LaunchAgent.
pub fn repair(app: &AppHandle) -> Result<(), String> {
    if crate::PREVIEW {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or("无法定位用户目录")?;
        let file = home
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", app.package_info().name));
        let exe = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|error| error.to_string())?;
        macos::repair_file(&file, &exe)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

#[cfg(any(windows, test))]
fn windows_command(exe: &std::path::Path) -> Result<String, String> {
    let exe = exe.to_str().ok_or("可执行文件路径无法转换为 Unicode")?;
    Ok(format!("\"{exe}\""))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_windows_run_value_quotes_the_entire_executable_path() {
        assert_eq!(
            super::windows_command(std::path::Path::new(r"C:\Program Files\AgentNave\tray.exe"))
                .unwrap(),
            r#""C:\Program Files\AgentNave\tray.exe""#
        );
    }
}

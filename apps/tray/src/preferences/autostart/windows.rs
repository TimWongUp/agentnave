//! The plugin cannot select HKCU or quote its executable, so Windows uses this small wrapper.
use windows_registry::CURRENT_USER;

const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// `HRESULT_FROM_WIN32` of `ERROR_FILE_NOT_FOUND` and `ERROR_PATH_NOT_FOUND`: windows-registry
/// reports a missing key or value this way, and its `io::Error` keeps the raw code, so the kind
/// is never `NotFound`.
const MISSING: [i32; 2] = [0x8007_0002u32 as i32, 0x8007_0003u32 as i32];

fn optional<T>(result: windows_registry::Result<T>) -> Result<Option<T>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            let error = std::io::Error::from(error);
            let missing = error
                .raw_os_error()
                .is_some_and(|code| MISSING.contains(&code));
            if missing || error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error.to_string())
            }
        }
    }
}

pub(super) fn is_enabled(name: &str) -> Result<bool, String> {
    let Some(key) = optional(CURRENT_USER.open(RUN))? else {
        return Ok(false);
    };
    if optional(key.get_string(name))?.is_none() {
        return Ok(false);
    }
    if let Some(key) = optional(CURRENT_USER.open(APPROVED))?
        && let Some(value) = optional(key.get_value(name))?
    {
        // Respect Task Manager's disable timestamp, as auto-launch does.
        return Ok(value.len() < 8 || value.iter().rev().take(8).all(|byte| *byte == 0));
    }
    Ok(true)
}

pub(super) fn set(name: &str, enabled: bool) -> Result<(), String> {
    if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let command = super::windows_command(&exe)?;
        CURRENT_USER
            .create(RUN)
            .and_then(|key| key.set_string(name, command))
            .map_err(|error| error.to_string())?;
        if let Some(key) = optional(CURRENT_USER.options().write().open(APPROVED))? {
            key.set_bytes(
                name,
                windows_registry::Type::Bytes,
                &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            )
            .map_err(|error| error.to_string())?;
        }
    } else if let Some(key) = optional(CURRENT_USER.options().write().open(RUN))? {
        optional(key.remove_value(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_key_that_does_not_exist_reads_as_absent() {
        // windows-registry reports a missing key as HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND),
        // whose raw code std::io::Error keeps; a settings save must not fail over it.
        let path = r"SOFTWARE\AgentNave\autostart-tests\definitely-missing";
        let found = super::optional(windows_registry::CURRENT_USER.open(path));
        assert!(matches!(found, Ok(None)));
    }
}

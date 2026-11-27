use super::*;

pub(super) fn expand(entry: &str) -> Option<PathBuf> {
    match entry.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)),
        None => Some(PathBuf::from(entry)),
    }
}

pub(super) fn common_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        // npm's global prefix. `codex`, and the npm install of `pi` and `codebuddy`, land there as
        // `.cmd` shims; a desktop app started before that PATH entry existed would miss them.
        return dirs::data_dir()
            .map(|roaming| roaming.join("npm"))
            .into_iter()
            .filter(|dir| dir.is_dir())
            .collect();
    }
    COMMON_DIRS
        .iter()
        .filter_map(|entry| expand(entry))
        .filter(|dir| dir.is_dir())
        .collect()
}

pub(crate) fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

/// The Codex or DeepSeek Harness desktop bundles on macOS, then PATH, then the common
/// install directories.
pub(super) fn find(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let desktop: &[&str] = match name {
        "codex" => &DESKTOP_CODEX,
        "dsh" => &DESKTOP_DSH,
        _ => &[],
    };
    #[cfg(target_os = "macos")]
    if let Some(path) = desktop
        .iter()
        .filter_map(|entry| expand(entry))
        .find(|path| executable(path))
    {
        return Some(path);
    }
    #[cfg(windows)]
    if let Some(path) = desktop_binary(name) {
        return Some(path);
    }
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|ext| format!("{name}.{ext}"))
            .collect()
    } else {
        vec![name.to_owned()]
    };
    search_path(&names)
}

/// The same bundled priority as `find`; standalone discovery must not use `find("codex")`.
pub(super) fn codex_desktop() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return DESKTOP_CODEX
        .iter()
        .filter_map(|entry| expand(entry))
        .find(|p| executable(p));
    #[cfg(windows)]
    return codex_bundled();
    #[cfg(not(any(target_os = "macos", windows)))]
    None
}

pub(crate) fn codex_desktop_app(path: &Path) -> Option<&'static str> {
    let real = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    for path in [path, real.as_path()] {
        let text = path.to_string_lossy().replace('\\', "/").to_lowercase();
        if text.contains("chatgpt.app/contents/") || text.contains("/openai/chatgpt/bin/") {
            return Some("ChatGPT");
        }
        if text.contains(".app/contents/")
            || text.contains("/.codex/plugins/.plugin-appserver/")
            || text.contains("/openai/codex/bin/")
        {
            return Some("Codex");
        }
    }
    None
}

pub(crate) fn standalone_codex() -> Option<PathBuf> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    standalone_codex_in(std::env::split_paths(&inherited).chain(common_dirs()))
}

pub(super) fn standalone_codex_in(dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["codex.exe", "codex.cmd", "codex.bat"]
    } else {
        &["codex"]
    };
    dirs.into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| executable(path) && codex_desktop_app(path).is_none())
}

pub(super) fn find_on_path(name: &str) -> Option<PathBuf> {
    search_path(&[name.to_owned()])
}

fn search_path(names: &[String]) -> Option<PathBuf> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&inherited)
        .chain(common_dirs())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| executable(path))
}

/// A CLI that ships inside a desktop app instead of going on PATH.
#[cfg(windows)]
pub(super) fn desktop_binary(name: &str) -> Option<PathBuf> {
    match name {
        // The DeepSeek Harness desktop app keeps its CLI under its install directory.
        "dsh" => bundled_in_app("DeepSeek Harness", "resources/runtime/cli/bin/dsh.cmd"),
        // The Codex desktop app installs the whole CLI too.
        "codex" => codex_bundled(),
        _ => None,
    }
}

#[cfg(windows)]
pub(super) fn bundled_in_app(app: &str, relative: &str) -> Option<PathBuf> {
    let dir = install_dir(app).or_else(|| {
        // The uninstall entry is the answer; the default per-user location is the fallback.
        let local = dirs::data_local_dir()?;
        Some(local.join("Programs").join(app))
    })?;
    let path = dir.join(relative);
    executable(&path).then_some(path)
}

/// The Codex desktop app ships `codex.exe` twice: a stable copy it hands to plugins, and the
/// app's own copies under `%LOCALAPPDATA%\OpenAI\Codex\bin\<content hash>`, which cannot be
/// spelled out. Prefer the stable copy and fall back to the newest hash directory.
#[cfg(windows)]
pub(super) fn codex_bundled() -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let stable = home.join(".codex/plugins/.plugin-appserver/codex.exe");
        if executable(&stable) {
            return Some(stable);
        }
    }
    let bin = dirs::data_local_dir()?.join("OpenAI/Codex/bin");
    let mut found: Vec<PathBuf> = std::fs::read_dir(bin)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join("codex.exe"))
        .filter(|path| executable(path))
        .collect();
    found.sort_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok());
    found.pop()
}

/// Where an app installed itself, according to the uninstall entry Windows keeps for it.
#[cfg(windows)]
pub(super) fn install_dir(display_name: &str) -> Option<PathBuf> {
    for key in matching_uninstall_keys(display_name) {
        if let Some(dir) = registry_string(&key, "InstallLocation").filter(|dir| !dir.is_empty()) {
            let dir = registry_path(&dir);
            if dir.is_dir() {
                return Some(dir);
            }
        }
        // Electron apps often leave `InstallLocation` out but point `DisplayIcon` at their exe.
        if let Some(icon) = registry_string(&key, "DisplayIcon")
            && let Some(parent) = registry_path(&icon).parent()
        {
            return Some(parent.to_path_buf());
        }
    }
    None
}

#[cfg(windows)]
fn uninstall_roots() -> [(&'static windows_registry::Key, &'static str); 3] {
    [
        (
            windows_registry::CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            windows_registry::LOCAL_MACHINE,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            windows_registry::LOCAL_MACHINE,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ]
}

/// Uninstall entries whose string value data mentions `needle` (case-insensitively), since
/// uninstall key names are often GUIDs.
#[cfg(windows)]
fn matching_uninstall_keys(needle: &str) -> Vec<windows_registry::Key> {
    let needle = needle.to_lowercase();
    let mut matches = Vec::new();
    for (hive, path) in uninstall_roots() {
        let Ok(root) = hive.open(path) else {
            continue;
        };
        let Ok(names) = root.keys() else {
            continue;
        };
        for name in names {
            let Ok(entry) = root.open(&name) else {
                continue;
            };
            let matched = entry.values().is_ok_and(|values| {
                values.into_iter().any(|(_, value)| {
                    String::try_from(value)
                        .ok()
                        .is_some_and(|text| text.to_lowercase().contains(&needle))
                })
            });
            if matched {
                matches.push(entry);
            }
        }
    }
    matches
}

#[cfg(windows)]
fn registry_string(key: &windows_registry::Key, name: &str) -> Option<String> {
    key.get_string(name)
        .ok()
        .map(|value| value.trim().to_owned())
}

#[cfg(windows)]
pub(crate) fn store_packages(key: &str) -> Vec<String> {
    let (hive, subpath) = if let Some(rest) = key.strip_prefix(r"HKCU\") {
        (windows_registry::CURRENT_USER, rest)
    } else if let Some(rest) = key.strip_prefix(r"HKLM\") {
        (windows_registry::LOCAL_MACHINE, rest)
    } else {
        return Vec::new();
    };
    let Ok(root) = hive.open(subpath) else {
        return Vec::new();
    };
    let Ok(names) = root.keys() else {
        return Vec::new();
    };
    names
        .filter(|name| name.contains('_') && !name.contains(' '))
        .collect()
}

/// A CLI this app knows how to find, including the ones a desktop app bundles instead of
/// putting on PATH.
pub(crate) fn locate(name: &str) -> Option<PathBuf> {
    find(name)
}

/// The path an uninstall entry names, without the quotes and icon index Windows allows around it.
#[cfg(any(test, windows))]
pub(super) fn registry_path(value: &str) -> PathBuf {
    let value = value.trim();
    let path = match value.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => value.split(',').next().unwrap_or(value),
    };
    PathBuf::from(path.trim())
}

/// The executable an installed app's uninstall entry points at, for apps that install wherever
/// the user chose and keep no other record (WorkBuddy points `DisplayIcon` at its exe).
#[cfg(windows)]
pub(crate) fn uninstall_icon(display_name: &str) -> Option<PathBuf> {
    for key in matching_uninstall_keys(display_name) {
        if let Some(icon) = registry_string(&key, "DisplayIcon") {
            let path = registry_path(&icon);
            if path.is_file() {
                return Some(path);
            }
        }
        // A folder left behind after uninstalling is no app; its executable must still be there.
        if let Some(dir) = registry_string(&key, "InstallLocation").filter(|dir| !dir.is_empty()) {
            let exe = registry_path(&dir).join(format!("{display_name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

use super::*;

pub(super) fn package(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some("@anthropic-ai/claude-code"),
        "codex" => Some("@openai/codex"),
        "codebuddy" => Some("@tencent-ai/codebuddy-code"),
        "pi" => Some("@earendil-works/pi-coding-agent"),
        "deepseek" => Some("@deepseek-ai/dsh"),
        _ => None,
    }
}

pub(super) fn desktop(path: &Path, windows: bool) -> bool {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if path.contains(".app/contents/") {
        return true;
    }
    windows
        && (path.contains("/.codex/plugins/.plugin-appserver/")
            || [
                "/openai/codex/",
                "/chatgpt/",
                "/programs/codex/",
                "/program files/codex/",
                "/program files (x86)/codex/",
                "/deepseek harness/",
                "/zcode/",
            ]
            .iter()
            .any(|part| path.contains(part))
            || (path.contains("/windowsapps/")
                && (path.contains("openai") || path.contains("codex"))))
}

/// Follow the same executable reported by detect, including npm's Windows .cmd launcher.
pub(super) fn target(path: &Path, provider: &str, windows: bool) -> Option<PathBuf> {
    let real = path.canonicalize().ok()?;
    if windows && real.extension().is_some_and(|ext| ext == "cmd") {
        let text = std::fs::read_to_string(&real).ok()?.replace('\\', "/");
        let primary = package(provider)?;
        let packages = if provider == "pi" {
            vec![primary, "@mariozechner/pi-coding-agent"]
        } else {
            vec![primary]
        };
        let (package, rest) = packages.iter().find_map(|package| {
            let marker = format!("/node_modules/{package}/");
            Some((*package, text.split_once(&marker)?.1))
        })?;
        let suffix = rest.split(['"', '\'', '\r', '\n', ' ']).next()?;
        return real
            .parent()?
            .join("node_modules")
            .join(package)
            .join(suffix)
            .canonicalize()
            .ok();
    }
    Some(real)
}

pub(super) fn identify(provider: &str, path: &Path) -> Option<Plan> {
    if matches!(provider, "antigravity" | "zcode") || desktop(path, cfg!(windows)) {
        return None;
    }
    let real = target(path, provider, cfg!(windows))?;
    if desktop(&real, cfg!(windows)) {
        return None;
    }
    let mut plan = classify(provider, &real, cfg!(windows))?;
    plan.search_path = super::super::probe::search_path(path);
    Some(plan)
}

pub(super) fn classify(provider: &str, real: &Path, windows: bool) -> Option<Plan> {
    let normalized = real.to_string_lossy().replace('\\', "/");
    let native = match provider {
        "claude" => {
            normalized.contains("/.local/share/claude/versions/")
                || (windows
                    && normalized
                        .to_lowercase()
                        .ends_with("/.local/bin/claude.exe"))
        }
        "codex" => normalized.contains("/.codex/packages/standalone/"),
        "grok" => normalized.contains("/.grok/bin/") || normalized.contains("/.grok/downloads/"),
        _ => false,
    };
    if native {
        let channel = if provider == "grok" {
            Channel::Grok(grok_channel())
        } else {
            Channel::Npm(package(provider)?.to_owned())
        };
        return Some(Plan {
            program: real.to_owned(),
            search_path: None,
            args: vec!["update".into()],
            command: format!("{provider} update"),
            via: "native",
            channel,
        });
    }
    brew(provider, real).or_else(|| npm(provider, real, windows))
}

fn brew(provider: &str, real: &Path) -> Option<Plan> {
    let names: &[&str] = match provider {
        "claude" => &["claude-code"],
        "codex" => &["codex"],
        "pi" => &["pi-coding-agent"],
        "grok" => &["grok-build"],
        _ => return None,
    };
    for ancestor in real.ancestors() {
        let name = ancestor.file_name()?.to_str()?;
        if !names.contains(&name) {
            continue;
        }
        let store = ancestor.parent()?;
        let cask = store.file_name()? == "Caskroom";
        if !cask && store.file_name()? != "Cellar" {
            continue;
        }
        if provider == "grok" && !cask {
            return None;
        }
        let program = store.parent()?.join("bin/brew");
        if !super::super::executable(&program) {
            return None;
        }
        let mut args = vec!["upgrade".to_owned()];
        if cask {
            args.push("--cask".into());
        }
        args.push(name.to_owned());
        let channel = if provider == "grok" {
            Channel::Grok(grok_channel())
        } else {
            Channel::Brew {
                program: program.clone(),
                name: name.into(),
                cask,
            }
        };
        return Some(Plan {
            program,
            search_path: None,
            command: format!("brew {}", args.join(" ")),
            args,
            via: "homebrew",
            channel,
        });
    }
    None
}

fn npm(provider: &str, real: &Path, windows: bool) -> Option<Plan> {
    let primary = package(provider)?;
    let packages = if provider == "pi" {
        vec![primary, "@mariozechner/pi-coding-agent"]
    } else {
        vec![primary]
    };
    for modules in real
        .ancestors()
        .filter(|p| p.file_name().is_some_and(|n| n == "node_modules"))
    {
        let relative = real.strip_prefix(modules).ok()?;
        let Some(package) = packages
            .iter()
            .find(|package| relative.starts_with(package))
        else {
            continue;
        };
        if !writable(modules) {
            return None;
        }
        let prefix = if windows {
            modules.parent()?
        } else {
            let lib = modules.parent()?;
            if lib.file_name()? != "lib" {
                return None;
            }
            lib.parent()?
        };
        let program = prefix.join(if windows { "npm.cmd" } else { "bin/npm" });
        if !super::super::executable(&program) {
            return None;
        }
        let npm = if windows { "npm.cmd" } else { "npm" };
        let package = format!("{package}@latest");
        return Some(Plan {
            program,
            search_path: None,
            args: vec![
                "install".into(),
                "-g".into(),
                "--prefix".into(),
                prefix.to_string_lossy().into(),
                package.clone(),
            ],
            command: format!("{npm} install -g --prefix <prefix> {package}"),
            via: "npm",
            channel: Channel::Npm(package.trim_end_matches("@latest").into()),
        });
    }
    None
}

fn writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        let Ok(meta) = path.metadata() else {
            return false;
        };
        if meta.permissions().mode() & 0o222 == 0 {
            return false;
        }
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: access reads a NUL-terminated path; it does not change permissions.
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateFileW(
                path: *const u16,
                access: u32,
                share: u32,
                security: *const std::ffi::c_void,
                creation: u32,
                flags: u32,
                template: *mut std::ffi::c_void,
            ) -> *mut std::ffi::c_void;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: open the existing directory for write access without creating or modifying it.
        unsafe {
            let handle = CreateFileW(
                path.as_ptr(),
                0x40000000,
                7,
                std::ptr::null(),
                3,
                0x02000000,
                std::ptr::null_mut(),
            );
            if handle as isize == -1 {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
}

fn grok_channel() -> String {
    let text = dirs::home_dir()
        .and_then(|home| std::fs::read_to_string(home.join(".grok/config.toml")).ok())
        .unwrap_or_default();
    parse_channel(&text)
}

pub(super) fn parse_channel(text: &str) -> String {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|table| {
            table
                .get("cli")?
                .get("channel")?
                .as_str()
                .map(str::to_owned)
        })
        .filter(|channel| {
            !channel.is_empty()
                && channel
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
        .unwrap_or_else(|| "stable".into())
}

pub(super) fn commands(os: &str) -> Value {
    let windows = os == "windows";
    let npm = if windows { "npm.cmd" } else { "npm" };
    let mut all = serde_json::Map::new();
    for provider in crate::members::PROVIDERS {
        let mut entries = Vec::new();
        match (provider, windows) {
            ("claude", false) => entries.push(json!({"label": "安装脚本", "command": "curl -fsSL https://claude.ai/install.sh | bash"})),
            ("claude", true) => entries.push(json!({"label": "PowerShell", "command": "irm https://claude.ai/install.ps1 | iex"})),
            ("codex", false) => {
                entries.push(json!({"label": "安装脚本", "command": "curl -fsSL https://chatgpt.com/codex/install.sh | sh"}));
                if os == "macos" { entries.push(json!({"label": "Homebrew", "command": "brew install --cask codex"})); }
            }
            ("codex", true) => entries.push(json!({"label": "PowerShell", "command": "powershell -ExecutionPolicy ByPass -c \"irm https://chatgpt.com/codex/install.ps1 | iex\""})),
            _ => {}
        }
        if let Some(package) = package(provider) {
            entries.push(json!({"label": "npm", "command": format!("{npm} install -g {package}")}));
        }
        all.insert((*provider).to_owned(), json!(entries));
    }
    Value::Object(all)
}

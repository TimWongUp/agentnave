use super::*;

#[allow(dead_code)]
pub(crate) fn install_service(uv: &Path, dirs: &ToolDirs) -> Result<(), String> {
    install_service_version(uv, dirs, VERSION, None)
}

/// The same directories with uv's dependency index pointed at a reachable mirror.
pub(crate) fn with_mirror(dirs: &ToolDirs) -> Option<ToolDirs> {
    if [
        "UV_DEFAULT_INDEX",
        "UV_INDEX",
        "UV_INDEX_URL",
        "UV_EXTRA_INDEX_URL",
        "PIP_INDEX_URL",
        "PIP_EXTRA_INDEX_URL",
    ]
    .iter()
    .any(|key| {
        dirs.network
            .get(*key)
            .is_some_and(|value| !value.trim().is_empty())
    }) {
        return None;
    }
    let mut mirrored = dirs.clone();
    mirrored
        .network
        .insert("UV_DEFAULT_INDEX".into(), MIRROR_INDEX.into());
    Some(mirrored)
}

pub(crate) fn install_service_version(
    uv: &Path,
    dirs: &ToolDirs,
    version: &str,
    archive: Option<&[u8]>,
) -> Result<(), String> {
    let embedded = if version == VERSION {
        extract_wheel(EMBEDDED_WHEEL, VERSION)
    } else {
        archive.and_then(|bytes| wheel_from_archive(bytes, version))
    };
    if let Some(wheel) = embedded {
        let stage = tempfile::tempdir().map_err(|error| error.to_string())?;
        let file = stage
            .path()
            .join(format!("agentnave-{version}-py3-none-any.whl"));
        fs::write(&file, wheel).map_err(|error| error.to_string())?;
        let source = file.to_str().ok_or("临时目录路径无效")?;
        let online = run(
            uv,
            &["tool", "install", "--force", "--python", "3.12", source],
            dirs,
        );
        if online.is_ok() {
            return Ok(());
        }
        // The wheel is embedded but its dependency (`mcp`) is not, so an offline upgrade relies on
        // the wheels the previous install already put in uv's cache. Retry offline for any target
        // directory, not just a re-install of the same version, or a machine without a network
        // could never move to a new version.
        if run(
            uv,
            &[
                "tool",
                "install",
                "--force",
                "--offline",
                "--python",
                "3.12",
                source,
            ],
            dirs,
        )
        .is_ok()
        {
            return Ok(());
        }
        // Dependencies still missing from the cache: the direct index was unreachable, so try a
        // mirror before giving up. This is the only path that needs the network.
        let Some(mirrored) = with_mirror(dirs) else {
            return Err(format!(
                "{}（已保留指定的软件源，请检查其网络或依赖配置）",
                online.unwrap_err()
            ));
        };
        if run(
            uv,
            &["tool", "install", "--force", "--python", "3.12", source],
            &mirrored,
        )
        .is_ok()
        {
            return Ok(());
        }
        return Err(format!(
            "{}（首次安装需要联网下载依赖：直连与镜像均失败，可设置 UV_DEFAULT_INDEX 或使用代理）",
            online.unwrap_err()
        ));
    }
    let source = format!("git+{REPOSITORY}@v{version}");
    run(
        uv,
        &["tool", "install", "--force", "--python", "3.12", &source],
        dirs,
    )
}

pub(crate) fn extract_wheel(haystack: &[u8], version: &str) -> Option<Vec<u8>> {
    let mut prefix = Vec::with_capacity(20 + version.len());
    prefix.extend_from_slice(b"\x89ANWHEEL");
    prefix.extend_from_slice(b"\r\n\x1a\n\x00\x00\x00\x01");
    prefix.extend_from_slice(&(version.len() as u32).to_le_bytes());
    prefix.extend_from_slice(version.as_bytes());
    let pos = haystack
        .windows(prefix.len())
        .position(|window| window == prefix.as_slice())?;
    let len_start = pos + prefix.len();
    let len_bytes: [u8; 4] = haystack.get(len_start..len_start + 4)?.try_into().ok()?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    let start = len_start + 4;
    let wheel = haystack.get(start..start.checked_add(len)?)?;
    wheel.starts_with(b"PK\x03\x04").then(|| wheel.to_vec())
}

pub(crate) fn wheel_from_archive(archive: &[u8], version: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut decompressed = Vec::new();
    if flate2::read::GzDecoder::new(archive)
        .take(64 * 1024 * 1024)
        .read_to_end(&mut decompressed)
        .is_ok()
        && let Some(wheel) = extract_wheel(&decompressed, version)
    {
        return Some(wheel);
    }
    extract_wheel(archive, version)
}

/// uv on PATH, else where its installers put it.
pub(crate) fn find_uv() -> Option<PathBuf> {
    let name = if cfg!(windows) { "uv.exe" } else { "uv" };
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("bin").join(name));
        candidates.push(home.join(".cargo").join("bin").join(name));
    }
    // The winget package links uv here. A desktop app started before that PATH entry existed
    // inherits a PATH without it, so the install step would claim uv is missing.
    #[cfg(windows)]
    if let Some(local) = dirs::data_local_dir() {
        candidates.push(local.join("Microsoft/WinGet/Links").join(name));
        candidates.push(local.join("Programs/uv").join(name));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin").join(name));
    candidates.push(PathBuf::from("/usr/local/bin").join(name));
    candidates.into_iter().find(|path| path.is_file())
}

/// Read exactly the cached roots used by Hosts; GET/status never probes a shell.
pub(crate) fn service_dirs(app: &AppHandle) -> Option<ToolDirs> {
    app.state::<crate::Tray>().reader.tool_dirs()
}

pub(crate) fn service_for_app(app: &AppHandle) -> Value {
    // A versioned install is the truth once one exists; the shared uv tool directory is only
    // the legacy fallback.
    if let Some(version) = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|data| active_version(&data))
    {
        return outdated_note(version);
    }
    service(&service_dirs(app).map(|dirs| dirs.tools))
}

/// The upgrade note for an installed version that is older than this app.
pub(crate) fn outdated_note(installed: String) -> Value {
    if parse(&installed) >= parse(VERSION) {
        return Value::Null;
    }
    let command = format!("uv tool install --force --python 3.12 \"git+{REPOSITORY}@v{VERSION}\"");
    json!({"state": "outdated", "version": installed, "error": null, "command": command})
}

/// The launcher the hosts' configs point at once a versioned install exists.
pub(crate) fn active_launcher(app_data: &Path) -> Option<PathBuf> {
    active_entry(app_data).map(|(_, bin)| bin)
}

/// The active version and its launcher, only when both the launcher and the tool environment
/// behind it are still there. A half-removed install must not read as "installed".
pub(crate) fn active_entry(app_data: &Path) -> Option<(String, PathBuf)> {
    let raw = std::fs::read_to_string(app_data.join("service").join("active.json")).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let version = value["version"].as_str()?.to_owned();
    let bin = PathBuf::from(value["bin"].as_str()?);
    let env = app_data
        .join("service")
        .join(&version)
        .join("tools")
        .join("agentnave");
    (bin.is_file() && installed_version(&env).is_some()).then_some((version, bin))
}

/// The version the active manifest records, when its entry still exists.
/// The active version's tool environment, when the versioned install is complete. Callers that
/// report a service version must read this, not the legacy shared directory.
pub(crate) fn active_tools(app_data: &Path) -> Option<PathBuf> {
    active_entry(app_data).map(|(version, _)| {
        app_data
            .join("service")
            .join(version)
            .join("tools")
            .join("agentnave")
    })
}

pub(crate) fn active_version(app_data: &Path) -> Option<String> {
    active_entry(app_data).map(|(version, _)| version)
}

/// This version's own tool and bin directories, so an upgrade never touches the files the
/// running service executes (Windows refuses to remove them; POSIX would unlink them under it).
pub(crate) fn version_dirs(app: &AppHandle, dirs: &ToolDirs, version: &str) -> Option<ToolDirs> {
    let root = app
        .path()
        .app_data_dir()
        .ok()?
        .join("service")
        .join(version);
    Some(ToolDirs {
        home: dirs.home.clone(),
        tools: root.join("tools"),
        bin: root.join("bin"),
        network: dirs.network.clone(),
    })
}

/// Point the hosts at this install once it is complete.
pub(crate) fn write_active(app: &AppHandle, version: &str, dirs: &ToolDirs) -> Result<(), String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("service");
    std::fs::create_dir_all(&base).map_err(|error| error.to_string())?;
    let name = if cfg!(windows) {
        "agentnave-mcp.exe"
    } else {
        "agentnave-mcp"
    };
    let launcher = dirs.bin.join(name);
    let value = json!({"version": version, "bin": launcher.to_string_lossy()});
    write_active_manifest(&base, &value)
}

pub(crate) fn write_active_manifest(base: &Path, value: &Value) -> Result<(), String> {
    // Replace in one step: a truncated manifest would leave the hosts pointing at nothing. The
    // staging name is unique per process, and the swap is retried: on Windows a scanner holding
    // the old file for a moment rejects the replace, which must not fail a finished install.
    let staging = base.join(format!("active.{}.tmp", std::process::id()));
    std::fs::write(&staging, value.to_string()).map_err(|error| error.to_string())?;
    let target = base.join("active.json");
    let mut last = None;
    for attempt in 0..3 {
        match std::fs::rename(&staging, &target) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
            }
        }
    }
    let _ = std::fs::remove_file(&staging);
    Err(last.map_or_else(|| "无法写入 active.json".into(), |error| error.to_string()))
}

/// A note with the upgrade command when the installed service is older than this app.
pub(crate) fn service(tools: &Option<PathBuf>) -> Value {
    let Some(tools) = tools else {
        return Value::Null;
    };
    let Some(installed) = installed_version(&tools.join("agentnave")) else {
        return Value::Null;
    };
    if parse(&installed) >= parse(VERSION) {
        return Value::Null;
    }
    let command = format!("uv tool install --force --python 3.12 \"git+{REPOSITORY}@v{VERSION}\"");
    json!({"state": "outdated", "version": installed, "error": null, "command": command})
}

/// The version in `agentnave-<version>.dist-info` inside a uv tool environment.
pub(crate) fn installed_version(env: &Path) -> Option<String> {
    installed_metadata(env)?
        .file_name()?
        .to_str()?
        .strip_prefix("agentnave-")?
        .strip_suffix(".dist-info")
        .map(str::to_owned)
}

/// Recognize the unified runtime by its installed console-script contract, not an old core binary.
pub(crate) fn unified_service(tools: &Path) -> bool {
    installed_metadata(&tools.join("agentnave"))
        .and_then(|path| std::fs::read_to_string(path.join("entry_points.txt")).ok())
        .is_some_and(|text| {
            text.lines()
                .any(|line| line.trim() == "agentnave-mcp = agentnave.relay:main")
        })
}

pub(crate) fn installed_metadata(env: &Path) -> Option<PathBuf> {
    let mut queue = vec![(env.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("agentnave-") && name.ends_with(".dist-info") {
                return Some(entry.path());
            }
            // lib/python3.x/site-packages on POSIX, Lib/site-packages on Windows.
            if depth < 3 && entry.path().is_dir() && !name.starts_with('.') && name != "bin" {
                queue.push((entry.path(), depth + 1));
            }
        }
    }
    None
}

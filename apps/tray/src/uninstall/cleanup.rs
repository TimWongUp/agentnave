//! Prepare a detached OS helper before quitting; locked files are deliberately left in place.
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) struct Plan {
    pub(super) directories: Vec<PathBuf>,
    #[cfg(unix)]
    pub(super) bundle: Option<PathBuf>,
}

impl Plan {
    pub(super) fn new(
        app_directories: Vec<PathBuf>,
        history: &Path,
        delete_history: bool,
        home: &Path,
        executable: &Path,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        let bundle = app_bundle(executable);
        let mut directories = app_directories;
        if delete_history {
            // Validated as given: a relative data root must not resolve against the working directory.
            directories.push(history.to_path_buf());
        }
        let history = std::path::absolute(history).map_err(|e| e.to_string())?;
        directories.sort();
        directories.dedup();
        for dir in &directories {
            validate_directory(dir, home, executable)?;
        }
        if !delete_history
            && directories
                .iter()
                .any(|dir| resolved(&history).starts_with(resolved(dir)))
        {
            return Err("记录目录位于 App 数据目录内，无法保留记录；请先移出记录目录".into());
        }
        Ok(Self {
            directories,
            #[cfg(unix)]
            bundle,
        })
    }
}

fn resolved(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    // Resolve existing ancestors too: macOS /var -> /private/var and a missing final
    // directory must not let a parent of HOME pass the deletion guard.
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => path.to_path_buf(),
    }
}

fn validate_directory(dir: &Path, home: &Path, executable: &Path) -> Result<(), String> {
    let resolved = resolved(dir);
    // Compare HOME both as given and resolved: a symlinked HOME has two sets of ancestors.
    if !dir.is_absolute()
        || !home.is_absolute()
        || dir
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || resolved.parent().is_none()
        || home.starts_with(dir)
        || self::resolved(home).starts_with(&resolved)
        || self::resolved(executable).starts_with(&resolved)
    {
        return Err(format!("拒绝删除不安全的数据目录：{}", dir.display()));
    }
    Ok(())
}

#[cfg(any(unix, test))]
pub(super) fn app_bundle(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

#[cfg(unix)]
pub(super) fn spawn(plan: &Plan) -> Result<(), String> {
    command(plan, std::process::id())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法启动卸载清理：{e}"))
}

#[cfg(unix)]
pub(super) fn command(plan: &Plan, parent: u32) -> Command {
    use std::os::unix::process::CommandExt;
    let trash = if cfg!(target_os = "macos") {
        plan.bundle.as_deref()
    } else {
        None
    };
    // Arguments remain arguments, including apostrophes, spaces and shell metacharacters.
    // Foundation's trash operation does not need Finder automation permission.
    let script = r#"
while kill -0 "$1" 2>/dev/null; do sleep 1; done
shift
bundle=$1
shift
if [ -n "$bundle" ]; then
  /usr/bin/osascript -l JavaScript -e 'ObjC.import("Foundation"); function run(argv) { var resulting = Ref(); var error = Ref(); if (!$.NSFileManager.defaultManager.trashItemAtURLResultingItemURLError($.NSURL.fileURLWithPath(argv[0]), resulting, error)) throw Error("无法将 App 移入废纸篓"); }' "$bundle"
fi
for directory do
  /bin/rm -rf -- "$directory" 2>/dev/null || :
done
"#;
    let mut command = Command::new("/bin/sh");
    // Only setsid runs between fork and exec; it is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .args(["-c", script, "agentnave-uninstall"])
        .arg(parent.to_string())
        .arg(trash.unwrap_or_else(|| Path::new("")))
        .args(&plan.directories)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(std::env::temp_dir());
    command
}

#[cfg(windows)]
pub(super) fn spawn(plan: &Plan, installer: Option<(&Path, &str)>) -> Result<(), String> {
    use base64::Engine;
    use std::os::windows::process::CommandExt;
    // Windows PowerShell 5.1's recursive Remove-Item may traverse junctions. Walk ordinary
    // directories ourselves and delete each reparse point without following its target.
    let mut script = String::from(
        r#"
function Remove-AgentNaveTree([string]$path) {
  try {
    $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
      if ($item.PSIsContainer) { [IO.Directory]::Delete($path) } else { [IO.File]::Delete($path) }
    } elseif ($item.PSIsContainer) {
      foreach ($child in Get-ChildItem -LiteralPath $path -Force -ErrorAction Stop) { Remove-AgentNaveTree $child.FullName }
      [IO.Directory]::Delete($path)
    } else { [IO.File]::Delete($path) }
  } catch { }
}
"#,
    );
    script.push_str(&format!(
        "$p = Get-Process -Id {} -ErrorAction SilentlyContinue; if ($p) {{ $p.WaitForExit() }}; ",
        std::process::id()
    ));
    if let Some((path, token)) = installer {
        script.push_str(&format!(
            "Start-Process -FilePath {} -ArgumentList '/S','/AGENTNAVE_APPROVED={token}' -Wait; ",
            quote(path)
        ));
    }
    for dir in &plan.directories {
        script.push_str(&format!("Remove-AgentNaveTree {}; ", quote(dir)));
    }
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("无法定位 Windows 系统目录")?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(base64::engine::general_purpose::STANDARD.encode(bytes))
        .creation_flags(0x0000_0008) // DETACHED_PROCESS: survive the App's exit.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(std::env::temp_dir())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法启动卸载清理：{e}"))
}

#[cfg(windows)]
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

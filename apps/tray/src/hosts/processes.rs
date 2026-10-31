//! Read-only host process observations. Keep only host identities and start times in the cache.
use super::{Context, IDS};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Copy)]
pub(super) enum State {
    Stopped,
    Running(u64),
    Unknown,
}

#[derive(Default)]
pub(super) struct Probe {
    checked: Option<Instant>,
    running: Option<HashMap<&'static str, u64>>,
    #[cfg(test)]
    fixed: bool,
}

impl Probe {
    #[cfg(test)]
    pub fn fixed(running: Option<HashMap<&'static str, u64>>) -> Self {
        Self {
            checked: None,
            running,
            fixed: true,
        }
    }

    pub fn state(&mut self, context: &Context, id: &str) -> State {
        #[cfg(test)]
        let fixed = self.fixed;
        #[cfg(not(test))]
        let fixed = false;
        if !fixed
            && self
                .checked
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            self.running = observe(context);
            self.checked = Some(Instant::now());
        }
        match &self.running {
            Some(running) => running
                .get(id)
                .map_or(State::Stopped, |at| State::Running(*at)),
            None => State::Unknown,
        }
    }

    pub fn invalidate(&mut self) {
        self.checked = None;
    }
}

fn name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn identity(path: &str) -> Option<&'static str> {
    let path = path.replace('\\', "/").to_lowercase();
    for (app, id) in [
        ("codex", "codex"),
        ("chatgpt", "codex"),
        ("claude", "claude"),
        ("workbuddy", "workbuddy"),
        ("antigravity", "antigravity"),
        ("deepseek harness", "deepseek"),
        ("zcode", "zcode"),
    ] {
        if path.contains(&format!("/{app}.app/contents/macos/")) {
            return Some(id);
        }
    }
    if path.contains("/claude/versions/") {
        return Some("claude");
    }
    let file = name(&path);
    let stem = [".exe", ".js", ".mjs", ".cjs", ".py"]
        .into_iter()
        .find_map(|suffix| file.strip_suffix(suffix))
        .unwrap_or(file);
    match stem {
        "agy" => Some("antigravity"),
        "dsh" | "deepseek-harness" | "deepseek harness" => Some("deepseek"),
        other => IDS.into_iter().find(|id| *id == other),
    }
}

/// The script a Windows `.cmd` shim launches. Without it the shim path never matches the
/// `node.exe <script>.js` process that is actually running, so the host looks stopped.
/// A comparable form of one path: Windows spells the same file many ways (`C:\x`, `c:/x`,
/// `\\?\C:\x`, `\\?\UNC\server\share`), and a process argument rarely spells it the way
/// `canonicalize` does, so both sides go through this before matching.
fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let value = value.strip_prefix("//?/").unwrap_or(&value).to_owned();
    let value = value
        .strip_prefix("unc/")
        .map_or(value.clone(), |rest| format!("//{rest}"));
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
fn shim_script(shim: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .filter_map(|token| {
            let lower = token.to_lowercase();
            if !(lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs")) {
                return None;
            }
            let relative = token
                .replace("%~dp0", "")
                .replace("%dp0%", "")
                .replace('\\', "/");
            let path = dir.join(relative.trim_start_matches('/'));
            Some(path.canonicalize().unwrap_or(path))
        })
        .next()
}
fn agentnave(process: &Process) -> bool {
    // The executable itself living in the tool environment cannot be spoofed by an argument.
    if process
        .exe()
        .is_some_and(|path| tool_environment(&path.to_string_lossy()))
    {
        return true;
    }
    process
        .cmd()
        .iter()
        .take(6)
        .any(|arg| agentnave_arg(&arg.to_string_lossy()))
}

/// Whether a path is an executable inside this app's own installed environment.
fn tool_environment(path: &str) -> bool {
    path.replace('\\', "/").contains("/tools/agentnave/")
}

/// One command-line argument that names this app's server. Anchored on a whole path or on the
/// exact `-c` source the supervisor generates, so a prompt that merely mentions the module does
/// not turn its own process into "the app" and hide a real host.
fn agentnave_arg(arg: &str) -> bool {
    let arg = arg.replace('\\', "/");
    matches!(name(&arg), "agentnave-mcp" | "agentnave-mcp.exe")
        || arg.ends_with("/agentnave/supervisor.py")
        || arg.ends_with("/agentnave/windows_supervisor.py")
        || arg.ends_with("/agentnave/mcp_server.py")
        || arg.starts_with("from agentnave.mcp_server")
}

fn dispatched(process: &Process, system: &System) -> bool {
    let mut current = Some(process);
    for _ in 0..64 {
        let Some(process) = current else { break };
        if agentnave(process) {
            return true;
        }
        current = process.parent().and_then(|pid| system.process(pid));
    }
    false
}

fn observe(context: &Context) -> Option<HashMap<&'static str, u64>> {
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        return None;
    }
    // Resolve installed entry points so node/python CLIs with a generic script name also match.
    let mut targets: HashMap<String, &'static str> = HashMap::new();
    for id in IDS {
        let binary = match id {
            "antigravity" => "agy",
            "deepseek" => "dsh",
            _ => id,
        };
        let Some(found) = context.find(binary) else {
            continue;
        };
        targets.insert(path_key(&found), id);
        if let Ok(path) = found.canonicalize() {
            targets.insert(path_key(&path), id);
        }
        // Windows installs a CLI as a `.cmd` shim; the live process is `node <script>.js`.
        if let Some(script) = shim_script(&found) {
            targets.insert(path_key(&script), id);
        }
    }
    let identify = |path: &Path| {
        targets
            .get(&path_key(path))
            .copied()
            .or_else(|| identity(&path.to_string_lossy()))
    };
    let mut system = System::new();
    let count = system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always)
            .with_user(UpdateKind::Always),
    );
    if count == 0 {
        return None;
    }
    let mut running = HashMap::new();
    let user = system
        .process(sysinfo::Pid::from_u32(std::process::id()))?
        .user_id()?;
    for process in system.processes().values() {
        if process.user_id() != Some(user) {
            continue;
        }
        let host = process.exe().and_then(identify).or_else(|| {
            // Only an executable or its script argument identifies a host; prompt text cannot.
            process
                .cmd()
                .first()
                .and_then(|p| identify(Path::new(p)))
                .or_else(|| {
                    let exe = process.exe()?.file_stem()?.to_str()?.to_lowercase();
                    if !matches!(
                        exe.as_str(),
                        "node" | "bun" | "deno" | "python" | "python3" | "uv" | "uvx" | "npx"
                    ) && !exe.starts_with("python3.")
                    {
                        return None;
                    }
                    process
                        .cmd()
                        .iter()
                        .skip(1)
                        .find(|p| !p.to_string_lossy().starts_with('-'))
                        .and_then(|p| identify(Path::new(p)))
                })
        });
        if let Some(id) = host
            && !dispatched(process, &system)
        {
            running
                .entry(id)
                .and_modify(|at: &mut u64| *at = (*at).min(process.start_time()))
                .or_insert(process.start_time());
        }
    }
    Some(running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_canonical_path_and_a_plain_one_look_the_same() {
        assert_eq!(
            path_key(Path::new(r"D:\x\pi-launcher.js")),
            path_key(Path::new(r"\\?\D:\x\pi-launcher.js"))
        );
        assert_eq!(
            path_key(Path::new(r"\\?\UNC\srv\share\a.js")),
            path_key(Path::new(r"\\srv\share\a.js"))
        );
        assert_ne!(
            path_key(Path::new(r"D:\a.js")),
            path_key(Path::new(r"D:\b.js"))
        );
    }
    #[test]
    fn a_cmd_shim_points_at_the_script_that_is_actually_running() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("pi.cmd");
        std::fs::write(
            &shim,
            "@ECHO off\nSETLOCAL\n\"%node%\" \"%~dp0pi-launcher.js\" %*\n",
        )
        .unwrap();
        let script = dir.path().join("pi-launcher.js");
        std::fs::write(&script, "").unwrap();
        assert_eq!(shim_script(&shim), Some(script.canonicalize().unwrap()));
    }
    #[test]
    fn our_own_server_and_supervisor_are_recognized() {
        for arg in [
            r"C:\Users\timwo\AppData\Roaming\io.github.timwongup.agentnave\service\0.58.0\tools\agentnave\Scripts\python.exe",
            r"C:\Users\timwo\.local\bin\agentnave-mcp.exe",
            "from agentnave.mcp_server import main; main()",
            r"C:\x\agentnave\mcp_server.py",
            r"C:\x\agentnave\windows_supervisor.py",
        ] {
            assert!(
                agentnave_arg(arg) || tool_environment(arg),
                "{arg} should be ours"
            );
        }
    }

    /// Only a whole path or the generated `-c` source counts. A prompt that merely mentions the
    /// module must not turn the user's own host into "the app": that would hide a running host.
    #[test]
    fn text_that_mentions_the_module_is_not_this_app() {
        for arg in [
            r"D:\code\agentnave",
            r"C:\Users\timwo\Documents\agentnave notes.md",
            r"C:\tools\other\python.exe",
            "check agentnave.mcp_server in the tray",
            r"see C:\x\tools\agentnave\Scripts\python.exe for details",
        ] {
            assert!(!agentnave_arg(arg), "{arg}");
        }
    }

    /// Only the executable path decides the tool environment, so a prompt cannot claim it.
    #[test]
    fn the_tool_environment_is_matched_by_executable_path() {
        assert!(tool_environment(
            r"C:\Users\timwo\AppData\Roaming\io.github.timwongup.agentnave\service\0.58.0\tools\agentnave\Scripts\python.exe"
        ));
        assert!(!tool_environment(r"D:\code\agentnave"));
        assert!(!tool_environment(r"C:\tools\other\python.exe"));
    }

    #[test]
    fn host_paths_match_exact_executables_and_main_app_processes() {
        for (path, id) in [
            (
                "/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness",
                "deepseek",
            ),
            (
                "C:\\Program Files\\DeepSeek Harness\\DeepSeek Harness.exe",
                "deepseek",
            ),
            ("/Applications/Codex.app/Contents/MacOS/Codex", "codex"),
            ("/home/a/.local/share/claude/versions/2.0.0", "claude"),
            ("/usr/bin/agy", "antigravity"),
            ("/usr/bin/dsh.js", "deepseek"),
        ] {
            assert_eq!(identity(path), Some(id), "{path}");
        }
        for path in [
            "/tmp/codex-notes.txt",
            "/Applications/DeepSeek Harness.app/Contents/Frameworks/DeepSeek Harness Helper.app/Contents/MacOS/DeepSeek Harness Helper",
            "deepseek.example",
        ] {
            assert_eq!(identity(path), None, "{path}");
        }
    }
}

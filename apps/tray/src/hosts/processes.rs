//! Read-only host process observations. Keep only host identities and start times in the cache.
use super::{Context, IDS};
use std::{
    collections::HashMap,
    path::Path,
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

fn agentnave(process: &Process) -> bool {
    process.cmd().iter().take(4).any(|arg| {
        let arg = arg.to_string_lossy().replace('\\', "/");
        matches!(name(&arg), "agentnave-mcp" | "agentnave-mcp.exe")
            || arg.ends_with("/agentnave/supervisor.py")
            || arg.ends_with("/agentnave/windows_supervisor.py")
    })
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
    let targets: HashMap<_, _> = IDS
        .into_iter()
        .filter_map(|id| {
            let binary = match id {
                "antigravity" => "agy",
                "deepseek" => "dsh",
                _ => id,
            };
            Some((context.find(binary)?.canonicalize().ok()?, id))
        })
        .collect();
    let identify = |path: &Path| {
        targets
            .get(path)
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
                    let exe = process.exe()?.file_stem()?.to_str()?;
                    if !matches!(
                        exe,
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

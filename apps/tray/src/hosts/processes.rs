//! Read-only process observations. Cache host identities and PID start times together.
use super::{Context, IDS};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant, SystemTime},
};
use sysinfo::{Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
#[cfg(unix)]
mod copies;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Copy {
    pub kind: &'static str,
    /// Unix milliseconds, at sysinfo's one-second resolution.
    pub since: u64,
    pub bundle: Option<String>,
}
impl Copy {
    pub fn into_json(self) -> serde_json::Value {
        let mut value = serde_json::json!({"kind":self.kind,"since":self.since});
        if let Some(bundle) = self.bundle {
            value["bundle"] = serde_json::json!(bundle);
        }
        value
    }
}

#[derive(Clone, Copy)]
pub(super) enum State {
    Stopped,
    Running(u64),
    Unknown,
}

#[derive(Default)]
pub(super) struct Probe {
    checked: Option<Instant>,
    observed_at: Option<SystemTime>,
    running: Option<HashMap<&'static str, u64>>,
    receipt_processes: Option<HashMap<u32, u64>>,
    #[cfg(unix)]
    copies: Option<copies::Snapshot>,
    #[cfg(test)]
    fixed: bool,
}

impl Probe {
    #[cfg(test)]
    pub fn fixed(running: Option<HashMap<&'static str, u64>>) -> Self {
        Self {
            running,
            fixed: true,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn with_receipt_processes(mut self, processes: Option<HashMap<u32, u64>>) -> Self {
        self.receipt_processes = processes;
        self
    }

    fn is_fixed(&self) -> bool {
        #[cfg(test)]
        {
            self.fixed
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    fn refresh(&mut self, context: &Context) {
        if !self.is_fixed()
            && self
                .checked
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            // Capture the beginning, not the end: a relay can start during enumeration.
            self.observed_at = Some(SystemTime::now());
            match observe(context) {
                Some(observation) => {
                    self.running = observation.running;
                    self.receipt_processes = Some(observation.processes);
                    #[cfg(unix)]
                    {
                        self.copies = Some(observation.copies);
                    }
                }
                None => {
                    self.running = None;
                    self.receipt_processes = None;
                    #[cfg(unix)]
                    {
                        self.copies = None;
                    }
                }
            }
            self.checked = Some(Instant::now());
        }
    }

    pub fn state(&mut self, context: &Context, id: &str) -> State {
        self.refresh(context);
        match &self.running {
            Some(running) => running
                .get(id)
                .map_or(State::Stopped, |at| State::Running(*at)),
            None => State::Unknown,
        }
    }

    pub fn receipt_process(
        &mut self,
        context: &Context,
        pid: u32,
        started_ms: Option<u64>,
        modified: SystemTime,
    ) -> State {
        self.refresh(context);
        let Some(processes) = &self.receipt_processes else {
            return State::Unknown;
        };
        // A newer receipt may name a relay that was not visible in this snapshot yet.
        // Keep the shared five-second cache and defer negative results until it catches up.
        let can_stop = self.is_fixed() || self.observed_at.is_some_and(|at| at >= modified);
        match processes.get(&pid).copied() {
            Some(at)
                if at != 0
                    && started_ms.is_some_and(|started| {
                        started != 0 && at > (started / 1000).saturating_add(2)
                    }) =>
            {
                if can_stop {
                    State::Stopped
                } else {
                    State::Unknown
                }
            }
            Some(at) => State::Running(at),
            None if can_stop => State::Stopped,
            None => State::Unknown,
        }
    }

    pub fn invalidate(&mut self) {
        self.checked = None;
    }

    /// Classify only observed live relays; unknown or reused PIDs supply no copy evidence.
    pub fn receipt_copy(&mut self, context: &Context, id: &str, pid: u32) -> Option<Copy> {
        self.refresh(context);
        #[cfg(unix)]
        {
            self.copies.as_ref()?.classify(id, pid, true)
        }
        #[cfg(not(unix))]
        {
            let _ = (id, pid);
            None
        }
    }

    pub fn old_copies(&mut self, context: &Context, id: &str, before: u64) -> Vec<Copy> {
        self.refresh(context);
        #[cfg(unix)]
        {
            self.copies
                .as_ref()
                .map_or_else(Vec::new, |s| s.old_copies(id, before))
        }
        #[cfg(not(unix))]
        {
            let _ = (id, before);
            Vec::new()
        }
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

/// A comparable form of one path: Windows spells the same file many ways (`C:\x`, `c:/x`,
/// `\\?\C:\x`, `\\?\UNC\server\share`), and a process argument rarely spells it the way
/// `canonicalize` does, so both sides go through this before matching.
fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let value = value.strip_prefix("//?/").unwrap_or(&value).to_owned();
    let value = if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    };
    // `\\?\UNC\srv\share` names the same place as `\\srv\share`; lower-casing comes first so the
    // prefix matches whatever case the caller used.
    value
        .strip_prefix("unc/")
        .map_or(value.clone(), |rest| format!("//{rest}"))
}
#[cfg(windows)]
fn shim_script(shim: &Path) -> Option<std::path::PathBuf> {
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    // npm also launches extensionless scripts (CodeBuddy). Preserve spaces in quoted paths,
    // resolve only shim-relative tokens, and reject the bundled node executable itself.
    text.split(['"', '\''])
        .chain(text.split_whitespace())
        .find_map(|token| {
            let lower = token.to_ascii_lowercase();
            let offset = if lower.starts_with("%~dp0") || lower.starts_with("%dp0%") {
                5
            } else {
                return None;
            };
            let relative = token[offset..].replace('\\', "/");
            let path = dir.join(relative.trim_start_matches('/'));
            if path.extension().is_some_and(|ext| {
                ["exe", "cmd", "bat"]
                    .iter()
                    .any(|s| ext.eq_ignore_ascii_case(s))
            }) || !path.is_file()
            {
                return None;
            }
            path.canonicalize().ok()
        })
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
    // This is a snapshot heuristic, not durable provenance: if a supervisor/ancestor exits
    // before the snapshot (or the chain exceeds 64), an orphan CLI can look like a host.
    // Never use this observation as an authorization or process-ownership boundary.
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

#[derive(Debug)]
struct Observation {
    running: Option<HashMap<&'static str, u64>>,
    processes: HashMap<u32, u64>,
    #[cfg(unix)]
    copies: copies::Snapshot,
}

fn observe(context: &Context) -> Option<Observation> {
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        return None;
    }
    let mut system = System::new();
    let count = system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always)
            .with_user(UpdateKind::Always),
    );
    if count == 0
        || system
            .process(sysinfo::Pid::from_u32(std::process::id()))
            .is_none()
    {
        return None;
    }
    let hosts = observe_hosts(context, &system);
    Some(Observation {
        running: hosts.as_ref().map(|hosts| hosts.running.clone()),
        #[cfg(unix)]
        copies: copies::Snapshot::new(&system, hosts.map(|hosts| hosts.pids).unwrap_or_default()),
        // Missing executable, arguments or user information never means a PID is dead.
        processes: system
            .processes()
            .iter()
            .map(|(pid, process)| (pid.as_u32(), process.start_time()))
            .collect(),
    })
}

struct HostProcesses {
    running: HashMap<&'static str, u64>,
    #[cfg(unix)]
    pids: HashMap<&'static str, Vec<u32>>,
}

fn host_targets(context: &Context) -> HashMap<String, &'static str> {
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
        #[cfg(windows)]
        if let Some(script) = shim_script(&found) {
            targets.insert(path_key(&script), id);
        }
    }
    targets
}

fn process_identity(
    process: &Process,
    targets: &HashMap<String, &'static str>,
) -> Option<&'static str> {
    let identify = |path: &Path| {
        targets
            .get(&path_key(path))
            .copied()
            .or_else(|| identity(&path.to_string_lossy()))
    };
    process.exe().and_then(identify).or_else(|| {
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
    })
}

fn observe_hosts(context: &Context, system: &System) -> Option<HostProcesses> {
    let targets = host_targets(context);
    let mut running = HashMap::new();
    #[cfg(unix)]
    let mut pids: HashMap<&'static str, Vec<u32>> = HashMap::new();
    let user = system
        .process(sysinfo::Pid::from_u32(std::process::id()))?
        .user_id()?;
    for process in system.processes().values() {
        if process.user_id() != Some(user) {
            continue;
        }
        let host = process_identity(process, &targets);
        if let Some(id) = host
            && !dispatched(process, system)
        {
            #[cfg(unix)]
            {
                if id == "codex"
                    && copies::daemon_helper(process.cmd().iter().map(|a| a.to_string_lossy()))
                {
                    continue;
                }
                pids.entry(id).or_default().push(process.pid().as_u32());
            }
            running
                .entry(id)
                .and_modify(|at: &mut u64| *at = (*at).min(process.start_time()))
                .or_insert(process.start_time());
        }
    }
    Some(HostProcesses {
        running,
        #[cfg(unix)]
        pids,
    })
}

#[cfg(test)]
mod tests;

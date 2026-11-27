//! Startup environment sanitization and resilient temporary-directory helpers.
//!
//! When the tray app is launched from a short-lived agent subshell (such as T3 Code or
//! Antigravity), it can inherit a session-scoped `TMPDIR` (or `TMP`/`TEMP`) that disappears once
//! that turn ends, as well as parent-session variables. Sanitizing at single-threaded startup and
//! routing temp paths through [`usable`] prevents background probes and subprocesses from failing
//! with `ENOENT`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use portable_pty::CommandBuilder;

static INHERITED: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Exact environment variable names that identify a parent agent/host session.
///
/// Ordinary terminal variables (`PWD`, `OLDPWD`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`) are
/// intentionally excluded here so launching from a regular terminal does not reset a valid
/// user-configured temp directory.
const AGENT_SESSION_EXACT: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_PROJECT_DIR",
    // Claude session-scoped variables aligned with `src/agentnave/adapters/claude.py`'s
    // `HOST_SESSION_ENV` plus `CLAUDE_CODE_SSE_PORT` and `CLAUDE_CODE_OAUTH_TOKEN`.
    // Config/gateway variables such as `CLAUDE_CODE_USE_BEDROCK` are never listed here.
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_DESKTOP_APP_VERSION",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "GROK_SESSION_ID",
    "GROK_SANDBOX",
    "ELECTRON_RUN_AS_NODE",
    "AGENTNAVE_HOST",
    "AGENTNAVE_HOST_APP",
    "AGENTNAVE_HOST_SESSION_ID",
    "AGENTNAVE_EXCLUDED_PROVIDERS",
    "AGENTNAVE_MANAGER_SESSION",
    "AGENTNAVE_MANAGER_STARTED_AT",
    "AGENTNAVE_DISPATCHED",
    "AGENTNAVE_DERIVED_SESSION",
    "AGENTNAVE_HANDOVER_PROMPT",
    "SERVER__PORT",
    "SERVER__HOST",
];

/// Variables stripped at startup and from background children without triggering a temp-directory
/// reset on their own (regular terminal vars and persistent auth tokens a user may export globally).
const STRIP_ONLY_EXACT: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "PWD",
    "OLDPWD",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
];

/// Cross-platform temporary-directory environment variable names checked by Rust, Python, Node,
/// and shell tools.
const TEMP_ENV_KEYS: &[&str] = &["TMPDIR", "TEMP", "TMP"];

/// Original temp-directory paths inherited at process startup (before sanitization), used for
/// scratch-directory classification.
pub(crate) fn inherited() -> &'static [PathBuf] {
    INHERITED.get().map_or(&[], Vec::as_slice)
}

/// Whether `name` marks an inherited parent agent/host session that warrants resetting temp env.
fn is_agent_session_trigger(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    (upper.starts_with("ANTIGRAVITY_") && upper != "ANTIGRAVITY_CLI_HOME")
        || upper.starts_with("HERDR_")
        || upper.starts_with("T3_ACP_MCP_")
        || (upper.starts_with("T3CODE_") && upper != "T3CODE_HOME")
        || AGENT_SESSION_EXACT.contains(&upper.as_str())
}

/// The loopback `SSH_CONNECTION` T3 Code sets for its Antigravity ACP profile, so the CLI prints
/// its sign-in URL; a real SSH login's value is kept.
const T3_SSH_CONNECTION: &str = "127.0.0.1 0 127.0.0.1 0";
/// What T3 Code's `BROWSER` stub carries to capture that URL; a user's own `BROWSER` is kept.
const T3_BROWSER_MARKER: &str = "__T3_ANTIGRAVITY_AUTH_URL__";

/// How a startup variable relates to the parent session that launched the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParentMark {
    /// Not a parent-session variable.
    Keep,
    /// Stripped without resetting temp env on its own.
    Strip,
    /// Stripped, and proves the process came from a session whose temp dir may vanish.
    Trigger,
}

/// What the whole inherited environment says about where it came from, judged once before any
/// variable is removed so `GEMINI_HOME`'s origin can rest on markers removed with it.
pub(crate) struct ParentEnv {
    t3_antigravity_profile: bool,
}

impl ParentEnv {
    pub(crate) fn new<K: AsRef<OsStr>, V: AsRef<OsStr>>(
        vars: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut app_data = None;
        let mut file_storage = false;
        let mut gemini_home = None;
        let mut t3_home = None;
        for (name, value) in vars {
            let (Some(name), value) = (name.as_ref().to_str(), value.as_ref()) else {
                continue;
            };
            let present = !value.to_string_lossy().trim().is_empty();
            match name.to_ascii_uppercase().as_str() {
                "ANTIGRAVITY_APP_DATA_DIR" if present => app_data = Some(PathBuf::from(value)),
                "AGY_ACP_FORCE_FILE_STORAGE" => file_storage |= present,
                "GEMINI_HOME" if present => gemini_home = Some(PathBuf::from(value)),
                "T3CODE_HOME" if present => t3_home = Some(PathBuf::from(value)),
                _ => {}
            }
        }
        let acp_app_data =
            app_data.is_some_and(|dir| dir.file_name() == Some("antigravity-acp".as_ref()));
        let t3_profile_home = gemini_home.is_some_and(|home| {
            t3_home
                .or_else(|| dirs::home_dir().map(|home| home.join(".t3")))
                .is_some_and(|t3| home.starts_with(t3.join("userdata/providers/antigravity")))
        });
        Self {
            t3_antigravity_profile: acp_app_data || file_storage || t3_profile_home,
        }
    }

    /// The current process environment.
    pub(crate) fn current() -> Self {
        Self::new(std::env::vars_os())
    }

    fn mark(&self, name: &str, value: &OsStr) -> ParentMark {
        let upper = name.to_ascii_uppercase();
        let t3_injected = match upper.as_str() {
            "SSH_CONNECTION" => value == T3_SSH_CONNECTION,
            "BROWSER" => value.to_string_lossy().contains(T3_BROWSER_MARKER),
            "AGY_ACP_FORCE_FILE_STORAGE" => true,
            _ => false,
        };
        if t3_injected || is_agent_session_trigger(&upper) {
            ParentMark::Trigger
        } else if STRIP_ONLY_EXACT.contains(&upper.as_str())
            || (upper == "GEMINI_HOME" && self.t3_antigravity_profile)
        {
            ParentMark::Strip
        } else {
            ParentMark::Keep
        }
    }

    /// Whether `name=value` is a parent-session variable stripped at startup and from background
    /// children.
    ///
    /// Preserves user configuration, gateway, and credential variables such as `T3CODE_HOME`,
    /// `ANTIGRAVITY_CLI_HOME`, `AGENTNAVE_DATA_DIR`, `CODEX_HOME`, `CLAUDE_CONFIG_DIR`,
    /// `PI_CODING_AGENT_DIR`, `CODEBUDDY_CONFIG_DIR`, `ZCODE_DATA_BASE_DIR`,
    /// `CLAUDE_CODE_USE_BEDROCK`, and `CLAUDE_CODE_USE_VERTEX`, as well as a real SSH login's
    /// `SSH_CONNECTION`, a user's `BROWSER` and a `GEMINI_HOME` outside T3 Code's profile.
    pub(crate) fn is_parent_session(&self, name: &str, value: &OsStr) -> bool {
        self.mark(name, value) != ParentMark::Keep
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StartupEnvPlan {
    pub(crate) reset_temp: bool,
    pub(crate) remove_vars: Vec<OsString>,
}

pub(crate) fn plan_startup_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    current_temp_dir: &Path,
) -> StartupEnvPlan {
    let vars: Vec<_> = vars.into_iter().collect();
    let parent = ParentEnv::new(vars.iter().map(|(name, value)| (name, value)));
    let mut from_agent_session = false;
    let mut remove_vars = Vec::new();
    for (name, value) in vars {
        let Some(text) = name.to_str() else {
            continue;
        };
        match parent.mark(text, &value) {
            ParentMark::Trigger => {
                from_agent_session = true;
                remove_vars.push(name);
            }
            ParentMark::Strip => remove_vars.push(name),
            ParentMark::Keep => {}
        }
    }
    StartupEnvPlan {
        reset_temp: !current_temp_dir.is_dir() || from_agent_session,
        remove_vars,
    }
}

/// Record inherited temp paths, reset temp env when needed, and strip parent-session variables.
///
/// Must be called at the very start of `main()` while the process is strictly single-threaded.
pub(crate) fn sanitize_startup_env() {
    let mut original = Vec::new();
    for key in TEMP_ENV_KEYS {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            let path = PathBuf::from(value);
            if !original.contains(&path) {
                original.push(path);
            }
        }
    }
    let _ = INHERITED.set(original);

    let plan = plan_startup_env(std::env::vars_os(), &std::env::temp_dir());

    // SAFETY: `main` calls this before starting any other thread, so no other thread reads the
    // environment while it changes.
    unsafe {
        if plan.reset_temp {
            reset_process_temp_env();
        } else {
            repair_invalid_temp_env_vars();
        }
        for name in plan.remove_vars {
            std::env::remove_var(name);
        }
    }
}

/// Repair any secondary temp-directory variables (`TEMP`/`TMP` on Unix, `TMPDIR` on Windows) that
/// point to non-existent directories even when the primary temp directory is valid.
///
/// # Safety
/// Caller must ensure no other thread is concurrently reading or mutating the process environment.
unsafe fn repair_invalid_temp_env_vars() {
    for key in TEMP_ENV_KEYS {
        let Some(value) = std::env::var_os(key) else {
            continue;
        };
        if !value.is_empty() && Path::new(&value).is_dir() {
            continue;
        }
        #[cfg(unix)]
        // SAFETY: forwarded from caller's single-threaded startup guarantee.
        unsafe {
            std::env::remove_var(key);
        }
        #[cfg(windows)]
        // SAFETY: forwarded from caller's single-threaded startup guarantee.
        unsafe {
            if key.eq_ignore_ascii_case("TMPDIR") {
                std::env::remove_var(key);
            } else if let Some(dir) = windows_persistent_temp_dir() {
                std::env::set_var(key, dir);
            }
        }
    }
}

/// Reset the process's temp-directory environment variables to the platform's persistent default.
///
/// # Safety
/// Caller must ensure no other thread is concurrently reading or mutating the process environment.
unsafe fn reset_process_temp_env() {
    #[cfg(target_os = "macos")]
    {
        let dir = darwin_user_temp_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        // SAFETY: forwarded from caller's single-threaded startup guarantee.
        unsafe {
            std::env::set_var("TMPDIR", dir);
            std::env::remove_var("TEMP");
            std::env::remove_var("TMP");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // SAFETY: forwarded from caller's single-threaded startup guarantee.
        unsafe {
            std::env::remove_var("TMPDIR");
            std::env::remove_var("TEMP");
            std::env::remove_var("TMP");
        }
        if !std::env::temp_dir().is_dir() {
            let dir = xdg_runtime_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
            // SAFETY: forwarded from caller's single-threaded startup guarantee.
            unsafe { std::env::set_var("TMPDIR", dir) };
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: forwarded from caller's single-threaded startup guarantee.
        unsafe { std::env::remove_var("TMPDIR") };
        if let Some(dir) = windows_persistent_temp_dir() {
            // SAFETY: forwarded from caller's single-threaded startup guarantee.
            unsafe {
                std::env::set_var("TMP", &dir);
                std::env::set_var("TEMP", &dir);
            }
        }
    }
}

/// Return a verified existing temporary directory, falling back to platform defaults if
/// `std::env::temp_dir()` no longer exists.
pub(crate) fn usable() -> PathBuf {
    resolve_usable_in(std::env::temp_dir(), platform_fallbacks())
}

pub(crate) fn resolve_usable_in(
    current: PathBuf,
    fallbacks: impl IntoIterator<Item = PathBuf>,
) -> PathBuf {
    if current.is_dir() {
        return current;
    }
    fallbacks
        .into_iter()
        .find(|candidate| candidate.is_dir())
        .unwrap_or(current)
}

fn platform_fallbacks() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut list = Vec::with_capacity(2);
        if let Some(dir) = darwin_user_temp_dir() {
            list.push(dir);
        }
        list.push(PathBuf::from("/tmp"));
        list
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut list = Vec::with_capacity(2);
        if let Some(dir) = xdg_runtime_dir() {
            list.push(dir);
        }
        list.push(PathBuf::from("/tmp"));
        list
    }
    #[cfg(windows)]
    {
        windows_temp_candidates()
    }
}

#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    // SAFETY: `confstr` with a null buffer and zero length queries the required buffer size.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if len <= 1 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is a valid writable buffer of `len` bytes.
    let written = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
        )
    };
    if written <= 1 || written > buf.len() {
        return None;
    }
    buf.truncate(written - 1);
    let path = PathBuf::from(OsString::from_vec(buf));
    path.is_dir().then_some(path)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

#[cfg(windows)]
fn windows_temp_candidates() -> Vec<PathBuf> {
    let mut list = Vec::with_capacity(2);
    if let Some(local) = dirs::data_local_dir() {
        list.push(local.join("Temp"));
    }
    if let Some(root) = std::env::var_os("SystemRoot").filter(|v| !v.is_empty()) {
        list.push(PathBuf::from(root).join("Temp"));
    }
    list
}

#[cfg(windows)]
fn windows_persistent_temp_dir() -> Option<PathBuf> {
    windows_temp_candidates().into_iter().find(|p| p.is_dir())
}

pub(crate) fn invalid_temp_overrides_in(
    get_var: impl Fn(&str) -> Option<OsString>,
    fallback: impl FnOnce() -> PathBuf,
) -> Vec<(&'static str, PathBuf)> {
    let mut broken = Vec::new();
    for key in TEMP_ENV_KEYS {
        if let Some(value) = get_var(key)
            && !Path::new(&value).is_dir()
        {
            broken.push(*key);
        }
    }
    if broken.is_empty() {
        return Vec::new();
    }
    let replacement = fallback();
    broken
        .into_iter()
        .map(|key| (key, replacement.clone()))
        .collect()
}

/// If the current process's temp env var is set to a non-existent directory, override it on `cmd`
/// with [`usable`]. Leaves unset or valid temp env vars untouched.
pub(crate) fn apply_to_command(cmd: &mut Command) {
    for (key, dir) in invalid_temp_overrides_in(|k| std::env::var_os(k), usable) {
        cmd.env(key, dir);
    }
}

/// Same as [`apply_to_command`] for a PTY [`CommandBuilder`].
pub(crate) fn apply_to_pty(cmd: &mut CommandBuilder) {
    for (key, dir) in invalid_temp_overrides_in(|k| std::env::var_os(k), usable) {
        cmd.env(key, dir);
    }
}

/// Remove startup parent-session variables from `cmd` and repair any invalid temp env var.
pub(crate) fn strip_child_env(cmd: &mut Command) {
    let parent = ParentEnv::current();
    for (name, value) in std::env::vars_os() {
        if name
            .to_str()
            .is_some_and(|text| parent.is_parent_session(text, &value))
        {
            cmd.env_remove(name);
        }
    }
    apply_to_command(cmd);
}

fn is_temp_env_key(name: &str) -> bool {
    #[cfg(windows)]
    {
        TEMP_ENV_KEYS
            .iter()
            .any(|key| name.eq_ignore_ascii_case(key))
    }
    #[cfg(not(windows))]
    {
        TEMP_ENV_KEYS.contains(&name)
    }
}

/// Filter parent-session variables out of an explicit environment map and replace any temp env var
/// pointing to a non-existent directory with `fallback`.
pub(crate) fn sanitize_env_map_with(
    env: &BTreeMap<String, String>,
    fallback: impl FnOnce() -> PathBuf,
) -> BTreeMap<String, String> {
    let mut cached_fallback: Option<String> = None;
    let mut fallback = Some(fallback);
    let parent = ParentEnv::new(env);
    let mut out = BTreeMap::new();
    for (key, value) in env {
        if parent.is_parent_session(key, value.as_ref()) {
            continue;
        }
        if is_temp_env_key(key) && !Path::new(value).is_dir() {
            let replacement = cached_fallback.get_or_insert_with(|| {
                fallback
                    .take()
                    .map(|f| f().to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            out.insert(key.clone(), replacement.clone());
        } else {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Convenience wrapper around [`sanitize_env_map_with`] using [`usable`].
pub(crate) fn sanitize_env_map(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    sanitize_env_map_with(env, usable)
}

#[cfg(test)]
mod tests;

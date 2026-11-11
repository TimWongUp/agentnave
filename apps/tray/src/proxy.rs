//! System and shell proxy resolution, plus shared release mirror expansion.
//!
//! Desktop apps started from Finder or the Windows shell do not inherit terminal `HTTPS_PROXY`
//! variables. Windows keeps its system proxy in the WinINET registry; macOS keeps it in DynamicStore
//! (`scutil --proxy`). Reading both lets the updater, video-pack downloader, and child CLIs use the
//! user's active proxy without manual configuration.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// Public mirrors that serve GitHub release files when GitHub itself is unreachable.
pub const GITHUB_MIRRORS: [&str; 2] = ["https://ghfast.top/", "https://gh-proxy.com/"];

const PROXY_KEYS: [&str; 6] = [
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

const NO_PROXY_KEYS: [&str; 2] = ["NO_PROXY", "no_proxy"];

static SHELL_NETWORK: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();

/// Cache the login shell's network variables detected by `hosts`, so background downloads can
/// use a proxy configured only in `.zshrc` or `.bashrc`.
pub fn cache_network(network: &BTreeMap<String, String>) {
    let mutex = SHELL_NETWORK.get_or_init(Mutex::default);
    *mutex.lock().unwrap_or_else(|p| p.into_inner()) = network.clone();
}

/// Copy the system proxy into this process's environment at startup, unless the user already set one.
pub fn apply() {
    if PROXY_KEYS
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
    {
        return;
    }
    let Some(proxy) = system_proxy_url() else {
        return;
    };
    // SAFETY: `main` calls this before starting any other thread, so no other thread reads the
    // environment while it changes.
    unsafe {
        if proxy.starts_with("socks5://") || proxy.starts_with("socks5h://") {
            std::env::set_var("ALL_PROXY", &proxy);
        }
        std::env::set_var("HTTPS_PROXY", &proxy);
        std::env::set_var("HTTP_PROXY", &proxy);
    }
}

/// Resolve the active proxy URL from an explicit network map, cached login-shell environment,
/// process environment, or live OS system proxy settings.
pub fn resolve(network: Option<&BTreeMap<String, String>>) -> Option<tauri::Url> {
    if let Some(map) = network
        && let Some(url) = env_proxy_url(map)
    {
        return Some(url);
    }
    if let Some(mutex) = SHELL_NETWORK.get()
        && let Some(url) = env_proxy_url(&mutex.lock().unwrap_or_else(|p| p.into_inner()))
    {
        return Some(url);
    }
    let env: BTreeMap<String, String> = std::env::vars().collect();
    if let Some(url) = env_proxy_url(&env) {
        return Some(url);
    }
    system_proxy_url().and_then(|raw| tauri::Url::parse(&raw).ok())
}

/// Read the first valid proxy URL from an environment variable map.
pub fn env_proxy_url(env: &BTreeMap<String, String>) -> Option<tauri::Url> {
    for key in PROXY_KEYS {
        if let Some(raw) = env.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
            && let Ok(url) = tauri::Url::parse(raw)
        {
            return Some(url);
        }
    }
    None
}

/// The hosts that bypass the proxy, read with the same precedence as `resolve`.
pub fn resolve_no_proxy(network: Option<&BTreeMap<String, String>>) -> Option<String> {
    fn from(env: &BTreeMap<String, String>) -> Option<String> {
        NO_PROXY_KEYS.iter().find_map(|key| {
            env.get(*key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    }
    if let Some(map) = network
        && let Some(value) = from(map)
    {
        return Some(value);
    }
    if let Some(mutex) = SHELL_NETWORK.get()
        && let Some(value) = from(&mutex.lock().unwrap_or_else(|p| p.into_inner()))
    {
        return Some(value);
    }
    from(&std::env::vars().collect())
}

/// Expand canonical download URLs with their fallback GitHub Release mirrors (`GITHUB_MIRRORS`),
/// keeping official sources before mirrors.
pub fn expand_urls(direct: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut github = Vec::new();
    for raw in direct {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        push_unique(&mut out, trimmed.to_owned());
        if trimmed.starts_with("https://github.com/")
            || trimmed.starts_with("https://raw.githubusercontent.com/")
        {
            github.push(trimmed.to_owned());
        }
    }
    for prefix in GITHUB_MIRRORS {
        for url in &github {
            push_unique(&mut out, format!("{prefix}{url}"));
        }
    }
    out
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

#[cfg(windows)]
fn system_proxy_url() -> Option<String> {
    if read("ProxyEnable")
        .as_deref()
        .is_none_or(|value| !enabled(value))
    {
        return None;
    }
    let server = read("ProxyServer")?;
    Some(proxy_url(&server))
}

#[cfg(target_os = "macos")]
fn system_proxy_url() -> Option<String> {
    let mut command = std::process::Command::new("/usr/sbin/scutil");
    command.arg("--proxy").stdin(std::process::Stdio::null());
    let output = crate::process::output(command, std::time::Duration::from_secs(5)).ok()?;
    if !output.status.success() {
        return None;
    }
    parse_scutil(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_proxy_url() -> Option<String> {
    None
}

/// Parse `scutil --proxy` output on macOS: HTTPS wins over HTTP over SOCKS5.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn parse_scutil(out: &str) -> Option<String> {
    let mut kv = BTreeMap::new();
    let mut in_list = false;
    for raw_line in out.lines() {
        let line = raw_line.trim();
        if in_list {
            if line == "}" {
                in_list = false;
            }
            continue;
        }
        let Some((k, v)) = line.split_once(" : ") else {
            continue;
        };
        if k.trim() == "ExceptionsList" {
            in_list = true;
            continue;
        }
        kv.insert(k.trim(), v.trim());
    }
    let entry = |key: &str, scheme: &str| -> Option<String> {
        if kv.get(format!("{key}Enable").as_str()).copied() != Some("1") {
            return None;
        }
        let host = kv
            .get(format!("{key}Proxy").as_str())
            .copied()
            .filter(|h| !h.is_empty())?;
        let host_port = match kv
            .get(format!("{key}Port").as_str())
            .copied()
            .filter(|p| !p.is_empty())
        {
            Some(port) if host.contains(':') && !host.starts_with('[') => {
                format!("[{host}]:{port}")
            }
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        Some(format!("{scheme}://{host_port}"))
    };
    entry("HTTPS", "http")
        .or_else(|| entry("HTTP", "http"))
        .or_else(|| entry("SOCKS", "socks5"))
}

/// `ProxyEnable` is a DWORD: the registry prints it as `0x1`, and older tools as `1`.
#[cfg(any(windows, test))]
fn enabled(value: &str) -> bool {
    matches!(value.trim(), "1" | "0x1" | "0x00000001")
}

/// The proxy URL as an environment variable takes it. The registry holds `host:port`, or a
/// per-scheme list such as `http=host:port;https=host:port`.
#[cfg(any(windows, test))]
fn proxy_url(server: &str) -> String {
    let entry = server
        .split(';')
        .filter_map(|part| part.split_once('='))
        .find(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        .or_else(|| {
            server
                .split(';')
                .filter_map(|part| part.split_once('='))
                .find(|(scheme, _)| scheme.eq_ignore_ascii_case("http"))
        })
        .map(|(_, value)| value)
        .unwrap_or(server);
    let entry = entry.split(';').next().unwrap_or(entry).trim();
    if entry.contains("://") {
        entry.to_owned()
    } else {
        format!("http://{entry}")
    }
}

/// One value from the WinINET settings key.
#[cfg(windows)]
fn read(name: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            name,
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(name)?
            .split_once("    REG_")
            .map(|(_, rest)| {
                rest.split_once("    ")
                    .map(|(_, value)| value.trim().to_owned())
            })?
    })
}

#[cfg(test)]
mod tests;

//! The system proxy, handed on to the CLIs this app starts.
//!
//! Windows keeps its proxy in the registry, where only WinINET clients look. The CLIs we probe
//! are Go and Node programs that read `HTTPS_PROXY` instead, so on a proxied machine every probe
//! tries a direct connection: `agy models` sits on a Google IPv6 address until the timeout and
//! the desk lists no models at all.

/// Copy the system proxy into this process's environment, unless the user already set one.
#[cfg(windows)]
pub fn apply() {
    if std::env::var_os("HTTPS_PROXY").is_some() || std::env::var_os("https_proxy").is_some() {
        return;
    }
    if read("ProxyEnable")
        .as_deref()
        .is_none_or(|value| !enabled(value))
    {
        return;
    }
    let Some(server) = read("ProxyServer") else {
        return;
    };
    let proxy = proxy_url(&server);
    // SAFETY: `main` calls this before it starts any other thread, so no other thread is reading
    // the environment while it changes.
    unsafe {
        std::env::set_var("HTTPS_PROXY", &proxy);
        std::env::set_var("HTTP_PROXY", &proxy);
    }
}

#[cfg(not(windows))]
pub fn apply() {}

/// `ProxyEnable` is a DWORD: the registry prints it as `0x1`, and older tools as `1`.
#[cfg(windows)]
fn enabled(value: &str) -> bool {
    matches!(value.trim(), "1" | "0x1" | "0x00000001")
}

/// The proxy URL as an environment variable takes it. The registry holds `host:port`, or a
/// per-scheme list such as `http=host:port;https=host:port`.
#[cfg(windows)]
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

#[cfg(all(test, windows))]
mod tests {
    use super::{enabled, proxy_url};

    #[test]
    fn a_plain_host_and_port_gets_a_scheme() {
        assert_eq!(proxy_url("127.0.0.1:7897"), "http://127.0.0.1:7897");
    }

    #[test]
    fn a_per_scheme_list_prefers_https() {
        assert_eq!(
            proxy_url("http=127.0.0.1:7890;https=127.0.0.1:7897"),
            "http://127.0.0.1:7897"
        );
    }

    #[test]
    fn a_value_that_already_has_a_scheme_is_kept() {
        assert_eq!(
            proxy_url("https://127.0.0.1:7897"),
            "https://127.0.0.1:7897"
        );
    }

    #[test]
    fn the_enable_flag_is_a_dword_in_the_registry() {
        assert!(enabled("0x1"));
        assert!(enabled("1"));
        assert!(!enabled("0x0"));
        assert!(!enabled("0"));
    }
}

//! Addresses of services the App calls, as every settings page that takes one checks them: HTTPS,
//! or HTTP on this machine, and nothing else.

use std::net::IpAddr;

/// Whether a host is this machine: `localhost` or a loopback address. An IPv4-mapped IPv6 address
/// is not one, whatever it maps to.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(crate) fn is_loopback_url(url: &reqwest::Url) -> bool {
    is_loopback_host(url.host_str().unwrap_or(""))
}

/// An address the App will call: HTTPS, or HTTP on this machine, without credentials, query or
/// fragment, a zero port or an empty one, and without a trailing slash. `label` names the address
/// in the error.
pub(crate) fn normalize_endpoint(base_url: &str, label: &str) -> Result<String, String> {
    if base_url.is_empty() {
        return Err(format!("请填写{label} 地址"));
    }
    if base_url.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(format!("{label} 地址无效"));
    }
    let url = reqwest::Url::parse(base_url).map_err(|_| format!("{label} 地址无效"))?;
    let authority = base_url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
    // The parsed form also catches shorthands like `https:user:pass@host`.
    let has_userinfo = !url.username().is_empty()
        || url.password().is_some()
        || authority.is_some_and(|authority| authority.contains('@'));
    if has_userinfo || url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{label} 地址不能包含账号、查询参数或片段"));
    }
    // Require `scheme://authority`, and reject backslashes, a zero port, or a trailing colon with no port.
    let Some(authority) = authority else {
        return Err(format!("{label} 地址无效"));
    };
    if authority.is_empty()
        || authority.contains('\\')
        || url.port() == Some(0)
        || authority.ends_with(':')
    {
        return Err(format!("{label} 地址无效"));
    }
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback_url(&url)) {
        return Err(format!("{label} 只允许 HTTPS，或本机回环地址上的 HTTP"));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_https_or_loopback_without_credentials_query_fragment_or_empty_port() {
        assert_eq!(
            normalize_endpoint("https://api.example.com/v1/", "X").unwrap(),
            "https://api.example.com/v1"
        );
        assert!(normalize_endpoint("http://[::1]:8080/v1", "X").is_ok());
        for invalid in [
            "http://api.example.com/v1",
            "http://[::ffff:127.0.0.1]:11434",
            "https://example.com:0/v1",
            "https://h:",
            "https://h:/v1",
            "https:example.invalid:",
            "https:///example.invalid:",
            "https://\\example.invalid:",
            "https://\t/example.invalid:",
            "https://user@api.example.com/v1",
            "https://api.example.com/v1?",
            "https://api.example.com/v1#",
        ] {
            assert!(normalize_endpoint(invalid, "X").is_err(), "{invalid}");
        }
    }
}

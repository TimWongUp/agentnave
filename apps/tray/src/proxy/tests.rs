use super::{enabled, expand_urls, parse_scutil, proxy_url, resolve_no_proxy};
use std::collections::BTreeMap;

#[test]
fn no_proxy_is_the_first_nonempty_key_of_the_environment() {
    let mut env = BTreeMap::new();
    env.insert("no_proxy".to_owned(), " 127.0.0.1,localhost ".to_owned());
    assert_eq!(
        resolve_no_proxy(Some(&env)).as_deref(),
        Some("127.0.0.1,localhost")
    );
    env.insert("NO_PROXY".to_owned(), "example.com".to_owned());
    assert_eq!(resolve_no_proxy(Some(&env)).as_deref(), Some("example.com"));
    // An empty upper-case value falls through to the lower-case one.
    env.insert("NO_PROXY".to_owned(), "   ".to_owned());
    assert_eq!(
        resolve_no_proxy(Some(&env)).as_deref(),
        Some("127.0.0.1,localhost")
    );
}

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

#[test]
fn scutil_prefers_https_then_http_then_socks_and_ignores_exceptions() {
    let https = "\
<dictionary> {
  ExceptionsList : <array> {
    0 : 127.0.0.1
    1 : localhost
  }
  HTTPEnable : 1
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7897
  HTTPSProxy : 127.0.0.1
  SOCKSEnable : 1
  SOCKSPort : 7891
  SOCKSProxy : 127.0.0.1
}";
    assert_eq!(
        parse_scutil(https).as_deref(),
        Some("http://127.0.0.1:7897")
    );

    let socks_only = "\
<dictionary> {
  HTTPSEnable : 0
  SOCKSEnable : 1
  SOCKSPort : 1080
  SOCKSProxy : 127.0.0.1
}";
    assert_eq!(
        parse_scutil(socks_only).as_deref(),
        Some("socks5://127.0.0.1:1080")
    );
    assert_eq!(parse_scutil("<dictionary> {\n  HTTPEnable : 0\n}"), None);
}

#[test]
fn expand_urls_puts_github_mirrors_after_direct_sources() {
    let urls = expand_urls(&[
        "https://github.com/TimWongUp/agentnave/releases/latest/download/pack.tar.gz".into(),
    ]);
    assert_eq!(
        urls,
        vec![
            "https://github.com/TimWongUp/agentnave/releases/latest/download/pack.tar.gz",
            "https://ghfast.top/https://github.com/TimWongUp/agentnave/releases/latest/download/pack.tar.gz",
            "https://gh-proxy.com/https://github.com/TimWongUp/agentnave/releases/latest/download/pack.tar.gz",
        ]
    );
}

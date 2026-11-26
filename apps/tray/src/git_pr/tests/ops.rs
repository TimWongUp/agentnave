use serde_json::json;

use super::super::gh::{parse_status_check_rollup, validate_github_url};

#[test]
fn validates_only_https_github_urls() {
    assert!(validate_github_url("https://github.com/acme/demo/pull/42").is_ok());
    assert!(validate_github_url("https://github.com/acme/demo/pull/42/checks").is_ok());

    for bad in [
        "",
        "http://github.com/acme/demo/pull/42",
        "https://evil.com/acme/demo/pull/42",
        "https://github.com.evil.com/acme/demo/pull/42",
        "https://user@github.com/acme/demo/pull/42",
        "https://github.com:443/acme/demo/pull/42",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "https://github.com/acme/demo/pull/42 extra",
        "https://github.com/acme/demo/pull/42\n",
    ] {
        assert!(
            validate_github_url(bad).is_err(),
            "expected err for {bad:?}"
        );
    }
}

#[test]
fn external_ci_check_url_falls_back_to_github_pr_checks() {
    let rollup = json!([
        {
            "name": "external-ci",
            "status": "COMPLETED",
            "conclusion": "FAILURE",
            "detailsUrl": "https://ci.example.com/build/123"
        }
    ]);
    let summary = parse_status_check_rollup(&rollup, "https://github.com/acme/demo/pull/7");
    assert_eq!(summary.state, "failing");
    assert_eq!(
        summary.checks_url.as_deref(),
        Some("https://github.com/acme/demo/pull/7/checks")
    );
}

//! Read-only GitHub CLI (`gh`) invocation and `gh pr view --json` parsing (ADR-0115).
//!
//! Strictly read-only: never invokes `gh auth token`, never stores credentials, and only inspects
//! `github.com` pull requests. URL/link helpers ported from AgentNave M1 (`terminal/gh.rs`,
//! `terminal/gh/parse.rs`); merged-PR check ported from t3code `storageCleanupPullRequestMerged`
//! (MIT).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::{OutputError, output};

const GH_TIMEOUT: Duration = Duration::from_secs(15);
/// Single-call JSON fields requested from `gh pr view --json`.
pub(crate) const PR_VIEW_FIELDS: &str = "title,number,state,isDraft,additions,deletions,\
statusCheckRollup,reviewDecision,mergeable,updatedAt,url,headRefName,headRefOid,baseRefName";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GhStatus {
    Ready,
    NotInstalled,
    NotAuthenticated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GhError {
    NotInstalled,
    NotAuthenticated,
    RateLimited,
    NotFound,
    Other(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrCheckSummary {
    /// `"passing"` | `"failing"` | `"pending"` | `"none"`
    pub state: String,
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub pending: u64,
    pub checks_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrInfo {
    pub repo: String,
    pub number: u64,
    pub title: String,
    /// `"open"` | `"draft"` | `"merged"` | `"closed"`
    pub state: String,
    pub is_draft: bool,
    pub additions: u64,
    pub deletions: u64,
    pub checks: PrCheckSummary,
    /// `"approved"` | `"changes_requested"` | `"review_required"` | `None`
    pub review_decision: Option<String>,
    /// `"mergeable"` | `"conflicting"` | `"unknown"`
    pub mergeable: String,
    pub updated_at: String,
    pub url: String,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_name: String,
}

/// Compact summary for isolated-directory delete confirmation when merged via PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergedPrSummary {
    pub number: u64,
    pub repo: String,
    pub title: String,
    pub url: String,
}

impl From<&PrInfo> for MergedPrSummary {
    fn from(pr: &PrInfo) -> Self {
        Self {
            number: pr.number,
            repo: pr.repo.clone(),
            title: pr.title.clone(),
            url: pr.url.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct GhClient {
    program: Option<PathBuf>,
}

impl GhClient {
    pub(crate) fn system() -> Self {
        Self { program: None }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn with_program(program: PathBuf) -> Self {
        Self {
            program: Some(program),
        }
    }

    pub(crate) fn resolve_program(&self) -> Option<PathBuf> {
        if let Some(path) = &self.program {
            return path.is_file().then(|| path.clone());
        }
        locate_gh()
    }

    /// Checks `gh` installation and login status without reading tokens.
    pub(crate) fn check_status(&self) -> GhStatus {
        let Some(program) = self.resolve_program() else {
            return GhStatus::NotInstalled;
        };
        let Ok(out) = self.run_gh(&program, &["auth", "status", "--hostname", "github.com"]) else {
            return GhStatus::NotAuthenticated;
        };
        if out.status.success() {
            GhStatus::Ready
        } else {
            GhStatus::NotAuthenticated
        }
    }

    /// Fetches one PR by number in `owner/repo` using a single `gh pr view --json` call.
    pub(crate) fn view_pr_number(
        &self,
        repo: &str,
        number: u64,
    ) -> Result<Option<PrInfo>, GhError> {
        if split_repo(repo).is_none() || number == 0 {
            return Ok(None);
        }
        let num = number.to_string();
        self.view_pr_selector(&num, repo)
    }

    /// Fetches the PR associated with `branch` in `owner/repo` using `gh pr view --json`.
    pub(crate) fn view_pr_branch(
        &self,
        repo: &str,
        branch: &str,
    ) -> Result<Option<PrInfo>, GhError> {
        let branch = branch.trim();
        if split_repo(repo).is_none() || branch.is_empty() || branch.starts_with('-') {
            return Ok(None);
        }
        self.view_pr_selector(branch, repo)
    }

    fn view_pr_selector(&self, selector: &str, repo: &str) -> Result<Option<PrInfo>, GhError> {
        let program = self.resolve_program().ok_or(GhError::NotInstalled)?;
        let out = self.run_gh(
            &program,
            &[
                "pr",
                "view",
                selector,
                "--repo",
                repo,
                "--json",
                PR_VIEW_FIELDS,
            ],
        )?;
        if out.status.success() {
            let value: Value = serde_json::from_slice(&out.stdout)
                .map_err(|e| GhError::Other(format!("解析 gh pr view 输出失败：{e}")))?;
            return Ok(parse_pr_view_json(&value, Some(repo)));
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("not logged")
            || lower.contains("gh auth login")
            || lower.contains("authentication")
            || lower.contains("http 401")
        {
            return Err(GhError::NotAuthenticated);
        }
        if lower.contains("rate limit") || lower.contains("http 429") {
            return Err(GhError::RateLimited);
        }
        if lower.contains("no pull requests found")
            || lower.contains("could not resolve to a pullrequest")
            || lower.contains("could not resolve to a repository")
            || lower.contains("not found")
            || lower.contains("http 404")
        {
            return Err(GhError::NotFound);
        }
        Err(GhError::Other(
            stderr.trim().chars().take(200).collect::<String>(),
        ))
    }

    fn run_gh(&self, program: &Path, args: &[&str]) -> Result<std::process::Output, GhError> {
        self.run_gh_with_timeout(program, args, GH_TIMEOUT)
    }

    fn run_gh_with_timeout(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<std::process::Output, GhError> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.stdin(Stdio::null());
        cmd.current_dir(std::env::temp_dir());
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(crate::terminal::parent_session) {
                cmd.env_remove(name);
            }
        }
        cmd.env("GH_PROMPT_DISABLED", "1");
        if self.program.is_none()
            && let Some(path) = crate::detect::search_path(program)
        {
            cmd.env("PATH", path);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        output(cmd, timeout).map_err(|err| match err {
            OutputError::Timeout => GhError::Other("gh 执行超时".into()),
            OutputError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => GhError::NotInstalled,
            OutputError::Io(e) => GhError::Other(format!("无法执行 gh：{e}")),
            OutputError::Reader => GhError::Other("读取 gh 输出失败".into()),
        })
    }
}

/// Validates that `raw` is strictly an `https://github.com/...` URL safe to open in the browser.
pub(crate) fn validate_github_url(raw: &str) -> Result<String, String> {
    if raw.is_empty()
        || !raw.starts_with("https://github.com/")
        || raw.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("仅允许打开 https://github.com/ 开头的链接".to_owned());
    }
    let parsed = tauri::Url::parse(raw).map_err(|_| "无效的 GitHub 链接".to_owned())?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("仅允许打开 https://github.com/ 开头的链接".to_owned());
    }
    Ok(parsed.to_string())
}

/// Locates `gh` on PATH or common install locations.
pub(crate) fn locate_gh() -> Option<PathBuf> {
    let mut dirs_list: Vec<PathBuf> = Vec::new();
    if let Some(path_env) = std::env::var_os("PATH") {
        dirs_list.extend(std::env::split_paths(&path_env));
    }
    #[cfg(not(windows))]
    {
        for fallback in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            dirs_list.push(PathBuf::from(fallback));
        }
        if let Some(home) = dirs::home_dir() {
            dirs_list.push(home.join(".local/bin"));
        }
    }
    let name = if cfg!(windows) { "gh.exe" } else { "gh" };
    dirs_list
        .into_iter()
        .map(|d| d.join(name))
        .find(|candidate| candidate.is_file())
}

/// Parses one `gh pr view --json ...` JSON object into `PrInfo`.
pub(crate) fn parse_pr_view_json(body: &Value, fallback_repo: Option<&str>) -> Option<PrInfo> {
    let number = body["number"].as_u64()?;
    let url = body["url"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| fallback_repo.map(|repo| format!("https://github.com/{repo}/pull/{number}")))?;
    let repo = extract_github_pr_links(&url)
        .into_iter()
        .next()
        .map(|(r, _)| r)
        .or_else(|| fallback_repo.map(str::to_owned))?;
    let is_draft = body["isDraft"].as_bool().unwrap_or(false);
    let state = match body["state"]
        .as_str()
        .unwrap_or("OPEN")
        .to_ascii_uppercase()
        .as_str()
    {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        _ if is_draft => "draft",
        _ => "open",
    }
    .to_owned();
    let review_decision = body["reviewDecision"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|raw| match raw.to_ascii_uppercase().as_str() {
            "APPROVED" => "approved".to_owned(),
            "CHANGES_REQUESTED" => "changes_requested".to_owned(),
            "REVIEW_REQUIRED" => "review_required".to_owned(),
            other => other.to_ascii_lowercase(),
        });
    let mergeable = if matches!(state.as_str(), "merged" | "closed") {
        "unknown".to_owned()
    } else {
        match body["mergeable"]
            .as_str()
            .unwrap_or("UNKNOWN")
            .to_ascii_uppercase()
            .as_str()
        {
            "MERGEABLE" => "mergeable".to_owned(),
            "CONFLICTING" => "conflicting".to_owned(),
            _ => "unknown".to_owned(),
        }
    };
    let checks = parse_status_check_rollup(&body["statusCheckRollup"], &url);
    Some(PrInfo {
        repo,
        number,
        title: body["title"].as_str().unwrap_or_default().to_owned(),
        state,
        is_draft,
        additions: body["additions"].as_u64().unwrap_or(0),
        deletions: body["deletions"].as_u64().unwrap_or(0),
        checks,
        review_decision,
        mergeable,
        updated_at: body["updatedAt"].as_str().unwrap_or_default().to_owned(),
        url,
        head_ref_name: body["headRefName"].as_str().unwrap_or_default().to_owned(),
        head_ref_oid: body["headRefOid"].as_str().unwrap_or_default().to_owned(),
        base_ref_name: body["baseRefName"].as_str().unwrap_or_default().to_owned(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckBucket {
    Passed,
    Failed,
    Pending,
}

/// Aggregates `statusCheckRollup` entries (both `CheckRun` and `StatusContext`), deduplicating by
/// check name/context so the latest run wins.
pub(crate) fn parse_status_check_rollup(rollup: &Value, pr_url: &str) -> PrCheckSummary {
    let Some(items) = rollup.as_array() else {
        return PrCheckSummary {
            state: "none".to_owned(),
            ..PrCheckSummary::default()
        };
    };
    let mut by_name: HashMap<String, (CheckBucket, Option<String>)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for item in items {
        let Some((name, bucket, url)) = classify_rollup_item(item) else {
            continue;
        };
        if !by_name.contains_key(&name) {
            order.push(name.clone());
        }
        by_name.insert(name, (bucket, url));
    }
    let (mut passed, mut failed, mut pending) = (0u64, 0u64, 0u64);
    let mut failing_url: Option<String> = None;
    let mut any_url: Option<String> = None;
    for name in order {
        let Some((bucket, url)) = by_name.remove(&name) else {
            continue;
        };
        if any_url.is_none() && url.is_some() {
            any_url = url.clone();
        }
        match bucket {
            CheckBucket::Passed => passed += 1,
            CheckBucket::Failed => {
                failed += 1;
                if failing_url.is_none() && url.is_some() {
                    failing_url = url;
                }
            }
            CheckBucket::Pending => pending += 1,
        }
    }
    let total = passed + failed + pending;
    let state = if total == 0 {
        "none"
    } else if failed > 0 {
        "failing"
    } else if pending > 0 {
        "pending"
    } else {
        "passing"
    }
    .to_owned();
    let checks_url = if total == 0 {
        None
    } else {
        failing_url
            .filter(|u| validate_github_url(u).is_ok())
            .or_else(|| {
                validate_github_url(pr_url)
                    .ok()
                    .map(|_| format!("{pr_url}/checks"))
            })
            .or_else(|| any_url.filter(|u| validate_github_url(u).is_ok()))
    };
    PrCheckSummary {
        state,
        total,
        passed,
        failed,
        pending,
        checks_url,
    }
}

fn classify_rollup_item(item: &Value) -> Option<(String, CheckBucket, Option<String>)> {
    if let Some(name) = item["name"].as_str().filter(|s| !s.is_empty()) {
        let status = item["status"].as_str().unwrap_or("").to_ascii_uppercase();
        let conclusion = item["conclusion"]
            .as_str()
            .unwrap_or("")
            .to_ascii_uppercase();
        let bucket = if !matches!(status.as_str(), "" | "COMPLETED") {
            CheckBucket::Pending
        } else {
            match conclusion.as_str() {
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckBucket::Passed,
                "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "CANCELLED" | "STALE"
                | "STARTUP_FAILURE" => CheckBucket::Failed,
                "" => CheckBucket::Pending,
                _ => CheckBucket::Failed,
            }
        };
        let url = item["detailsUrl"]
            .as_str()
            .or_else(|| item["htmlUrl"].as_str())
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            .map(str::to_owned);
        return Some((name.to_owned(), bucket, url));
    }
    let context = item["context"].as_str().filter(|s| !s.is_empty())?;
    let bucket = match item["state"]
        .as_str()
        .unwrap_or("")
        .to_ascii_uppercase()
        .as_str()
    {
        "SUCCESS" => CheckBucket::Passed,
        "FAILURE" | "ERROR" => CheckBucket::Failed,
        _ => CheckBucket::Pending,
    };
    let url = item["targetUrl"]
        .as_str()
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        .map(str::to_owned);
    Some((context.to_owned(), bucket, url))
}

/// Priority rank for sorting PRs and picking badge color:
/// CI failing or conflict (5) > open / awaiting review (4) > draft (3) > merged (2) > closed (1).
pub(crate) fn pr_priority_rank(pr: &PrInfo) -> u8 {
    let active = matches!(pr.state.as_str(), "open" | "draft");
    if active && (pr.checks.state == "failing" || pr.mergeable == "conflicting") {
        return 5;
    }
    match pr.state.as_str() {
        "open" => 4,
        "draft" => 3,
        "merged" => 2,
        _ => 1,
    }
}

/// Matches t3code `storageCleanupPullRequestMerged` (MIT):
/// A worktree counts as merged via `pr` iff `pr.state == "merged"` and either the worktree commit
/// is already integrated into the default branch or the merged PR's headRef, baseRef, and headSha
/// match the worktree's branch, default branch, and HEAD SHA.
pub(crate) fn pr_merged_for_worktree(
    pr: &PrInfo,
    branch: &str,
    default_branch: Option<&str>,
    head_sha: &str,
    integrated: bool,
) -> bool {
    if pr.state != "merged" {
        return false;
    }
    if integrated {
        return true;
    }
    let base_matches = default_branch
        .filter(|name| !name.is_empty())
        .is_some_and(|name| pr.base_ref_name == name);
    !head_sha.is_empty()
        && pr.head_ref_name == branch
        && base_matches
        && pr.head_ref_oid.eq_ignore_ascii_case(head_sha)
}

/// Parses a Git remote URL and returns `Some("owner/repo")` strictly for `github.com`.
pub(crate) fn parse_github_remote_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let rest = if let Some(s) = trimmed.strip_prefix("git@") {
        let (host, path) = s.split_once(':')?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    } else {
        let idx = trimmed.find("://")?;
        let scheme = &trimmed[..idx];
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "https" | "http" | "ssh" | "git"
        ) {
            return None;
        }
        let after = &trimmed[idx + 3..];
        let (authority, path) = after.split_once('/')?;
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        let host = host_port.split(':').next().unwrap_or(host_port);
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    };
    let mut parts = rest.trim_start_matches('/').split('/');
    let owner = parts.next()?;
    let repo_raw = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
    if !valid_gh_segment(owner) || !valid_gh_segment(repo) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Extracts all `(owner/repo, pr_number)` pairs from `https?://github.com/<owner>/<repo>/pull/<n>`.
/// Scans byte-wise so multibyte UTF-8 text never causes char-boundary slicing panics.
pub(crate) fn extract_github_pr_links(text: &str) -> Vec<(String, u64)> {
    const HTTPS: &[u8] = b"https://github.com/";
    const HTTP: &[u8] = b"http://github.com/";
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let prefix_len = if bytes.len() - i >= HTTPS.len()
            && bytes[i..i + HTTPS.len()].eq_ignore_ascii_case(HTTPS)
        {
            HTTPS.len()
        } else if bytes.len() - i >= HTTP.len()
            && bytes[i..i + HTTP.len()].eq_ignore_ascii_case(HTTP)
        {
            HTTP.len()
        } else {
            i += 1;
            continue;
        };
        let after_host = &text[i + prefix_len..];
        if let Some((repo, num, consumed)) = parse_pr_path_prefix(after_host) {
            found.push((repo, num));
            i += prefix_len + consumed;
        } else {
            i += 1;
        }
    }
    found
}

fn parse_pr_path_prefix(after_host: &str) -> Option<(String, u64, usize)> {
    let (owner, rest) = after_host.split_once('/')?;
    let (repo, rest) = rest.split_once('/')?;
    let rest = rest.strip_prefix("pull/")?;
    if !valid_gh_segment(owner) || !valid_gh_segment(repo) {
        return None;
    }
    let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    if let Some(next) = rest.as_bytes().get(digits_len)
        && (next.is_ascii_alphanumeric() || *next == b'_')
    {
        return None;
    }
    let num = rest[..digits_len].parse::<u64>().ok()?;
    let consumed = owner.len() + 1 + repo.len() + 6 + digits_len;
    Some((format!("{owner}/{repo}"), num, consumed))
}

pub(crate) fn valid_gh_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub(crate) fn split_repo(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.split_once('/')?;
    (valid_gh_segment(owner) && valid_gh_segment(name)).then_some((owner, name))
}

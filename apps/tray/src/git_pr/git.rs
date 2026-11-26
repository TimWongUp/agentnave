//! Read-only Git workspace status inspection using neutralized `GitRunner` (ADR-0115).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::gh::parse_github_remote_url;
use crate::isolated::git::{GitRunner, bytes_to_path, decode_git_path, ref_exists, trim_line_end};
use crate::isolated::leases::same_dir;

/// Read-only local Git status for a session or isolated directory `cwd`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GitWorkspaceStatus {
    pub is_git: bool,
    pub branch: Option<String>,
    pub detached: bool,
    pub head_sha: Option<String>,
    pub is_worktree: bool,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub default_branch: Option<String>,
    pub ahead_of_default: Option<u64>,
    pub integrated_into_default: bool,
    pub changed_files: u64,
    pub staged_files: u64,
    pub unstaged_files: u64,
    pub untracked_files: u64,
    pub additions: u64,
    pub deletions: u64,
    pub github_repo: Option<String>,
}

const READ_ONLY_ENV: &[(&str, &str)] = &[("GIT_OPTIONAL_LOCKS", "0")];

/// Inspect local Git status of `cwd` strictly read-only with neutralized git parameters.
pub(crate) fn inspect_workspace_git(git: &GitRunner, cwd: &Path) -> Option<GitWorkspaceStatus> {
    if !cwd.is_dir() {
        return None;
    }
    let status_out = git
        .run(
            cwd,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=all",
            ],
            &[0, 128],
            READ_ONLY_ENV,
        )
        .ok()?;
    if status_out.code != 0 {
        return None;
    }
    let mut info = parse_porcelain_v2_branch(&status_out.stdout);
    info.is_git = true;
    info.is_worktree = detect_is_worktree(git, cwd);

    if info.staged_files > 0 || info.unstaged_files > 0 {
        let (add, del) = read_diff_shortstat(git, cwd, info.head_sha.is_some());
        info.additions = add;
        info.deletions = del;
    }

    let remote_name = resolve_remote_name(git, cwd, info.branch.as_deref());
    info.github_repo = resolve_github_repo(git, cwd, &remote_name);

    if let Some((default_name, default_ref)) = resolve_default_branch(git, cwd, &remote_name) {
        if let Some(head_sha) = info.head_sha.as_deref() {
            if info.branch.as_deref() == Some(default_name.as_str()) {
                info.ahead_of_default = Some(0);
                info.integrated_into_default = true;
            } else if let Some(count) = count_commits_ahead(git, cwd, &default_ref, head_sha) {
                info.ahead_of_default = Some(count);
                info.integrated_into_default = count == 0;
            }
        }
        info.default_branch = Some(default_name);
    }

    Some(info)
}

pub(crate) fn parse_porcelain_v2_branch(raw: &[u8]) -> GitWorkspaceStatus {
    let mut info = GitWorkspaceStatus::default();
    let mut tokens: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    if tokens.last() == Some(&b"".as_slice()) {
        tokens.pop();
    }
    let mut changed = BTreeSet::new();
    let mut idx = 0;
    while idx < tokens.len() {
        let token = tokens[idx];
        if let Some(rest) = token.strip_prefix(b"# ") {
            parse_branch_header(&String::from_utf8_lossy(rest), &mut info);
            idx += 1;
        } else if let Some(rest) = token.strip_prefix(b"? ") {
            info.untracked_files += 1;
            changed.insert(decode_git_path(rest));
            idx += 1;
        } else if token.starts_with(b"1 ") {
            parse_tracked_entry(token, 9, &mut info, &mut changed);
            idx += 1;
        } else if token.starts_with(b"2 ") {
            parse_tracked_entry(token, 10, &mut info, &mut changed);
            idx += 2; // skip NUL-separated origPath
        } else if token.starts_with(b"u ") {
            let parts: Vec<&[u8]> = token.splitn(11, |&b| b == b' ').collect();
            if parts.len() == 11 {
                info.unstaged_files += 1;
                changed.insert(decode_git_path(parts[10]));
            }
            idx += 1;
        } else {
            idx += 1;
        }
    }
    info.changed_files = u64::try_from(changed.len()).unwrap_or(u64::MAX);
    info
}

fn parse_branch_header(line: &str, info: &mut GitWorkspaceStatus) {
    if let Some(oid) = line.strip_prefix("branch.oid ") {
        let oid = oid.trim();
        if !oid.is_empty() && oid != "(initial)" {
            info.head_sha = Some(oid.to_owned());
        }
    } else if let Some(head) = line.strip_prefix("branch.head ") {
        let head = head.trim();
        if head == "(detached)" {
            info.detached = true;
            info.branch = None;
        } else if !head.is_empty() {
            info.detached = false;
            info.branch = Some(head.to_owned());
        }
    } else if let Some(upstream) = line.strip_prefix("branch.upstream ") {
        let upstream = upstream.trim();
        if !upstream.is_empty() {
            info.upstream = Some(upstream.to_owned());
        }
    } else if let Some(ab) = line.strip_prefix("branch.ab ") {
        let mut parts = ab.split_whitespace();
        if let Some(ahead) = parts.next().and_then(|s| s.strip_prefix('+')) {
            info.ahead = ahead.parse().unwrap_or(0);
        }
        if let Some(behind) = parts.next().and_then(|s| s.strip_prefix('-')) {
            info.behind = behind.parse().unwrap_or(0);
        }
    }
}

fn parse_tracked_entry(
    token: &[u8],
    expected_parts: usize,
    info: &mut GitWorkspaceStatus,
    changed: &mut BTreeSet<String>,
) {
    let parts: Vec<&[u8]> = token.splitn(expected_parts, |&b| b == b' ').collect();
    if parts.len() != expected_parts {
        return;
    }
    let xy = parts[1];
    if xy.len() >= 2 {
        if xy[0] != b'.' {
            info.staged_files += 1;
        }
        if xy[1] != b'.' {
            info.unstaged_files += 1;
        }
    }
    changed.insert(decode_git_path(parts[expected_parts - 1]));
}

fn detect_is_worktree(git: &GitRunner, cwd: &Path) -> bool {
    let Ok(out) = git.run(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ],
        &[0, 128],
        READ_ONLY_ENV,
    ) else {
        return false;
    };
    if out.code != 0 {
        return false;
    }
    let mut lines = out.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty());
    let (Some(git_dir), Some(common_dir)) = (lines.next(), lines.next()) else {
        return false;
    };
    let git_dir: PathBuf = bytes_to_path(trim_line_end(git_dir));
    let common_dir: PathBuf = bytes_to_path(trim_line_end(common_dir));
    !same_dir(&git_dir, &common_dir)
}

fn read_diff_shortstat(git: &GitRunner, cwd: &Path, has_head: bool) -> (u64, u64) {
    if has_head {
        let text = git_stdout(
            git,
            cwd,
            &["diff", "--ignore-submodules=all", "HEAD", "--shortstat"],
        )
        .unwrap_or_default();
        return parse_shortstat(&text);
    }
    let staged = git_stdout(
        git,
        cwd,
        &["diff", "--cached", "--ignore-submodules=all", "--shortstat"],
    )
    .unwrap_or_default();
    let unstaged = git_stdout(
        git,
        cwd,
        &["diff", "--ignore-submodules=all", "--shortstat"],
    )
    .unwrap_or_default();
    let (sa, sd) = parse_shortstat(&staged);
    let (ua, ud) = parse_shortstat(&unstaged);
    (sa.saturating_add(ua), sd.saturating_add(ud))
}

pub(crate) fn parse_shortstat(text: &str) -> (u64, u64) {
    let mut additions = 0u64;
    let mut deletions = 0u64;
    for part in text.split(',') {
        let trimmed = part.trim();
        let num = trimmed
            .split_whitespace()
            .next()
            .and_then(|tok| tok.parse::<u64>().ok())
            .unwrap_or(0);
        if trimmed.contains("insertion") {
            additions = additions.saturating_add(num);
        } else if trimmed.contains("deletion") {
            deletions = deletions.saturating_add(num);
        }
    }
    (additions, deletions)
}

fn valid_remote_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub(crate) fn resolve_remote_name(git: &GitRunner, cwd: &Path, branch: Option<&str>) -> String {
    branch
        .and_then(|b| {
            git_stdout(
                git,
                cwd,
                &["config", "--get", &format!("branch.{b}.remote")],
            )
        })
        .filter(|r| valid_remote_name(r))
        .unwrap_or_else(|| "origin".to_owned())
}

pub(crate) fn resolve_github_repo(
    git: &GitRunner,
    cwd: &Path,
    remote_name: &str,
) -> Option<String> {
    let url = valid_remote_name(remote_name)
        .then(|| git_stdout(git, cwd, &["remote", "get-url", "--", remote_name]))
        .flatten()
        .or_else(|| {
            (remote_name != "origin")
                .then(|| git_stdout(git, cwd, &["remote", "get-url", "--", "origin"]))
                .flatten()
        })?;
    parse_github_remote_url(&url)
}

/// Returns `(short_name, full_ref)` for the repository's default branch.
pub(crate) fn resolve_default_branch(
    git: &GitRunner,
    cwd: &Path,
    remote_name: &str,
) -> Option<(String, String)> {
    if valid_remote_name(remote_name) {
        let remote_head = format!("refs/remotes/{remote_name}/HEAD");
        if let Some(full) = git_stdout(git, cwd, &["symbolic-ref", "-q", &remote_head]) {
            let prefix = format!("refs/remotes/{remote_name}/");
            if !full.starts_with('-')
                && !full.chars().any(|c| c.is_control() || c.is_whitespace())
                && !full.contains("..")
                && let Some(name) = full.strip_prefix(&prefix)
                && !name.is_empty()
                && !name.starts_with('-')
                && ref_exists(git, cwd, &full).unwrap_or(false)
            {
                return Some((name.to_owned(), full));
            }
        }
        for (name, candidate) in [
            ("main", format!("refs/remotes/{remote_name}/main")),
            ("master", format!("refs/remotes/{remote_name}/master")),
        ] {
            if ref_exists(git, cwd, &candidate).unwrap_or(false) {
                return Some((name.to_owned(), candidate));
            }
        }
    }
    for (name, candidate) in [("main", "refs/heads/main"), ("master", "refs/heads/master")] {
        if ref_exists(git, cwd, candidate).unwrap_or(false) {
            return Some((name.to_owned(), candidate.to_owned()));
        }
    }
    None
}

fn resolve_commit_oid(git: &GitRunner, cwd: &Path, rev: &str) -> Option<String> {
    if rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let target = format!("{rev}^{{commit}}");
    let oid = git_stdout(
        git,
        cwd,
        &["rev-parse", "--verify", "--end-of-options", &target],
    )?;
    (oid.len() >= 40 && oid.bytes().all(|b| b.is_ascii_hexdigit())).then_some(oid)
}

pub(crate) fn count_commits_ahead(
    git: &GitRunner,
    cwd: &Path,
    base_ref: &str,
    head_oid: &str,
) -> Option<u64> {
    let base = resolve_commit_oid(git, cwd, base_ref)?;
    let head = resolve_commit_oid(git, cwd, head_oid)?;
    let range = format!("{base}..{head}");
    git_stdout(
        git,
        cwd,
        &["rev-list", "--count", "--end-of-options", &range],
    )?
    .parse::<u64>()
    .ok()
}

pub(crate) fn git_stdout(git: &GitRunner, cwd: &Path, args: &[&str]) -> Option<String> {
    let out = git.run(cwd, args, &[0, 1, 2, 128], READ_ONLY_ENV).ok()?;
    if out.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(trim_line_end(&out.stdout))
        .trim()
        .to_owned();
    (!text.is_empty()).then_some(text)
}

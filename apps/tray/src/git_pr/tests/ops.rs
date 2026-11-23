use std::fs;
use std::path::Path;

use serde_json::json;

use super::super::gh::{GhClient, parse_status_check_rollup, validate_github_url};
use super::super::git::{default_pr_draft, list_changed_files};
use super::super::ops::{exec_commit, exec_create_pr, exec_merge_pr, exec_pull, exec_push};
use super::git_cmd;
#[cfg(unix)]
use super::write_fake_gh;
use crate::isolated::git::GitRunner;

fn setup_origin_and_clone(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bare = tmp.join("origin.git");
    fs::create_dir_all(&bare).unwrap();
    git_cmd(&bare, &["init", "--bare", "-b", "main"]);

    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    git_cmd(&work, &["init", "-b", "main"]);
    git_cmd(&work, &["config", "user.name", "Tester"]);
    git_cmd(&work, &["config", "user.email", "tester@example.com"]);
    git_cmd(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    fs::write(work.join("README.md"), "hello\n").unwrap();
    git_cmd(&work, &["add", "README.md"]);
    git_cmd(&work, &["commit", "-m", "initial commit"]);
    git_cmd(&work, &["push", "-u", "origin", "main"]);
    (bare, work)
}

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

#[test]
fn commit_lists_files_neutralizes_hooks_and_rejects_empty_message() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-b", "main"]);
    git_cmd(&repo, &["config", "user.name", "Tester"]);
    git_cmd(&repo, &["config", "user.email", "tester@example.com"]);
    fs::write(repo.join("a.txt"), "v1\n").unwrap();
    git_cmd(&repo, &["add", "a.txt"]);
    git_cmd(&repo, &["commit", "-m", "init"]);

    // Install a hostile pre-commit hook that would abort if hooks were not neutralized.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = repo.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fs::write(repo.join("a.txt"), "v2\n").unwrap();
    fs::write(repo.join("b.txt"), "new\n").unwrap();

    let runner = GitRunner::new().unwrap();
    let files = list_changed_files(&runner, &repo).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|f| f.path == "a.txt" && f.unstaged));
    assert!(files.iter().any(|f| f.path == "b.txt" && f.untracked));

    // Empty message is rejected without committing.
    assert!(exec_commit(&runner, &repo, "   ", true).is_err());

    let res = exec_commit(&runner, &repo, "feat: add b and edit a", true).unwrap();
    assert_eq!(res["ok"], true);
    assert!(!res["sha"].as_str().unwrap().is_empty());
    assert!(list_changed_files(&runner, &repo).unwrap().is_empty());
    assert_eq!(
        git_cmd(&repo, &["log", "-1", "--pretty=%s"]),
        "feat: add b and edit a"
    );
}

#[test]
fn push_sets_upstream_on_first_push_and_pull_fast_forwards_or_reports_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let (bare, work) = setup_origin_and_clone(tmp.path());
    let runner = GitRunner::new().unwrap();

    // Create feature branch without upstream and push -> sets upstream automatically.
    git_cmd(&work, &["checkout", "-b", "feat/demo"]);
    fs::write(work.join("feat.txt"), "hello\n").unwrap();
    exec_commit(&runner, &work, "feat: demo change", true).unwrap();

    let pushed = exec_push(&runner, &work).unwrap();
    assert_eq!(pushed["ok"], true);
    assert_eq!(pushed["set_upstream"], true);
    assert_eq!(pushed["upstream"], "origin/feat/demo");
    assert_eq!(
        git_cmd(&work, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/feat/demo"
    );

    // Clone peer repo and push a new commit onto feat/demo -> fast-forward pull succeeds.
    let peer = tmp.path().join("peer");
    git_cmd(
        tmp.path(),
        &[
            "clone",
            "-b",
            "feat/demo",
            bare.to_str().unwrap(),
            peer.to_str().unwrap(),
        ],
    );
    git_cmd(&peer, &["config", "user.name", "Peer"]);
    git_cmd(&peer, &["config", "user.email", "peer@example.com"]);
    fs::write(peer.join("peer.txt"), "from peer\n").unwrap();
    git_cmd(&peer, &["add", "peer.txt"]);
    git_cmd(&peer, &["commit", "-m", "peer commit"]);
    git_cmd(&peer, &["push"]);

    let pulled = exec_pull(&runner, &work).unwrap();
    assert_eq!(pulled["ok"], true);
    assert_eq!(pulled["up_to_date"], false);
    assert!(work.join("peer.txt").is_file());

    // Pulling again reports up_to_date.
    let again = exec_pull(&runner, &work).unwrap();
    assert_eq!(again["up_to_date"], true);

    // Diverge both repos -> pull refuses to merge or rebase and returns raw git error.
    fs::write(peer.join("peer2.txt"), "diverge peer\n").unwrap();
    git_cmd(&peer, &["add", "peer2.txt"]);
    git_cmd(&peer, &["commit", "-m", "peer diverge"]);
    git_cmd(&peer, &["push"]);

    fs::write(work.join("local.txt"), "diverge local\n").unwrap();
    exec_commit(&runner, &work, "local diverge", true).unwrap();

    let (status, err) = exec_pull(&runner, &work).unwrap_err();
    assert_eq!(status, 409);
    assert!(
        err.contains("分支已分叉") || err.to_ascii_lowercase().contains("fast-forward"),
        "unexpected error: {err}"
    );
}

#[test]
fn default_pr_draft_uses_commits_ahead_of_default_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let (_bare, work) = setup_origin_and_clone(tmp.path());
    let runner = GitRunner::new().unwrap();

    git_cmd(&work, &["checkout", "-b", "feat/draft"]);
    fs::write(work.join("x.txt"), "x\n").unwrap();
    exec_commit(&runner, &work, "feat: first change\n\ndetailed body", true).unwrap();

    let (title, body) = default_pr_draft(&runner, &work, "feat/draft", Some("refs/heads/main"));
    assert_eq!(title, "feat: first change");
    assert_eq!(body, "detailed body");

    fs::write(work.join("y.txt"), "y\n").unwrap();
    exec_commit(&runner, &work, "feat: second change", true).unwrap();
    let (title2, body2) = default_pr_draft(&runner, &work, "feat/draft", Some("refs/heads/main"));
    assert_eq!(title2, "feat: first change");
    assert!(body2.contains("- feat: first change"));
    assert!(body2.contains("- feat: second change"));
}

#[cfg(unix)]
#[test]
fn create_pr_and_merge_pr_respect_repo_settings_and_surface_gh_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let (_bare, work) = setup_origin_and_clone(tmp.path());
    // Set a GitHub remote URL so repo resolution matches acme/demo, while pushing to local bare.
    let bare_url = git_cmd(&work, &["remote", "get-url", "origin"]);
    git_cmd(
        &work,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/demo.git",
        ],
    );
    git_cmd(&work, &["remote", "set-url", "--push", "origin", &bare_url]);
    git_cmd(&work, &["checkout", "-b", "feat/pr-test"]);
    fs::write(work.join("pr.txt"), "pr\n").unwrap();
    let runner = GitRunner::new().unwrap();
    exec_commit(&runner, &work, "feat: open pr", true).unwrap();

    let log_file = tmp.path().join("gh-args.log");
    let fake = write_fake_gh(
        tmp.path(),
        &format!(
            r#"
printf '%s\n' "$*" >> "{log}"
if [ "$1" = "repo" ] && [ "$2" = "view" ]; then
  echo '{{"mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":true,"deleteBranchOnMerge":true,"viewerDefaultMergeMethod":"SQUASH"}}'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "create" ]; then
  echo 'https://github.com/acme/demo/pull/42'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "merge" ]; then
  echo 'Merged pull request #42'
  exit 0
fi
echo "unexpected: $*" >&2
exit 1
"#,
            log = log_file.display()
        ),
    );
    let gh = GhClient::with_program(fake);

    // Create PR automatically pushes first and parses returned URL/number.
    let created = exec_create_pr(
        &runner,
        &gh,
        &work,
        "feat: open pr",
        "PR body text",
        Some("main"),
        false,
    )
    .unwrap();
    assert_eq!(created["number"], 42);
    assert_eq!(created["url"], "https://github.com/acme/demo/pull/42");

    // Disallowed merge method ("merge") is rejected before calling `gh pr merge`.
    let (code, err) = exec_merge_pr(&gh, "acme/demo", 42, "merge").unwrap_err();
    assert_eq!(code, 400);
    assert!(err.contains("merge"));

    // Allowed merge method ("squash") succeeds and never passes --delete-branch.
    let merged = exec_merge_pr(&gh, "acme/demo", 42, "squash").unwrap();
    assert_eq!(merged["ok"], true);
    assert_eq!(merged["delete_branch_on_merge"], true);

    let logged = fs::read_to_string(&log_file).unwrap();
    assert!(logged.contains("pr create --repo acme/demo --base main --head feat/pr-test"));
    assert!(logged.contains("pr merge 42 --repo acme/demo --squash"));
    assert!(!logged.contains("--delete-branch"));

    // Failure surfaces raw gh stderr.
    let failing = write_fake_gh(
        tmp.path(),
        r#"
if [ "$1" = "repo" ] && [ "$2" = "view" ]; then
  echo '{"mergeCommitAllowed":true,"squashMergeAllowed":false,"rebaseMergeAllowed":false,"deleteBranchOnMerge":false}'
  exit 0
fi
echo "GraphQL: Pull Request is not mergeable" >&2
exit 1
"#,
    );
    let gh_fail = GhClient::with_program(failing);
    let (_status, raw_err) = exec_merge_pr(&gh_fail, "acme/demo", 42, "merge").unwrap_err();
    assert!(raw_err.contains("Pull Request is not mergeable"));
}

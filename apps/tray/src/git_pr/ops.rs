//! User-confirmed basic Git & GitHub PR operations for CLI sessions (ADR-0115 §3, §5).
//!
//! Every repository write uses `isolated::git::GitRunner` (hooks, filters, fsmonitor neutralized)
//! and never runs destructive commands (reset, discard, force push, rebase, branch switch).
//! Operations are strictly scoped to CLI sessions (embedded terminals, derived sessions,
//! terminal-CLI boss workspaces, and idle isolated directories) and refuse when the CLI is working.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::GitPr;
use super::gh::{GhClient, GhError, GhStatus, validate_github_url};
use super::git::{
    default_pr_draft, git_stdout, inspect_workspace_git, list_changed_files,
    resolve_default_branch, resolve_github_repo, resolve_push_remote, resolve_remote_name,
};
use crate::data::Reader;
use crate::isolated::git::{GitRunner, bytes_to_path, trim_line_end};
use crate::response::{self, Response};
use crate::terminal::Terminals;

const NETWORK_GIT_TIMEOUT: Duration = Duration::from_secs(120);
const LOCAL_GIT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub(crate) struct ResolvedTarget {
    pub cwd: PathBuf,
    pub working: bool,
    pub busy_reason: Option<String>,
}

/// Protocol dispatcher for `/app/git-pr/*` action routes.
pub(crate) fn request(
    app: &AppHandle,
    reader: &Reader,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let git_pr = app.state::<GitPr>();
    match path {
        "/app/git-pr/open-url" => response::post(method, action, body, handle_open_url),
        "/app/git-pr/prepare" => response::post(method, action, body, |payload| {
            handle_prepare(app, reader, &git_pr, &payload)
        }),
        "/app/git-pr/commit" => response::post(method, action, body, |payload| {
            handle_commit(app, reader, &git_pr, &payload)
        }),
        "/app/git-pr/pull" => response::post(method, action, body, |payload| {
            handle_pull(app, reader, &git_pr, &payload)
        }),
        "/app/git-pr/push" => response::post(method, action, body, |payload| {
            handle_push(app, reader, &git_pr, &payload)
        }),
        "/app/git-pr/pr/create" => response::post(method, action, body, |payload| {
            handle_create_pr(app, reader, &git_pr, &payload)
        }),
        "/app/git-pr/pr/merge" => response::post(method, action, body, |payload| {
            handle_merge_pr(app, reader, &git_pr, &payload)
        }),
        _ => Response::not_found(),
    }
}

fn handle_open_url(payload: Value) -> Response {
    let raw = payload["url"].as_str().unwrap_or("");
    match validate_github_url(raw) {
        Ok(valid) => {
            crate::browse_address(&valid);
            Response::json(&json!({ "ok": true }))
        }
        Err(message) => Response::error(400, &message),
    }
}

fn handle_prepare(app: &AppHandle, reader: &Reader, git_pr: &GitPr, payload: &Value) -> Response {
    let op = payload["op"].as_str().unwrap_or("");
    let target = match resolve_target(app, reader, payload) {
        Ok(t) => t,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    let git = match GitRunner::new() {
        Ok(g) => g,
        Err(e) => return Response::error(500, &e),
    };
    match prepare_op(git_pr, &git, &target, op, payload) {
        Ok(value) => Response::json(&value),
        Err((status, msg)) => Response::error(status, &msg),
    }
}

fn handle_commit(app: &AppHandle, reader: &Reader, git_pr: &GitPr, payload: &Value) -> Response {
    let target = match resolve_writable_target(app, reader, payload) {
        Ok(t) => t,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    let message = payload["message"].as_str().unwrap_or("");
    let stage_all = payload["stage_all"].as_bool().unwrap_or(true);
    let git = match GitRunner::new() {
        Ok(g) => g,
        Err(e) => return Response::error(500, &e),
    };
    match exec_commit(&git, &target.cwd, message, stage_all) {
        Ok(value) => {
            git_pr.invalidate_cwd(&target.cwd);
            Response::json(&value)
        }
        Err((status, msg)) => Response::error(status, &msg),
    }
}

fn handle_pull(app: &AppHandle, reader: &Reader, git_pr: &GitPr, payload: &Value) -> Response {
    let target = match resolve_writable_target(app, reader, payload) {
        Ok(t) => t,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    let git = match GitRunner::new() {
        Ok(g) => g,
        Err(e) => return Response::error(500, &e),
    };
    match exec_pull(&git, &target.cwd) {
        Ok(value) => {
            git_pr.invalidate_cwd(&target.cwd);
            Response::json(&value)
        }
        Err((status, msg)) => Response::error(status, &msg),
    }
}

fn handle_push(app: &AppHandle, reader: &Reader, git_pr: &GitPr, payload: &Value) -> Response {
    let target = match resolve_writable_target(app, reader, payload) {
        Ok(t) => t,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    let git = match GitRunner::new() {
        Ok(g) => g,
        Err(e) => return Response::error(500, &e),
    };
    match exec_push(&git, &target.cwd) {
        Ok(value) => {
            git_pr.invalidate_cwd(&target.cwd);
            Response::json(&value)
        }
        Err((status, msg)) => Response::error(status, &msg),
    }
}

fn handle_create_pr(app: &AppHandle, reader: &Reader, git_pr: &GitPr, payload: &Value) -> Response {
    let target = match resolve_writable_target(app, reader, payload) {
        Ok(t) => t,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    if !git_pr.is_gh_ready(Instant::now()) {
        return Response::error(400, "未安装或未登录 gh，无法创建 PR");
    }
    let title = payload["title"].as_str().unwrap_or("");
    let body_text = payload["body"].as_str().unwrap_or("");
    let base = payload["base"].as_str().filter(|s| !s.trim().is_empty());
    let draft = payload["draft"].as_bool().unwrap_or(false);
    let git = match GitRunner::new() {
        Ok(g) => g,
        Err(e) => return Response::error(500, &e),
    };
    match exec_create_pr(
        &git,
        git_pr.gh_client(),
        &target.cwd,
        title,
        body_text,
        base,
        draft,
    ) {
        Ok(value) => {
            git_pr.invalidate_cwd(&target.cwd);
            if let Some(repo) = value["repo"].as_str() {
                git_pr.invalidate_pr(repo, value["head"].as_str(), value["number"].as_u64());
            }
            Response::json(&value)
        }
        Err((status, msg)) => Response::error(status, &msg),
    }
}

fn handle_merge_pr(app: &AppHandle, reader: &Reader, git_pr: &GitPr, payload: &Value) -> Response {
    let target = match resolve_writable_target(app, reader, payload) {
        Ok(t) => t,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    if !git_pr.is_gh_ready(Instant::now()) {
        return Response::error(400, "未安装或未登录 gh，无法合并 PR");
    }
    let repo = payload["repo"].as_str().unwrap_or("").trim();
    let number = payload["number"].as_u64().unwrap_or(0);
    let method = payload["method"].as_str().unwrap_or("").trim();
    match exec_merge_pr(git_pr.gh_client(), repo, number, method) {
        Ok(value) => {
            git_pr.invalidate_cwd(&target.cwd);
            git_pr.invalidate_pr(repo, None, Some(number));
            Response::json(&value)
        }
        Err((status, msg)) => Response::error(status, &msg),
    }
}

fn resolve_writable_target(
    app: &AppHandle,
    reader: &Reader,
    payload: &Value,
) -> Result<ResolvedTarget, (u16, String)> {
    let target = resolve_target(app, reader, payload)?;
    if target.working {
        return Err((
            409,
            target
                .busy_reason
                .unwrap_or_else(|| "会话所在 CLI 正在工作，暂时无法执行 Git 操作".to_owned()),
        ));
    }
    Ok(target)
}

fn resolve_target(
    app: &AppHandle,
    reader: &Reader,
    payload: &Value,
) -> Result<ResolvedTarget, (u16, String)> {
    let kind = payload["target"]["kind"].as_str().unwrap_or("");
    let terminals = app.state::<Terminals>();
    match kind {
        "terminal" => {
            let id = payload["target"]["id"]
                .as_u64()
                .or_else(|| payload["target"]["id"].as_str()?.parse().ok())
                .ok_or_else(|| (400, "缺少终端 ID".to_owned()))?;
            let term = terminals
                .find_git_pr_terminal(id)
                .ok_or_else(|| (404, "找不到对应的终端会话".to_owned()))?;
            Ok(ResolvedTarget {
                cwd: term.cwd,
                working: term.working,
                busy_reason: term
                    .working
                    .then(|| "终端 CLI 正在工作，暂时无法执行 Git 操作".to_owned()),
            })
        }
        "workspace" => {
            let ws_id = payload["target"]["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| (400, "缺少工作区 ID".to_owned()))?;
            let all_terms = terminals.git_pr_targets();
            let own_term = all_terms
                .iter()
                .find(|t| !t.derived && t.workspaces.last().map(String::as_str) == Some(ws_id));
            let terminal_ws: HashSet<String> = all_terms
                .iter()
                .filter(|t| !t.derived)
                .filter_map(|t| t.workspaces.last().cloned())
                .collect();
            let ws = reader
                .resolve_git_pr_workspace(ws_id, &terminal_ws)
                .ok_or_else(|| (403, "仅 CLI 会话提供托盘 Git 操作".to_owned()))?;
            let cwd = own_term
                .map(|t| t.cwd.clone())
                .or(ws.cwd)
                .ok_or_else(|| (400, "无法确定该工作区的目录".to_owned()))?;
            let working = own_term.is_some_and(|t| t.working) || ws.working;
            Ok(ResolvedTarget {
                cwd,
                working,
                busy_reason: working
                    .then(|| "会话所在 CLI 正在工作，暂时无法执行 Git 操作".to_owned()),
            })
        }
        "isolated" => {
            let iso_id = payload["target"]["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| (400, "缺少隔离目录 ID".to_owned()))?;
            let dir = crate::isolated::read(reader.data_root())
                .into_iter()
                .find(|d| d.id == iso_id)
                .ok_or_else(|| (404, "找不到该隔离目录".to_owned()))?;
            if !dir.ready {
                return Err((
                    409,
                    "隔离目录处于未完成或已收起状态，无法执行 Git 操作".to_owned(),
                ));
            }
            let cwd = PathBuf::from(&dir.path);
            let mut working = terminals.has_active_terminal_for_isolated(&dir.id, &cwd)
                || reader.is_isolated_in_use(&cwd);
            if !working
                && let Ok(git) = GitRunner::new()
                && let Some(common_dir) = resolve_git_common_dir(&git, &cwd)
                && crate::isolated::leases::has_active_leases(&common_dir, &dir.id).unwrap_or(true)
            {
                working = true;
            }
            Ok(ResolvedTarget {
                cwd,
                working,
                busy_reason: working.then(|| {
                    "隔离目录仍有运行中的任务或派生终端持有租约，暂时无法执行 Git 操作".to_owned()
                }),
            })
        }
        _ => Err((400, "不支持的 Git 操作目标类型".to_owned())),
    }
}

fn resolve_git_common_dir(git: &GitRunner, cwd: &Path) -> Option<PathBuf> {
    let out = git
        .run(
            cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            &[0, 128],
            &[],
        )
        .ok()?;
    (out.code == 0).then(|| bytes_to_path(trim_line_end(&out.stdout)))
}

pub(crate) fn prepare_op(
    git_pr: &GitPr,
    git: &GitRunner,
    target: &ResolvedTarget,
    op: &str,
    payload: &Value,
) -> Result<Value, (u16, String)> {
    let status = inspect_workspace_git(git, &target.cwd)
        .ok_or_else(|| (400, "当前目录不是有效的 Git 仓库".to_owned()))?;
    match op {
        "commit" => {
            let files = list_changed_files(git, &target.cwd).map_err(|e| (400, e))?;
            let has_staged = files.iter().any(|f| f.staged);
            let has_unstaged = files.iter().any(|f| f.unstaged || f.untracked);
            let has_unmerged = files.iter().any(|f| f.unmerged);
            Ok(json!({
                "op": "commit",
                "branch": status.branch,
                "detached": status.detached,
                "working": target.working,
                "busy_reason": target.busy_reason,
                "files": files,
                "has_staged": has_staged,
                "has_unstaged": has_unstaged,
                "has_unmerged": has_unmerged,
            }))
        }
        "pull" => Ok(json!({
            "op": "pull",
            "branch": status.branch,
            "detached": status.detached,
            "upstream": status.upstream,
            "ahead": status.ahead,
            "behind": status.behind,
            "working": target.working,
            "busy_reason": target.busy_reason,
        })),
        "push" => {
            let plan = plan_push(git, &target.cwd, &status).map_err(|e| (400, e))?;
            Ok(json!({
                "op": "push",
                "branch": plan.branch,
                "remote": plan.remote,
                "remote_branch": plan.remote_branch,
                "set_upstream": plan.set_upstream,
                "upstream": status.upstream,
                "ahead": status.ahead,
                "behind": status.behind,
                "working": target.working,
                "busy_reason": target.busy_reason,
            }))
        }
        "create_pr" => prepare_create_pr(git_pr, git, target, &status),
        "merge_pr" => prepare_merge_pr(git_pr, target, payload),
        _ => Err((400, format!("不支持的操作：{op}"))),
    }
}

fn prepare_create_pr(
    git_pr: &GitPr,
    git: &GitRunner,
    target: &ResolvedTarget,
    status: &super::git::GitWorkspaceStatus,
) -> Result<Value, (u16, String)> {
    if !git_pr.is_gh_ready(Instant::now()) {
        return Err((400, "未安装或未登录 gh，无法创建 PR".to_owned()));
    }
    let branch = status
        .branch
        .as_deref()
        .filter(|_| !status.detached)
        .ok_or_else(|| (400, "当前处于游离 HEAD，无法创建 PR".to_owned()))?;
    let remote = resolve_push_remote(git, &target.cwd, branch).map_err(|e| (400, e))?;
    let repo = status
        .github_repo
        .clone()
        .or_else(|| resolve_github_repo(git, &target.cwd, &remote))
        .ok_or_else(|| (400, "当前仓库不是 GitHub 仓库，无法创建 PR".to_owned()))?;
    let (base, base_ref) = match resolve_default_branch(git, &target.cwd, &remote) {
        Some((name, full)) => (name, Some(full)),
        None => ("main".to_owned(), None),
    };
    if branch == base {
        return Err((
            400,
            format!("当前就在默认分支 {base} 上，无法对自己创建 PR"),
        ));
    }
    let (title, body) = default_pr_draft(git, &target.cwd, branch, base_ref.as_deref());
    let plan = plan_push(git, &target.cwd, status).map_err(|e| (400, e))?;
    let needs_push = plan.set_upstream || status.ahead > 0;
    Ok(json!({
        "op": "create_pr",
        "repo": repo,
        "head": branch,
        "base": base,
        "title": title,
        "body": body,
        "remote": plan.remote,
        "needs_push": needs_push,
        "working": target.working,
        "busy_reason": target.busy_reason,
    }))
}

fn prepare_merge_pr(
    git_pr: &GitPr,
    target: &ResolvedTarget,
    payload: &Value,
) -> Result<Value, (u16, String)> {
    if !git_pr.is_gh_ready(Instant::now()) {
        return Err((400, "未安装或未登录 gh，无法合并 PR".to_owned()));
    }
    let repo = payload["repo"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| (400, "缺少仓库标识".to_owned()))?;
    let number = payload["number"]
        .as_u64()
        .filter(|&n| n > 0)
        .ok_or_else(|| (400, "缺少 PR 编号".to_owned()))?;
    let settings = git_pr
        .gh_client()
        .view_repo_merge_settings(repo)
        .map_err(gh_error_to_http)?;
    let allowed = settings.allowed_methods();
    if allowed.is_empty() {
        return Err((400, "该仓库未启用任何 PR 合并方式".to_owned()));
    }
    Ok(json!({
        "op": "merge_pr",
        "repo": repo,
        "number": number,
        "allowed_methods": allowed,
        "default_method": settings.effective_default(),
        "delete_branch_on_merge": settings.delete_branch_on_merge,
        "working": target.working,
        "busy_reason": target.busy_reason,
    }))
}

struct PushPlan {
    branch: String,
    remote: String,
    remote_branch: String,
    set_upstream: bool,
}

fn plan_push(
    git: &GitRunner,
    cwd: &Path,
    status: &super::git::GitWorkspaceStatus,
) -> Result<PushPlan, String> {
    let branch = status
        .branch
        .as_deref()
        .filter(|_| !status.detached)
        .ok_or_else(|| "当前处于游离 HEAD，无法推送".to_owned())?;
    let remote = resolve_push_remote(git, cwd, branch)?;
    let upstream_points_to_default = status
        .upstream
        .as_deref()
        .zip(status.default_branch.as_deref())
        .is_some_and(|(up, def)| branch != def && up.ends_with(&format!("/{def}")));
    let set_upstream = status.upstream.is_none() || upstream_points_to_default;
    let remote_branch = if set_upstream {
        branch.to_owned()
    } else {
        status
            .upstream
            .as_deref()
            .and_then(|u| u.split_once('/').map(|(_, b)| b.to_owned()))
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| branch.to_owned())
    };
    Ok(PushPlan {
        branch: branch.to_owned(),
        remote,
        remote_branch,
        set_upstream,
    })
}

/// Executes neutralized `git add -A` (when `stage_all`) + `git commit -m`.
pub(crate) fn exec_commit(
    git: &GitRunner,
    cwd: &Path,
    message: &str,
    stage_all: bool,
) -> Result<Value, (u16, String)> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err((400, "请填写提交说明".to_owned()));
    }
    let files = list_changed_files(git, cwd).map_err(|e| (400, e))?;
    if files.iter().any(|f| f.unmerged) {
        return Err((409, "工作区存在未解决的合并冲突，无法提交".to_owned()));
    }
    if stage_all {
        if files.is_empty() {
            return Err((400, "工作区没有可提交的改动".to_owned()));
        }
        run_git_raw(git, cwd, &["add", "-A"], LOCAL_GIT_TIMEOUT)?;
    } else if !files.iter().any(|f| f.staged) {
        return Err((400, "暂存区没有可提交的改动".to_owned()));
    }
    let output_text = run_git_raw(git, cwd, &["commit", "-m", trimmed], LOCAL_GIT_TIMEOUT)?;
    let sha = git_stdout(git, cwd, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "sha": sha,
        "output": output_text,
    }))
}

/// Executes `git fetch` followed by strict fast-forward `git merge --ff-only @{u}`.
pub(crate) fn exec_pull(git: &GitRunner, cwd: &Path) -> Result<Value, (u16, String)> {
    let status = inspect_workspace_git(git, cwd)
        .ok_or_else(|| (400, "当前目录不是有效的 Git 仓库".to_owned()))?;
    let branch = status
        .branch
        .as_deref()
        .filter(|_| !status.detached)
        .ok_or_else(|| (400, "当前处于游离 HEAD，无法拉取".to_owned()))?;
    let upstream = status
        .upstream
        .as_deref()
        .ok_or_else(|| (400, "当前分支未配置上游分支，无法拉取".to_owned()))?;
    let remote = resolve_remote_name(git, cwd, Some(branch));
    let before = git_stdout(git, cwd, &["rev-parse", "HEAD"]).unwrap_or_default();

    run_git_raw(git, cwd, &["fetch", "--", &remote], NETWORK_GIT_TIMEOUT)?;
    let merge_out = git
        .run_with_timeout(
            cwd,
            &["merge", "--ff-only", "@{u}"],
            &[0, 1, 128],
            &[],
            LOCAL_GIT_TIMEOUT,
        )
        .map_err(|e| (500, e))?;
    let raw_text = join_git_output(&merge_out.stderr, &merge_out.stdout);
    if merge_out.code != 0 {
        let diverged = is_diverged_from_upstream(git, cwd);
        let prefix = if diverged {
            "分支已分叉，无法快进拉取（未自动合并或变基）：\n"
        } else {
            ""
        };
        return Err((409, format!("{prefix}{raw_text}")));
    }
    let after = git_stdout(git, cwd, &["rev-parse", "HEAD"]).unwrap_or_default();
    let short_sha = git_stdout(git, cwd, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "branch": branch,
        "upstream": upstream,
        "up_to_date": !before.is_empty() && before == after,
        "sha": short_sha,
        "output": raw_text,
    }))
}

/// Pushes current branch without `--force`; sets upstream on first push (or when upstream was base).
pub(crate) fn exec_push(git: &GitRunner, cwd: &Path) -> Result<Value, (u16, String)> {
    let status = inspect_workspace_git(git, cwd)
        .ok_or_else(|| (400, "当前目录不是有效的 Git 仓库".to_owned()))?;
    let plan = plan_push(git, cwd, &status).map_err(|e| (400, e))?;
    let refspec = format!("HEAD:refs/heads/{}", plan.remote_branch);
    let args: Vec<&str> = if plan.set_upstream {
        vec!["push", "-u", "--", &plan.remote, &refspec]
    } else {
        vec!["push", "--", &plan.remote, &refspec]
    };
    let output_text = run_git_raw(git, cwd, &args, NETWORK_GIT_TIMEOUT)?;
    Ok(json!({
        "ok": true,
        "branch": plan.branch,
        "remote": plan.remote,
        "upstream": format!("{}/{}", plan.remote, plan.remote_branch),
        "set_upstream": plan.set_upstream,
        "output": output_text,
    }))
}

/// Pushes current branch if needed, then creates a GitHub PR via `gh pr create`.
pub(crate) fn exec_create_pr(
    git: &GitRunner,
    gh: &GhClient,
    cwd: &Path,
    title: &str,
    body: &str,
    base_override: Option<&str>,
    draft: bool,
) -> Result<Value, (u16, String)> {
    let title = title.trim();
    if title.is_empty() {
        return Err((400, "请填写 PR 标题".to_owned()));
    }
    let status = inspect_workspace_git(git, cwd)
        .ok_or_else(|| (400, "当前目录不是有效的 Git 仓库".to_owned()))?;
    let branch = status
        .branch
        .as_deref()
        .filter(|_| !status.detached)
        .ok_or_else(|| (400, "当前处于游离 HEAD，无法创建 PR".to_owned()))?;
    let plan = plan_push(git, cwd, &status).map_err(|e| (400, e))?;
    let repo = status
        .github_repo
        .clone()
        .or_else(|| resolve_github_repo(git, cwd, &plan.remote))
        .ok_or_else(|| (400, "当前仓库不是 GitHub 仓库，无法创建 PR".to_owned()))?;
    let base = base_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| resolve_default_branch(git, cwd, &plan.remote).map(|(n, _)| n))
        .unwrap_or_else(|| "main".to_owned());
    if branch == base {
        return Err((
            400,
            format!("当前就在默认分支 {base} 上，无法对自己创建 PR"),
        ));
    }
    if plan.set_upstream || status.ahead > 0 {
        exec_push(git, cwd)?;
    }
    let created = gh
        .create_pr(&repo, &base, branch, title, body, draft)
        .map_err(gh_error_to_http)?;
    Ok(json!({
        "ok": true,
        "repo": repo,
        "head": branch,
        "base": base,
        "url": created.url,
        "number": created.number,
    }))
}

/// Validates `method` against repo's allowed merge strategies, then calls `gh pr merge`.
pub(crate) fn exec_merge_pr(
    gh: &GhClient,
    repo: &str,
    number: u64,
    method: &str,
) -> Result<Value, (u16, String)> {
    if repo.is_empty() || number == 0 {
        return Err((400, "缺少 PR 仓库或编号".to_owned()));
    }
    if !matches!(method, "merge" | "squash" | "rebase") {
        return Err((400, format!("不支持的合并方式：{method}")));
    }
    let settings = gh
        .view_repo_merge_settings(repo)
        .map_err(gh_error_to_http)?;
    if !settings.allowed_methods().contains(&method) {
        return Err((400, format!("该仓库不允许使用 {method} 方式合并 PR")));
    }
    let output_text = gh
        .merge_pr(repo, number, method)
        .map_err(gh_error_to_http)?;
    Ok(json!({
        "ok": true,
        "repo": repo,
        "number": number,
        "method": method,
        "delete_branch_on_merge": settings.delete_branch_on_merge,
        "output": output_text,
    }))
}

fn is_diverged_from_upstream(git: &GitRunner, cwd: &Path) -> bool {
    let Some(counts) = git_stdout(
        git,
        cwd,
        &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
    ) else {
        return false;
    };
    let mut parts = counts.split_whitespace();
    let ahead: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    ahead > 0 && behind > 0
}

fn run_git_raw(
    git: &GitRunner,
    cwd: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, (u16, String)> {
    let out = git
        .run_with_timeout(cwd, args, &[0, 1, 128], &[], timeout)
        .map_err(|e| (500, e))?;
    let text = join_git_output(&out.stderr, &out.stdout);
    if out.code != 0 {
        let detail = if text.is_empty() {
            format!(
                "git {} failed (exit {})",
                args.first().copied().unwrap_or(""),
                out.code
            )
        } else {
            text
        };
        return Err((400, detail));
    }
    Ok(text)
}

fn join_git_output(stderr: &[u8], stdout: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr).trim().to_owned();
    let out = String::from_utf8_lossy(stdout).trim().to_owned();
    match (err.is_empty(), out.is_empty()) {
        (false, true) => err,
        (true, false) => out,
        (false, false) => format!("{err}\n{out}"),
        (true, true) => String::new(),
    }
}

fn gh_error_to_http(err: GhError) -> (u16, String) {
    match err {
        GhError::NotInstalled => (400, "未安装 gh CLI".to_owned()),
        GhError::NotAuthenticated => (401, "gh 尚未登录 GitHub，请先执行 gh auth login".to_owned()),
        GhError::RateLimited => (429, "GitHub API 触发速率限制，请稍后重试".to_owned()),
        GhError::NotFound => (404, "找不到对应的仓库或 PR".to_owned()),
        GhError::Other(msg) => (400, msg),
    }
}

impl GitPr {
    pub(crate) fn gh_client(&self) -> &GhClient {
        &self.inner.gh
    }

    pub(crate) fn is_gh_ready(&self, now: Instant) -> bool {
        self.resolve_gh_status(now, false) == GhStatus::Ready
    }

    pub(crate) fn invalidate_cwd(&self, cwd: &Path) {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .invalidate_cwd(cwd);
    }

    pub(crate) fn invalidate_pr(&self, repo: &str, branch: Option<&str>, number: Option<u64>) {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .invalidate_pr(repo, branch, number);
    }
}

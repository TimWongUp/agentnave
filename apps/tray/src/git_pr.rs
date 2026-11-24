//! Read-only Git & GitHub PR status for CLI sessions and isolated directories (ADR-0115).
//!
//! Strictly read-only, in-memory only (nothing written to the data root), and scoped strictly to
//! CLI sessions (embedded terminals, derived sessions, terminal-CLI boss workspaces, and isolated
//! directories). Desktop host workspaces are excluded; unknown hosts default to desktop.

mod cache;
pub(crate) mod gh;
pub(crate) mod git;
mod links;
pub(crate) mod ops;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tauri::{AppHandle, Manager};

use crate::data::Reader;
use crate::data::boss_app::{ANTIGRAVITY_CLI, CLAUDE_CLI, CODEX_CLI, Homes};
use crate::isolated::git::GitRunner;
use crate::response::{self, Response};
use crate::terminal::Terminals;

use cache::Cache;
use gh::{
    GhClient, GhError, GhStatus, MergedPrSummary, PrInfo, pr_merged_for_worktree, pr_priority_rank,
};
use git::{
    GitWorkspaceStatus, inspect_workspace_git, resolve_default_branch, resolve_github_repo,
    resolve_remote_name,
};

/// Maximum number of recent CLI boss workspaces inspected in one refresh pass.
const MAX_CLI_WORKSPACES: usize = 24;

/// Managed Tauri state for in-memory read-only Git/PR status and refresh coalescing.
#[derive(Clone)]
pub(crate) struct GitPr {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct RefreshState {
    active: bool,
    started_gen: u64,
    completed_gen: u64,
    completed_manual_gen: u64,
}

struct Inner {
    cache: Mutex<Cache>,
    refreshing: Mutex<RefreshState>,
    done: Condvar,
    gh: GhClient,
}

struct RefreshGuard<'a> {
    inner: &'a Inner,
    gen_id: u64,
    manual: bool,
    completed: bool,
}

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .refreshing
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if self.completed {
            state.completed_gen = self.gen_id;
            if self.manual {
                state.completed_manual_gen = self.gen_id;
            }
        }
        state.active = false;
        self.inner.done.notify_all();
    }
}

impl Default for GitPr {
    fn default() -> Self {
        Self::with_client(GhClient::system())
    }
}

impl GitPr {
    pub(crate) fn with_client(gh: GhClient) -> Self {
        Self {
            inner: Arc::new(Inner {
                cache: Mutex::default(),
                refreshing: Mutex::default(),
                done: Condvar::new(),
                gh,
            }),
        }
    }

    pub(crate) fn snapshot(&self) -> Value {
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .snapshot()
    }

    /// Runs a coalesced refresh pass and returns the updated in-memory snapshot.
    /// Callers requesting fresh data (`manual`, `turn_end`, `focus`, `hover`) wait for any
    /// in-flight pass and run a new pass unless another pass started after they arrived.
    pub(crate) fn refresh(&self, input: RefreshInput) -> Value {
        let manual = input.reason == "manual";
        let need_fresh = matches!(
            input.reason.as_str(),
            "turn_end" | "focus" | "manual" | "hover"
        );
        let gen_id = {
            let mut state = self
                .inner
                .refreshing
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if state.active {
                let min_gen = state.started_gen;
                while state.active {
                    state = self
                        .inner
                        .done
                        .wait(state)
                        .unwrap_or_else(|p| p.into_inner());
                }
                let satisfied = if !need_fresh {
                    true
                } else if manual {
                    state.completed_manual_gen > min_gen
                } else {
                    state.completed_gen > min_gen
                };
                if satisfied {
                    return self.snapshot();
                }
            }
            state.active = true;
            state.started_gen = state.started_gen.saturating_add(1);
            state.started_gen
        };
        let mut guard = RefreshGuard {
            inner: &self.inner,
            gen_id,
            manual,
            completed: false,
        };
        let snapshot = self.compute_snapshot(input);
        {
            let mut cache = self.inner.cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.set_snapshot(snapshot.clone());
        }
        guard.completed = true;
        drop(guard);
        snapshot
    }

    fn compute_snapshot(&self, input: RefreshInput) -> Value {
        let now = Instant::now();
        let bypass_git_ttl = matches!(
            input.reason.as_str(),
            "turn_end" | "focus" | "manual" | "hover"
        );
        let gh_status = self.resolve_gh_status(now, input.reason == "manual");
        let gh_ready = gh_status == GhStatus::Ready;
        let Ok(git_runner) = GitRunner::new() else {
            return json!({
                "gh_status": gh_status,
                "terminals": {},
                "workspaces": {},
                "isolated": {},
                "updated_at": now_unix_ms(),
            });
        };

        let mut git_cache: HashMap<PathBuf, Option<GitWorkspaceStatus>> = HashMap::new();
        let (terminal_map, terminal_prs) = self.inspect_terminals(
            &git_runner,
            &mut git_cache,
            &input,
            now,
            bypass_git_ttl,
            gh_ready,
        );
        let (isolated_map, isolated_prs_by_workspace) = self.inspect_isolated_dirs(
            &git_runner,
            &mut git_cache,
            &input.isolated,
            now,
            bypass_git_ttl,
            gh_ready,
        );
        let workspace_map = self.inspect_workspaces(
            &git_runner,
            &mut git_cache,
            &input,
            &terminal_prs,
            &isolated_prs_by_workspace,
            now,
            bypass_git_ttl,
            gh_ready,
        );

        json!({
            "gh_status": gh_status,
            "terminals": terminal_map,
            "workspaces": workspace_map,
            "isolated": isolated_map,
            "updated_at": now_unix_ms(),
        })
    }

    fn read_git_cached(
        &self,
        git_runner: &GitRunner,
        local: &mut HashMap<PathBuf, Option<GitWorkspaceStatus>>,
        cwd: &Path,
        now: Instant,
        bypass_ttl: bool,
    ) -> Option<GitWorkspaceStatus> {
        if let Some(found) = local.get(cwd) {
            return found.clone();
        }
        let cached = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_git(cwd, now, bypass_ttl);
        let value = match cached {
            Some(hit) => hit,
            None => {
                let measured = inspect_workspace_git(git_runner, cwd);
                self.inner
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .put_git(cwd.to_path_buf(), now, measured.clone());
                measured
            }
        };
        local.insert(cwd.to_path_buf(), value.clone());
        value
    }

    fn inspect_terminals(
        &self,
        git_runner: &GitRunner,
        git_cache: &mut HashMap<PathBuf, Option<GitWorkspaceStatus>>,
        input: &RefreshInput,
        now: Instant,
        bypass_git_ttl: bool,
        gh_ready: bool,
    ) -> (Map<String, Value>, HashMap<u64, Vec<PrInfo>>) {
        let mut terminal_map = Map::new();
        let mut terminal_prs = HashMap::new();
        for term in &input.terminals {
            let git_status =
                self.read_git_cached(git_runner, git_cache, &term.cwd, now, bypass_git_ttl);
            let log_files = links::terminal_log_files(
                &input.homes,
                &term.provider,
                term.native.as_deref(),
                term.account_dir.as_deref(),
            );
            let prs = if gh_ready {
                self.collect_session_prs(git_status.as_ref(), &log_files, now)
            } else {
                Vec::new()
            };
            terminal_prs.insert(term.id, prs.clone());
            terminal_map.insert(
                term.id.to_string(),
                json!({ "git": git_status, "prs": prs, "working": term.working }),
            );
        }
        (terminal_map, terminal_prs)
    }

    fn inspect_isolated_dirs(
        &self,
        git_runner: &GitRunner,
        git_cache: &mut HashMap<PathBuf, Option<GitWorkspaceStatus>>,
        isolated: &[IsolatedTarget],
        now: Instant,
        bypass_git_ttl: bool,
        gh_ready: bool,
    ) -> (Map<String, Value>, HashMap<String, Vec<PrInfo>>) {
        let mut isolated_map = Map::new();
        let mut prs_by_ws: HashMap<String, Vec<PrInfo>> = HashMap::new();
        for iso in isolated {
            let git_status = self.read_git_cached(
                git_runner,
                git_cache,
                Path::new(&iso.path),
                now,
                bypass_git_ttl,
            );
            let pr = if gh_ready {
                git_status
                    .as_ref()
                    .and_then(|g| g.github_repo.as_deref())
                    .and_then(|r| self.lookup_branch_pr(r, &iso.branch, now))
            } else {
                None
            };
            let merged_pr = match (&git_status, &pr) {
                (Some(g), Some(pr_info)) => {
                    let head_sha = g.head_sha.as_deref().unwrap_or("");
                    pr_merged_for_worktree(
                        pr_info,
                        &iso.branch,
                        g.default_branch.as_deref(),
                        head_sha,
                        g.integrated_into_default,
                    )
                    .then(|| MergedPrSummary::from(pr_info))
                }
                _ => None,
            };
            if let Some(pr_info) = &pr {
                prs_by_ws
                    .entry(iso.workspace.clone())
                    .or_default()
                    .push(pr_info.clone());
            }
            isolated_map.insert(
                iso.id.clone(),
                json!({ "git": git_status, "pr": pr, "merged_pr": merged_pr }),
            );
        }
        (isolated_map, prs_by_ws)
    }

    #[allow(clippy::too_many_arguments)]
    fn inspect_workspaces(
        &self,
        git_runner: &GitRunner,
        git_cache: &mut HashMap<PathBuf, Option<GitWorkspaceStatus>>,
        input: &RefreshInput,
        terminal_prs: &HashMap<u64, Vec<PrInfo>>,
        isolated_prs_by_workspace: &HashMap<String, Vec<PrInfo>>,
        now: Instant,
        bypass_git_ttl: bool,
        gh_ready: bool,
    ) -> Map<String, Value> {
        let isolated_paths: Vec<PathBuf> = input
            .isolated
            .iter()
            .map(|i| PathBuf::from(&i.path))
            .collect();
        let mut workspace_map = Map::new();
        for ws in &input.workspaces {
            let own_term = input
                .terminals
                .iter()
                .find(|t| !t.derived && t.workspaces.last().map(String::as_str) == Some(&ws.id));
            let cwd = own_term.map(|t| t.cwd.as_path()).or(ws.cwd.as_deref());
            let git_status = cwd
                .and_then(|p| self.read_git_cached(git_runner, git_cache, p, now, bypass_git_ttl));
            let mut prs = if gh_ready {
                if let Some(term) = own_term {
                    terminal_prs.get(&term.id).cloned().unwrap_or_default()
                } else {
                    let log_files = match (&ws.host, &ws.owner_key) {
                        (Some(host), Some(key)) => links::workspace_log_files(
                            &input.homes,
                            host,
                            key,
                            ws.owner_file.as_deref(),
                            ws.account_dir.as_deref(),
                        ),
                        _ => Vec::new(),
                    };
                    self.collect_session_prs(git_status.as_ref(), &log_files, now)
                }
            } else {
                Vec::new()
            };
            if gh_ready {
                for child in input
                    .terminals
                    .iter()
                    .filter(|t| t.derived && t.parent_workspace_id.as_deref() == Some(&ws.id))
                {
                    if let Some(child_prs) = terminal_prs.get(&child.id) {
                        prs.extend(child_prs.iter().cloned());
                    }
                }
                if let Some(iso_prs) = isolated_prs_by_workspace.get(&ws.id) {
                    prs.extend(iso_prs.iter().cloned());
                }
                dedupe_and_sort_prs(&mut prs);
            }
            let derived_working = cwd.is_some_and(|dir| {
                input.terminals.iter().any(|t| {
                    t.derived
                        && t.working
                        && t.isolation_id.is_none()
                        && crate::data::is_non_isolated_in_dir(&t.cwd, dir, &isolated_paths)
                })
            });
            let ws_working = own_term.is_some_and(|t| t.working) || ws.working || derived_working;
            workspace_map.insert(
                ws.id.clone(),
                json!({ "git": git_status, "prs": prs, "working": ws_working }),
            );
        }
        workspace_map
    }

    fn resolve_gh_status(&self, now: Instant, force: bool) -> GhStatus {
        let cached = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_gh_status(now, force);
        if let Some(status) = cached {
            return status;
        }
        let status = self.inner.gh.check_status();
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .put_gh_status(now, status);
        status
    }

    fn collect_session_prs(
        &self,
        git: Option<&GitWorkspaceStatus>,
        log_files: &[PathBuf],
        now: Instant,
    ) -> Vec<PrInfo> {
        let mut prs = Vec::new();
        if let Some(g) = git
            && let (Some(repo), Some(branch)) = (g.github_repo.as_deref(), g.branch.as_deref())
            && Some(branch) != g.default_branch.as_deref()
            && let Some(pr) = self.lookup_branch_pr(repo, branch, now)
        {
            prs.push(pr);
        }
        let linked = {
            let mut cache = self.inner.cache.lock().unwrap_or_else(|p| p.into_inner());
            links::extract_pr_links_from_files(&mut cache, log_files)
        };
        for (repo, num) in linked {
            if let Some(pr) = self.lookup_number_pr(&repo, num, now) {
                prs.push(pr);
            }
        }
        dedupe_and_sort_prs(&mut prs);
        prs
    }

    fn lookup_branch_pr(&self, repo: &str, branch: &str, now: Instant) -> Option<PrInfo> {
        let cached = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_pr_by_branch(repo, branch, now);
        if let Some(hit) = cached {
            return hit;
        }
        let fetched = match self.inner.gh.view_pr_branch(repo, branch) {
            Ok(pr) => pr,
            Err(GhError::NotFound) => None,
            Err(GhError::NotInstalled) => {
                self.inner
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .put_gh_status(now, GhStatus::NotInstalled);
                return None;
            }
            Err(GhError::NotAuthenticated) => {
                self.inner
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .put_gh_status(now, GhStatus::NotAuthenticated);
                return None;
            }
            Err(GhError::RateLimited | GhError::HeadMismatch(_) | GhError::Other(_)) => {
                return None;
            }
        };
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .put_pr_by_branch(repo, branch, now, fetched.clone());
        fetched
    }

    fn lookup_number_pr(&self, repo: &str, number: u64, now: Instant) -> Option<PrInfo> {
        let cached = self
            .inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_pr_by_number(repo, number, now);
        if let Some(hit) = cached {
            return hit;
        }
        let fetched = match self.inner.gh.view_pr_number(repo, number) {
            Ok(pr) => pr,
            Err(GhError::NotFound) => None,
            Err(GhError::NotInstalled) => {
                self.inner
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .put_gh_status(now, GhStatus::NotInstalled);
                return None;
            }
            Err(GhError::NotAuthenticated) => {
                self.inner
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .put_gh_status(now, GhStatus::NotAuthenticated);
                return None;
            }
            Err(GhError::RateLimited | GhError::HeadMismatch(_) | GhError::Other(_)) => {
                return None;
            }
        };
        self.inner
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .put_pr_by_number(repo, number, now, fetched.clone());
        fetched
    }
}

/// Whether a boss workspace (without an embedded terminal) is strictly a terminal CLI session
/// (ADR-0115 §1). Desktop apps (`claude-desktop`, `codex-desktop`, `antigravity-desktop`,
/// `workbuddy`) and hosts whose desktop and CLI cannot be reliably distinguished (`zcode`,
/// `deepseek`, or unconfirmed `claude`/`codex`/`antigravity`) return `false`.
pub(crate) fn is_cli_boss(host: Option<&str>, app: Option<&str>) -> bool {
    match (host, app) {
        (Some("claude"), Some(CLAUDE_CLI)) => true,
        (Some("codex"), Some(CODEX_CLI)) => true,
        (Some("antigravity"), Some(ANTIGRAVITY_CLI)) => true,
        (
            Some(
                "claude" | "codex" | "antigravity" | "workbuddy" | "deepseek" | "hermes" | "zcode",
            ),
            _,
        ) => false,
        (Some("codebuddy" | "grok" | "pi" | "opencode" | "gemini"), _) => true,
        _ => false,
    }
}

/// Checks whether an isolated directory's managed branch has been merged via a GitHub PR
/// (t3code `storageCleanupPullRequestMerged`, ADR-0115 §5). Returns `None` when `gh` is
/// unavailable or no matching merged PR is found so callers fall back to existing Git checks.
pub(crate) fn inspect_merged_pr_for_isolated(
    git_runner: &GitRunner,
    gh: &GhClient,
    repo_dir: &Path,
    branch: &str,
    head_oid: &str,
) -> Option<MergedPrSummary> {
    if branch.is_empty() || head_oid.is_empty() {
        return None;
    }
    let remote = resolve_remote_name(git_runner, repo_dir, Some(branch));
    let github_repo = resolve_github_repo(git_runner, repo_dir, &remote)?;
    let (default_branch, default_ref) = match resolve_default_branch(git_runner, repo_dir, &remote)
    {
        Some((name, full)) => (Some(name), Some(full)),
        None => (None, None),
    };
    let integrated = default_ref
        .as_deref()
        .and_then(|base| git::count_commits_ahead(git_runner, repo_dir, base, head_oid))
        .is_some_and(|ahead| ahead == 0);
    let pr = gh.view_pr_branch(&github_repo, branch).ok()??;
    pr_merged_for_worktree(&pr, branch, default_branch.as_deref(), head_oid, integrated)
        .then(|| MergedPrSummary::from(&pr))
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalTarget {
    pub id: u64,
    pub provider: String,
    pub cwd: PathBuf,
    pub native: Option<String>,
    pub account_dir: Option<PathBuf>,
    pub workspaces: Vec<String>,
    pub derived: bool,
    pub parent_workspace_id: Option<String>,
    pub isolation_id: Option<String>,
    pub working: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceTarget {
    pub id: String,
    pub host: Option<String>,
    pub owner_key: Option<String>,
    pub owner_file: Option<PathBuf>,
    pub account_dir: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub working: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct IsolatedTarget {
    pub id: String,
    pub workspace: String,
    pub path: String,
    pub branch: String,
}

pub(crate) struct RefreshInput {
    pub reason: String,
    pub homes: Homes,
    pub terminals: Vec<TerminalTarget>,
    pub workspaces: Vec<WorkspaceTarget>,
    pub isolated: Vec<IsolatedTarget>,
}

/// Protocol route handler for `/app/git-pr/*`.
pub(crate) fn request(
    app: &AppHandle,
    reader: &Reader,
    method: &str,
    path: &str,
    action: Option<&str>,
    body: &[u8],
) -> Response {
    let git_pr = app.state::<GitPr>();
    match (method, path) {
        ("GET", "/app/git-pr/state") => Response::json(&git_pr.snapshot()),
        (_, "/app/git-pr/refresh") => response::post(method, action, body, |payload| {
            let reason = payload["reason"].as_str().unwrap_or("focus").to_owned();
            let selected = payload["selected"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
            let mut terminals = app.state::<Terminals>().git_pr_targets();
            let terminal_ws: HashSet<String> = terminals
                .iter()
                .filter(|t| !t.derived)
                .filter_map(|t| t.workspaces.last().cloned())
                .collect();
            let (homes, workspaces, isolated) =
                reader.git_pr_targets(selected.as_deref(), &terminal_ws, MAX_CLI_WORKSPACES);
            let isolated_paths: Vec<PathBuf> =
                isolated.iter().map(|i| PathBuf::from(&i.path)).collect();
            let running_cwds = reader.running_cwds();
            for term in &mut terminals {
                if !term.working
                    && running_cwds
                        .iter()
                        .any(|c| crate::data::is_non_isolated_in_dir(c, &term.cwd, &isolated_paths))
                {
                    term.working = true;
                }
            }
            let snap = git_pr.refresh(RefreshInput {
                reason,
                homes,
                terminals,
                workspaces,
                isolated,
            });
            Response::json(&snap)
        }),
        _ => ops::request(app, reader, method, path, action, body),
    }
}

fn dedupe_and_sort_prs(prs: &mut Vec<PrInfo>) {
    let mut seen: HashSet<(String, u64)> = HashSet::new();
    prs.retain(|pr| seen.insert((pr.repo.to_ascii_lowercase(), pr.number)));
    prs.sort_by(|a, b| {
        pr_priority_rank(b)
            .cmp(&pr_priority_rank(a))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| b.number.cmp(&a.number))
    });
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

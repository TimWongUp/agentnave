//! In-memory tiered cache for Git status, PR snapshots, and native session log PR links (ADR-0115).
//!
//! Strictly in-memory: never persists anything to the AgentNave data root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use super::gh::{GhStatus, PrInfo};
use super::git::GitWorkspaceStatus;

/// Open/draft PRs and missing-PR branch lookups cache for 60 seconds.
pub(crate) const OPEN_PR_TTL: Duration = Duration::from_secs(60);
/// Merged/closed PRs rarely change and cache for 10 minutes.
pub(crate) const CLOSED_PR_TTL: Duration = Duration::from_secs(600);
/// Local Git status cache TTL for non-forced refreshes.
pub(crate) const GIT_STATUS_TTL: Duration = Duration::from_secs(3);
/// `gh auth status` cache TTL when ready vs unavailable.
pub(crate) const GH_READY_TTL: Duration = Duration::from_secs(300);
pub(crate) const GH_UNAVAILABLE_TTL: Duration = Duration::from_secs(60);

const MAX_CACHED_ENTRIES: usize = 512;

#[derive(Clone)]
struct Timed<T> {
    at: Instant,
    value: T,
}

#[derive(Clone)]
struct CachedFileLinks {
    len: u64,
    modified: Option<SystemTime>,
    links: Vec<(String, u64)>,
}

#[derive(Default)]
pub(crate) struct Cache {
    gh_status: Option<Timed<GhStatus>>,
    git_by_cwd: HashMap<PathBuf, Timed<Option<GitWorkspaceStatus>>>,
    pr_by_number: HashMap<(String, u64), Timed<Option<PrInfo>>>,
    pr_by_branch: HashMap<(String, String), Timed<Option<PrInfo>>>,
    links_by_file: HashMap<PathBuf, CachedFileLinks>,
    snapshot: Option<Value>,
}

impl Cache {
    pub(crate) fn snapshot(&self) -> Value {
        self.snapshot.clone().unwrap_or_else(|| {
            json!({
                "gh_status": "unknown",
                "terminals": {},
                "workspaces": {},
                "isolated": {},
                "updated_at": 0,
            })
        })
    }

    pub(crate) fn set_snapshot(&mut self, value: Value) {
        self.snapshot = Some(value);
    }

    pub(crate) fn get_gh_status(&self, now: Instant, force: bool) -> Option<GhStatus> {
        if force {
            return None;
        }
        let entry = self.gh_status.as_ref()?;
        let ttl = if entry.value == GhStatus::Ready {
            GH_READY_TTL
        } else {
            GH_UNAVAILABLE_TTL
        };
        (now.saturating_duration_since(entry.at) < ttl).then_some(entry.value)
    }

    pub(crate) fn put_gh_status(&mut self, now: Instant, status: GhStatus) {
        self.gh_status = Some(Timed {
            at: now,
            value: status,
        });
    }

    pub(crate) fn get_git(
        &self,
        cwd: &Path,
        now: Instant,
        bypass_ttl: bool,
    ) -> Option<Option<GitWorkspaceStatus>> {
        if bypass_ttl {
            return None;
        }
        let entry = self.git_by_cwd.get(cwd)?;
        (now.saturating_duration_since(entry.at) < GIT_STATUS_TTL).then(|| entry.value.clone())
    }

    pub(crate) fn put_git(
        &mut self,
        cwd: PathBuf,
        now: Instant,
        status: Option<GitWorkspaceStatus>,
    ) {
        prune_if_full(&mut self.git_by_cwd);
        self.git_by_cwd.insert(
            cwd,
            Timed {
                at: now,
                value: status,
            },
        );
    }

    pub(crate) fn get_pr_by_number(
        &self,
        repo: &str,
        number: u64,
        now: Instant,
    ) -> Option<Option<PrInfo>> {
        let key = (repo.to_ascii_lowercase(), number);
        let entry = self.pr_by_number.get(&key)?;
        let ttl = pr_ttl(entry.value.as_ref());
        (now.saturating_duration_since(entry.at) < ttl).then(|| entry.value.clone())
    }

    pub(crate) fn put_pr_by_number(
        &mut self,
        repo: &str,
        number: u64,
        now: Instant,
        pr: Option<PrInfo>,
    ) {
        prune_if_full(&mut self.pr_by_number);
        let key = (repo.to_ascii_lowercase(), number);
        self.pr_by_number.insert(key, Timed { at: now, value: pr });
    }

    pub(crate) fn get_pr_by_branch(
        &self,
        repo: &str,
        branch: &str,
        now: Instant,
    ) -> Option<Option<PrInfo>> {
        let key = (repo.to_ascii_lowercase(), branch.to_owned());
        let entry = self.pr_by_branch.get(&key)?;
        let ttl = pr_ttl(entry.value.as_ref());
        (now.saturating_duration_since(entry.at) < ttl).then(|| entry.value.clone())
    }

    pub(crate) fn put_pr_by_branch(
        &mut self,
        repo: &str,
        branch: &str,
        now: Instant,
        pr: Option<PrInfo>,
    ) {
        prune_if_full(&mut self.pr_by_branch);
        let key = (repo.to_ascii_lowercase(), branch.to_owned());
        if let Some(info) = &pr {
            self.put_pr_by_number(&info.repo, info.number, now, Some(info.clone()));
        }
        self.pr_by_branch.insert(key, Timed { at: now, value: pr });
    }

    pub(crate) fn get_file_links(
        &self,
        path: &Path,
        len: u64,
        modified: Option<SystemTime>,
    ) -> Option<Vec<(String, u64)>> {
        let entry = self.links_by_file.get(path)?;
        (entry.len == len && entry.modified == modified).then(|| entry.links.clone())
    }

    pub(crate) fn put_file_links(
        &mut self,
        path: PathBuf,
        len: u64,
        modified: Option<SystemTime>,
        links: Vec<(String, u64)>,
    ) {
        if self.links_by_file.len() >= MAX_CACHED_ENTRIES && !self.links_by_file.contains_key(&path)
        {
            self.links_by_file.clear();
        }
        self.links_by_file.insert(
            path,
            CachedFileLinks {
                len,
                modified,
                links,
            },
        );
    }

    pub(crate) fn invalidate_cwd(&mut self, cwd: &Path) {
        self.git_by_cwd.remove(cwd);
    }

    pub(crate) fn invalidate_pr(&mut self, repo: &str, branch: Option<&str>, number: Option<u64>) {
        let lower = repo.to_ascii_lowercase();
        if let Some(num) = number {
            self.pr_by_number.remove(&(lower.clone(), num));
        }
        if let Some(br) = branch {
            self.pr_by_branch.remove(&(lower.clone(), br.to_owned()));
        } else if let Some(num) = number {
            self.pr_by_branch.retain(|(r, _), entry| {
                r != &lower || entry.value.as_ref().is_none_or(|p| p.number != num)
            });
        }
    }
}

pub(crate) fn pr_ttl(pr: Option<&PrInfo>) -> Duration {
    match pr.map(|p| p.state.as_str()) {
        Some("merged" | "closed") => CLOSED_PR_TTL,
        _ => OPEN_PR_TTL,
    }
}

fn prune_if_full<K, V>(map: &mut HashMap<K, Timed<V>>)
where
    K: Eq + std::hash::Hash,
{
    if map.len() >= MAX_CACHED_ENTRIES {
        map.clear();
    }
}

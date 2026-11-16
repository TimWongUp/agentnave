"""Change summaries of isolated directories, reported without raising to the caller."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from agentnave.isolated.release import STATUS_ARGS, porcelain_v2_paths
from agentnave.isolated.repo import IsolationError, decode_git_path, is_link, run_git
from agentnave.isolated.store import IsolatedRecord
from agentnave.isolated.worktree_state import (
    foreign_link_valid,
    nested_repositories,
    worktree_link_valid,
)

MAX_SUMMARY_FILES = 200

type SummaryStatus = Literal["complete", "no_baseline", "error"]


@dataclass(frozen=True, slots=True)
class ChangeSummary:
    id: str
    path: str
    branch: str
    summary_status: SummaryStatus
    head_oid: str | None = None
    actual_branch: str | None = None
    changed_files_from_base: tuple[str, ...] = ()
    truncated: bool = False
    uncommitted: bool | None = None
    nested_repos: tuple[str, ...] = ()
    error: str | None = None
    auto_released: bool = False


def _changed_paths(wt_path: Path, base_oid: str, status_raw: bytes) -> list[str]:
    """Files changed from base_oid: committed, staged, working tree, and untracked."""
    paths, _ = porcelain_v2_paths(status_raw)
    found = set(paths)
    diff_head = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=all",
        "--name-only",
        "-z",
    ]
    for revisions in ([base_oid, "HEAD"], ["--cached", base_oid], [base_oid]):
        raw = run_git([*diff_head, *revisions, "--"], cwd=wt_path).stdout
        found.update(decode_git_path(item) for item in raw.split(b"\x00") if item)
    return sorted(found)


def _read_head_and_branch(wt_path: Path) -> tuple[str, str | None]:
    head_oid = (
        run_git(["rev-parse", "--verify", "HEAD^{commit}"], cwd=wt_path)
        .stdout.strip()
        .decode("ascii", errors="replace")
    )
    sym_res = run_git(["symbolic-ref", "-q", "HEAD"], cwd=wt_path, allowed_exit_codes=(0, 1))
    actual_branch: str | None = None
    if sym_res.returncode == 0 and sym_res.stdout.strip():
        raw_ref = decode_git_path(sym_res.line)
        actual_branch = raw_ref.removeprefix("refs/heads/")
    return head_oid, actual_branch


def summarize_record_changes(record: IsolatedRecord) -> ChangeSummary:
    """Collect cumulative changes from base_oid without raising to the caller."""
    wt_path = Path(record.worktree_path)
    try:
        if is_link(wt_path) or not wt_path.is_dir():
            raise IsolationError(f"Isolated worktree is missing or unsafe: {wt_path}")
        if not worktree_link_valid(wt_path, record.worktree_git_dir, Path(record.repo_common_dir)):
            raise IsolationError("Isolated directory is no longer a valid git worktree")
        head_oid, actual_branch = _read_head_and_branch(wt_path)
        status_raw = run_git(STATUS_ARGS, cwd=wt_path).stdout
        combined = _changed_paths(wt_path, record.base_oid, status_raw)
        nested = tuple(nested_repositories(wt_path, status_raw))
        return ChangeSummary(
            id=record.id,
            path=record.worktree_path,
            branch=record.branch,
            summary_status="complete",
            head_oid=head_oid,
            actual_branch=actual_branch,
            changed_files_from_base=tuple(combined[:MAX_SUMMARY_FILES]),
            truncated=len(combined) > MAX_SUMMARY_FILES,
            uncommitted=bool(status_raw),
            nested_repos=nested,
        )
    except (IsolationError, OSError, ValueError) as exc:
        return ChangeSummary(
            id=record.id,
            path=record.worktree_path,
            branch=record.branch,
            summary_status="error",
            error=str(exc),
        )


def summarize_unregistered(isolation_id: str, wt_path: Path, branch: str) -> ChangeSummary:
    """Summary without a start commit, for a directory this data root has no registration of."""
    try:
        if is_link(wt_path) or not wt_path.is_dir():
            raise IsolationError(f"Isolated worktree is missing or unsafe: {wt_path}")
        if not foreign_link_valid(wt_path):
            raise IsolationError("Isolated directory is no longer a valid git worktree")
        head_oid, actual_branch = _read_head_and_branch(wt_path)
        status_raw = run_git(STATUS_ARGS, cwd=wt_path).stdout
        nested = tuple(nested_repositories(wt_path, status_raw))
        return ChangeSummary(
            id=isolation_id,
            path=str(wt_path),
            branch=branch,
            summary_status="no_baseline",
            head_oid=head_oid,
            actual_branch=actual_branch,
            uncommitted=bool(status_raw),
            nested_repos=nested,
        )
    except (IsolationError, OSError, ValueError) as exc:
        return ChangeSummary(
            id=isolation_id,
            path=str(wt_path),
            branch=branch,
            summary_status="error",
            error=str(exc),
        )

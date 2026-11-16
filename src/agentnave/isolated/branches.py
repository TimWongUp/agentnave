"""Managed branch checks shared by release, recovery and rollback."""

from __future__ import annotations

from pathlib import Path

from agentnave.isolated.repo import IsolationError, ref_exists, run_git
from agentnave.isolated.store import MANAGED_BRANCH_PREFIX, IsolatedRecord


def verify_managed_branch_ref(record: IsolatedRecord) -> None:
    """Refuse a record whose branch is not exactly its own refs/heads/agentnave/<id>."""
    expected = f"{MANAGED_BRANCH_PREFIX}{record.id}"
    if record.branch_ref != expected:
        raise IsolationError(f"Managed branch {record.branch_ref} does not match {expected}")


def has_unique_commits(cwd: Path, oid: str) -> bool:
    """Whether oid holds commits no durable branch reaches.

    `for-each-ref --contains` reports a false match with exit 0 when a promisor object is
    missing; this walk fails instead.
    """
    managed = MANAGED_BRANCH_PREFIX.removeprefix("refs/heads/")
    out = run_git(
        ["rev-list", "--count", oid, "--not", f"--exclude={managed}*", "--branches", "--remotes"],
        cwd=cwd,
    ).stdout
    try:
        return int(out.strip()) > 0
    except ValueError as exc:
        raise IsolationError("Could not tell whether the managed branch is merged") from exc


def branch_tip(main_root: Path, branch_ref: str) -> str:
    """The commit the managed branch points at, or "" once it is gone."""
    if not ref_exists(main_root, branch_ref):
        return ""
    return (
        run_git(
            ["rev-parse", "--verify", "--end-of-options", f"{branch_ref}^{{commit}}"],
            cwd=main_root,
        )
        .stdout.strip()
        .decode("ascii", errors="replace")
    )

"""Isolated directory acquisition and hand-back for one private task (ADR-0085)."""

from __future__ import annotations

import logging
from dataclasses import dataclass, replace
from pathlib import Path

from agentnave.isolated import (
    WORKTREES_DIR_NAME,
    ClaimedIsolation,
    CreatedIsolation,
    IsolatedDirs,
    IsolationContext,
    IsolationLease,
    isolation_instructions,
)
from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.models import InvocationRequest


@dataclass
class IsolationHold:
    """The isolated directory a task holds from before launch until its run settles."""

    request: InvocationRequest
    created: CreatedIsolation | None = None
    claimed: ClaimedIsolation | None = None
    info: TaskIsolationInfo | None = None
    warnings: tuple[str, ...] = ()
    consumed: bool = False  # the run owns the lease from here on

    @property
    def lease(self) -> IsolationLease | None:
        if self.created is not None:
            return self.created.lease
        return self.claimed.lease if self.claimed is not None else None


def may_touch_isolated_dirs(cwd: Path, *, requested: bool) -> bool:
    """False when acquiring can be skipped without reading any registry or repository."""
    if requested:
        return True
    return any(part.lower() == WORKTREES_DIR_NAME for part in cwd.resolve().parts)


def acquire_isolation(
    isolated: IsolatedDirs,
    request: InvocationRequest,
    workspace_id: str | None,
    *,
    requested: bool,
    ref: str | None,
) -> IsolationHold:
    """Create a new isolated directory or lease the one cwd already lies in (blocking)."""
    if requested:
        assert workspace_id is not None
        created = isolated.create(request.cwd, workspace_id=workspace_id, ref=ref)
        context = IsolationContext(
            id=created.id,
            worktree_path=created.worktree_path,
            branch=created.branch,
            main_root=created.main_root,
        )
        info = TaskIsolationInfo(
            id=created.id,
            path=str(created.worktree_path),
            cwd=str(created.execution_cwd),
            branch=created.branch,
            repo_common_dir=created.repo_common_dir,
            base_oid=created.base_oid,
            warnings=created.warnings,
        )
        return _hold(request, context, created.execution_cwd, info, created=created)
    notes: list[str] = []
    claimed = isolated.claim_for_cwd(request.cwd, notes)
    if claimed is None:
        return IsolationHold(request, warnings=tuple(notes))
    context = IsolationContext(
        id=claimed.id,
        worktree_path=claimed.worktree_path,
        branch=claimed.branch,
        main_root=claimed.main_root,
    )
    info = TaskIsolationInfo(
        id=claimed.id,
        path=str(claimed.worktree_path),
        cwd=str(claimed.execution_cwd),
        branch=claimed.branch,
        repo_common_dir=claimed.repo_common_dir,
        base_oid=claimed.base_oid,
    )
    hold = _hold(request, context, claimed.execution_cwd, info, claimed=claimed)
    hold.warnings = tuple(notes)
    return hold


def _hold(
    request: InvocationRequest,
    context: IsolationContext,
    cwd: Path,
    info: TaskIsolationInfo,
    *,
    created: CreatedIsolation | None = None,
    claimed: ClaimedIsolation | None = None,
) -> IsolationHold:
    text = isolation_instructions(context, cwd, read_only=request.read_only)
    info = replace(info, instructions_applied=text is not None)
    return IsolationHold(
        replace(request, cwd=cwd, extra_instructions=text),
        created=created,
        claimed=claimed,
        info=info,
    )


def release_hold(isolated: IsolatedDirs, hold: IsolationHold) -> None:
    """Give back a hold that never reached a running task; each step stands alone."""
    if hold.consumed:
        return
    log = logging.getLogger(__name__)
    if hold.created is not None:
        try:
            isolated.abort_created(hold.created)
        except Exception:
            log.exception("Rolling back isolated directory %s failed", hold.created.id)
    elif hold.claimed is not None:
        try:
            hold.claimed.lease.release()
        except Exception:
            log.exception("Releasing lease on isolated directory %s failed", hold.claimed.id)

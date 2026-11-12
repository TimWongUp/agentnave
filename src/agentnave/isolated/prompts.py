"""Service-appended prompt boundary instructions for isolated directories (ADR-0085)."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from agentnave.models import IsolationContext

ISOLATED_APPLIED = "isolated directory instructions"


@dataclass(frozen=True, slots=True)
class TaskIsolationInfo:
    """Isolated directory metadata attached to a started task."""

    id: str
    path: str
    cwd: str
    branch: str
    repo_common_dir: Path
    base_oid: str | None = None
    warnings: tuple[str, ...] = ()


def format_isolated_instructions(
    isolation: IsolationContext,
    cwd: Path,
    *,
    read_only: bool,
    read_only_instructions: str,
) -> str:
    """Build isolated directory instructions, merging cleanly with read-only rules when active."""
    header = (
        f"Isolated task environment: worktree {isolation.worktree_path}, working directory {cwd}, "
        f"managed branch {isolation.branch}, and main worktree {isolation.main_root}."
    )
    if read_only:
        return (
            f"{header} When the task refers to a path under the main worktree, use the "
            "corresponding path under this isolated worktree. Do not switch branches. "
            f"{read_only_instructions}"
        )
    return (
        f"{header} Do not modify the main worktree, push, or switch branches. When the task asks "
        "you to change a file under the main worktree, change the corresponding path under this "
        "isolated worktree instead."
    )

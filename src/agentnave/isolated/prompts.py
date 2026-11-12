"""Service-appended prompt boundary instructions for isolated directories (ADR-0085)."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from agentnave.isolated.repo import control_char_in

ISOLATED_APPLIED = "isolated directory instructions"


@dataclass(frozen=True, slots=True)
class IsolationContext:
    """Service-managed isolated worktree metadata used to word the task instructions."""

    id: str
    worktree_path: Path
    branch: str
    main_root: Path


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
    instructions_applied: bool = True


def format_isolated_instructions(isolation: IsolationContext, cwd: Path, *, read_only: bool) -> str:
    """Build isolated directory instructions; read-only rules are added separately."""
    header = (
        f"Isolated task environment: worktree {isolation.worktree_path}, working directory {cwd}, "
        f"managed branch {isolation.branch}, and main worktree {isolation.main_root}."
    )
    if read_only:
        return (
            f"{header} When the task refers to a path under the main worktree, use the "
            "corresponding path under this isolated worktree. Do not switch branches."
        )
    return (
        f"{header} Do not modify the main worktree, push, or switch branches. When the task asks "
        "you to change a file under the main worktree, change the corresponding path under this "
        "isolated worktree instead."
    )


def isolation_instructions(
    isolation: IsolationContext, cwd: Path, *, read_only: bool
) -> str | None:
    """The instructions, or None when a path holds control characters that could forge text."""
    paths = (isolation.worktree_path, cwd, isolation.main_root)
    if any(control_char_in(str(path)) for path in paths) or control_char_in(isolation.branch):
        return None
    return format_isolated_instructions(isolation, cwd, read_only=read_only)

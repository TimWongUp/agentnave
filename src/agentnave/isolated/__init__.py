"""Service-managed isolated Git worktrees, leases, release safety, and recovery (ADR-0085)."""

from agentnave.isolated.copy import (
    INCLUDE_FILENAME,
    MAX_INCLUDE_FILE_BYTES,
    MAX_INCLUDE_FILES,
    MAX_INCLUDE_TOTAL_BYTES,
)
from agentnave.isolated.leases import (
    IsolationLease,
    RepoLockUnavailable,
    other_active_leases,
    repo_lock,
)
from agentnave.isolated.manager import (
    ClaimedIsolation,
    CreatedIsolation,
    IsolatedDirs,
    IsolatedListItem,
)
from agentnave.isolated.prompts import (
    ISOLATED_APPLIED,
    IsolationContext,
    format_isolated_instructions,
    isolation_instructions,
)
from agentnave.isolated.release import (
    MAX_SUMMARY_FILES,
    ChangeSummary,
    RecoveryOutcome,
    RecoveryStatus,
    ReleaseResult,
    ReleaseStatus,
    SummaryStatus,
)
from agentnave.isolated.repo import (
    WORKTREES_DIR_NAME,
    IsolationError,
    isolated_worktree_root,
    shares_cwd_tree,
)
from agentnave.isolated.store import (
    ID_PATTERN,
    MANAGED_BRANCH_PREFIX,
    NAME,
    SCHEMA_VERSION,
    IsolatedRecord,
    IsolationState,
    WorktreeStore,
)

__all__ = [
    "ChangeSummary",
    "ClaimedIsolation",
    "CreatedIsolation",
    "ID_PATTERN",
    "INCLUDE_FILENAME",
    "ISOLATED_APPLIED",
    "IsolatedDirs",
    "IsolatedListItem",
    "IsolatedRecord",
    "IsolationError",
    "IsolationContext",
    "IsolationLease",
    "IsolationState",
    "MANAGED_BRANCH_PREFIX",
    "MAX_INCLUDE_FILES",
    "MAX_INCLUDE_FILE_BYTES",
    "MAX_INCLUDE_TOTAL_BYTES",
    "MAX_SUMMARY_FILES",
    "NAME",
    "RecoveryOutcome",
    "RepoLockUnavailable",
    "RecoveryStatus",
    "ReleaseResult",
    "ReleaseStatus",
    "SCHEMA_VERSION",
    "SummaryStatus",
    "WORKTREES_DIR_NAME",
    "WorktreeStore",
    "format_isolated_instructions",
    "isolation_instructions",
    "isolated_worktree_root",
    "other_active_leases",
    "repo_lock",
    "shares_cwd_tree",
]

"""Independent SQLite registry for service-managed isolated worktrees (ADR-0085)."""

from __future__ import annotations

import platform
import re
import sqlite3
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

NAME = "worktrees.db"
SCHEMA_VERSION = 1
ID_PATTERN = re.compile(r"^wt-[0-9a-f]{12}$")
MANAGED_BRANCH_PREFIX = "refs/heads/agentnave/"

type IsolationState = Literal["creating", "ready", "deleting", "needs_attention"]

_SCHEMA = """
CREATE TABLE IF NOT EXISTS isolated_dirs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    machine_id TEXT NOT NULL,
    repo_common_dir TEXT NOT NULL,
    repo_identity TEXT NOT NULL,
    main_root TEXT NOT NULL,
    worktree_path TEXT NOT NULL UNIQUE,
    worktree_git_dir TEXT,
    execution_cwd TEXT NOT NULL,
    branch_ref TEXT NOT NULL,
    base_oid TEXT NOT NULL,
    state TEXT NOT NULL,
    phase TEXT NOT NULL,
    usage_generation INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    attention TEXT,
    confirmation_digest TEXT,
    delete_oid TEXT
);
CREATE INDEX IF NOT EXISTS isolated_dirs_workspace
    ON isolated_dirs(workspace_id, state);
"""

_COLUMNS = (
    "id",
    "workspace_id",
    "machine_id",
    "repo_common_dir",
    "repo_identity",
    "main_root",
    "worktree_path",
    "worktree_git_dir",
    "execution_cwd",
    "branch_ref",
    "base_oid",
    "state",
    "phase",
    "usage_generation",
    "created_at",
    "updated_at",
    "attention",
    "confirmation_digest",
    "delete_oid",
)


def local_machine_id() -> str:
    """Diagnostic host identifier stored alongside isolated directory records."""
    return platform.node() or "local"


class StaleStateError(ValueError):
    """The registry row is no longer in the state the caller expected."""


@dataclass(frozen=True, slots=True)
class IsolatedRecord:
    """Persistent registry row in worktrees.db."""

    id: str
    workspace_id: str
    machine_id: str
    repo_common_dir: str
    repo_identity: str
    main_root: str
    worktree_path: str
    worktree_git_dir: str | None
    execution_cwd: str
    branch_ref: str
    base_oid: str
    state: IsolationState
    phase: str
    usage_generation: int
    created_at: str
    updated_at: str
    attention: str | None = None
    confirmation_digest: str | None = None
    delete_oid: str | None = None

    @property
    def branch(self) -> str:
        return self.branch_ref.removeprefix("refs/heads/")


def _row_to_record(row: tuple[object, ...]) -> IsolatedRecord:
    return IsolatedRecord(
        id=str(row[0]),
        workspace_id=str(row[1]),
        machine_id=str(row[2]),
        repo_common_dir=str(row[3]),
        repo_identity=str(row[4]),
        main_root=str(row[5]),
        worktree_path=str(row[6]),
        worktree_git_dir=None if row[7] is None else str(row[7]),
        execution_cwd=str(row[8]),
        branch_ref=str(row[9]),
        base_oid=str(row[10]),
        state=row[11],  # pyright: ignore[reportArgumentType]
        phase=str(row[12]),
        usage_generation=int(row[13]),  # pyright: ignore[reportArgumentType]
        created_at=str(row[14]),
        updated_at=str(row[15]),
        attention=None if row[16] is None else str(row[16]),
        confirmation_digest=None if row[17] is None else str(row[17]),
        delete_oid=None if row[18] is None else str(row[18]),
    )


class WorktreeStore:
    """SQLite registry at <data_root>/worktrees.db, separate from conversation history.

    Every operation uses its own connection, so the store is safe to call from worker threads.
    State changes are compare-and-set: they only apply while the row is still in the state the
    caller saw, and report whether they did.
    """

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.path = self.root / NAME

    def close(self) -> None:
        """Connections live only for one operation; nothing stays open."""

    def _connect(self, *, create: bool) -> sqlite3.Connection | None:
        if not create and not self.path.is_file():
            return None
        try:
            self.path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
            connection = sqlite3.connect(self.path, isolation_level=None, timeout=5)
            try:
                self.path.chmod(0o600)
                connection.execute("PRAGMA journal_mode = WAL")
                connection.execute("PRAGMA journal_size_limit = 8388608")
                connection.execute("PRAGMA synchronous = FULL")
                connection.execute("PRAGMA foreign_keys = ON")
                self._ensure_schema(connection)
            except BaseException:
                connection.close()
                raise
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        return connection

    def _ensure_schema(self, connection: sqlite3.Connection) -> None:
        version = int(connection.execute("PRAGMA user_version").fetchone()[0])
        if version == 0:
            connection.execute("BEGIN IMMEDIATE")
            try:
                if int(connection.execute("PRAGMA user_version").fetchone()[0]) == 0:
                    for statement in _SCHEMA.split(";"):
                        if statement.strip():
                            connection.execute(statement)
                    connection.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")
                connection.execute("COMMIT")
            except BaseException:
                if connection.in_transaction:
                    connection.rollback()
                raise
            version = int(connection.execute("PRAGMA user_version").fetchone()[0])
        if version != SCHEMA_VERSION:
            raise sqlite3.DatabaseError(
                f"{self.path} has schema version {version}; this AgentNave reads "
                f"version {SCHEMA_VERSION}"
            )

    @contextmanager
    def _write(self) -> Generator[sqlite3.Connection]:
        connection = self._connect(create=True)
        assert connection is not None
        try:
            connection.execute("BEGIN IMMEDIATE")
            try:
                yield connection
            except BaseException:
                if connection.in_transaction:
                    connection.rollback()
                raise
            connection.execute("COMMIT")
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        finally:
            connection.close()

    def _update(self, sql: str, params: tuple[object, ...]) -> bool:
        with self._write() as connection:
            return connection.execute(sql, params).rowcount > 0

    def _query(self, where: str = "", params: tuple[object, ...] = ()) -> list[IsolatedRecord]:
        connection = self._connect(create=False)
        if connection is None:
            return []
        clause = f" WHERE {where}" if where else ""
        sql = f"SELECT {', '.join(_COLUMNS)} FROM isolated_dirs{clause} ORDER BY created_at, id"
        try:
            rows = connection.execute(sql, params).fetchall()
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        finally:
            connection.close()
        return [_row_to_record(row) for row in rows]

    def get(self, isolation_id: str) -> IsolatedRecord | None:
        items = self._query("id = ?", (isolation_id,))
        return items[0] if items else None

    def all_records(self) -> list[IsolatedRecord]:
        return self._query()

    def list_workspace(self, workspace_id: str) -> list[IsolatedRecord]:
        return self._query("workspace_id = ? AND state != 'creating'", (workspace_id,))

    def list_repo(self, repo_identity: str) -> list[IsolatedRecord]:
        return self._query("repo_identity = ? AND state != 'creating'", (repo_identity,))

    def count_unreleased(self, workspace_id: str) -> int:
        return len(self.list_workspace(workspace_id))

    def pending_recovery(self) -> list[IsolatedRecord]:
        return self._query("state IN ('creating', 'deleting')")

    def insert_creating(self, record: IsolatedRecord) -> None:
        placeholders = ", ".join("?" * len(_COLUMNS))
        values = (
            record.id,
            record.workspace_id,
            record.machine_id,
            record.repo_common_dir,
            record.repo_identity,
            record.main_root,
            record.worktree_path,
            record.worktree_git_dir,
            record.execution_cwd,
            record.branch_ref,
            record.base_oid,
            record.state,
            record.phase,
            record.usage_generation,
            record.created_at,
            record.updated_at,
            record.attention,
            record.confirmation_digest,
            record.delete_oid,
        )
        with self._write() as connection:
            connection.execute(
                f"INSERT INTO isolated_dirs ({', '.join(_COLUMNS)}) VALUES ({placeholders})",
                values,
            )

    def mark_worktree_added(
        self, isolation_id: str, worktree_git_dir: str, phase: str, updated_at: str
    ) -> bool:
        return self._update(
            "UPDATE isolated_dirs SET worktree_git_dir = ?, phase = ?, updated_at = ? "
            "WHERE id = ? AND state = 'creating'",
            (worktree_git_dir, phase, updated_at, isolation_id),
        )

    def mark_ready(
        self, isolation_id: str, *, usage_generation: int, updated_at: str
    ) -> IsolatedRecord:
        changed = self._update(
            "UPDATE isolated_dirs SET state = 'ready', phase = 'ready', "
            "usage_generation = ?, attention = NULL, confirmation_digest = NULL, "
            "updated_at = ? WHERE id = ? AND state = 'creating'",
            (usage_generation, updated_at, isolation_id),
        )
        record = self.get(isolation_id)
        if not changed or record is None:
            raise StaleStateError(f"Isolated directory {isolation_id} left the creating state")
        return record

    def bump_usage(self, isolation_id: str, *, updated_at: str) -> IsolatedRecord:
        changed = self._update(
            "UPDATE isolated_dirs SET usage_generation = usage_generation + 1, "
            "confirmation_digest = NULL, updated_at = ? WHERE id = ? AND state = 'ready'",
            (updated_at, isolation_id),
        )
        record = self.get(isolation_id)
        if not changed or record is None:
            raise StaleStateError(f"Isolated directory {isolation_id} is no longer ready")
        return record

    def save_confirmation(self, isolation_id: str, digest: str, *, updated_at: str) -> bool:
        return self._update(
            "UPDATE isolated_dirs SET confirmation_digest = ?, updated_at = ? "
            "WHERE id = ? AND state IN ('ready', 'needs_attention', 'deleting')",
            (digest, updated_at, isolation_id),
        )

    def mark_deleting(
        self,
        isolation_id: str,
        *,
        phase: str,
        delete_oid: str,
        updated_at: str,
        expected_state: IsolationState = "ready",
    ) -> bool:
        return self._update(
            "UPDATE isolated_dirs SET state = 'deleting', phase = ?, delete_oid = ?, "
            "attention = NULL, updated_at = ? WHERE id = ? AND state = ?",
            (phase, delete_oid, updated_at, isolation_id, expected_state),
        )

    def revert_deleting_to_ready(self, isolation_id: str, *, updated_at: str) -> bool:
        """Hand a deletion that stopped before touching the worktree back as ready."""
        return self._update(
            "UPDATE isolated_dirs SET state = 'ready', phase = 'ready', delete_oid = NULL, "
            "attention = NULL, confirmation_digest = NULL, updated_at = ? "
            "WHERE id = ? AND state = 'deleting'",
            (updated_at, isolation_id),
        )

    def revert_to_creating(self, isolation_id: str, *, updated_at: str) -> bool:
        """Hand a ready record back to recovery as an unfinished creation."""
        return self._update(
            "UPDATE isolated_dirs SET state = 'creating', phase = 'abort_pending', "
            "confirmation_digest = NULL, updated_at = ? WHERE id = ? AND state = 'ready'",
            (updated_at, isolation_id),
        )

    def resume_creating(self, isolation_id: str, *, phase: str, updated_at: str) -> bool:
        """Put a needs_attention row back into creating so its rollback can be retried."""
        return self._update(
            "UPDATE isolated_dirs SET state = 'creating', phase = ?, attention = NULL, "
            "updated_at = ? WHERE id = ? AND state = 'needs_attention'",
            (phase, updated_at, isolation_id),
        )

    def update_phase(
        self,
        isolation_id: str,
        *,
        phase: str,
        updated_at: str,
        expected_state: IsolationState = "deleting",
    ) -> bool:
        return self._update(
            "UPDATE isolated_dirs SET phase = ?, updated_at = ? WHERE id = ? AND state = ?",
            (phase, updated_at, isolation_id, expected_state),
        )

    def mark_attention(
        self,
        isolation_id: str,
        *,
        expected_state: IsolationState,
        phase: str,
        reason: str,
        updated_at: str,
    ) -> IsolatedRecord | None:
        self._update(
            "UPDATE isolated_dirs SET state = 'needs_attention', phase = ?, attention = ?, "
            "confirmation_digest = NULL, updated_at = ? WHERE id = ? AND state = ?",
            (phase, reason, updated_at, isolation_id, expected_state),
        )
        return self.get(isolation_id)

    def delete(self, isolation_id: str, *, expected_state: IsolationState | None = None) -> bool:
        if expected_state is None:
            return self._update("DELETE FROM isolated_dirs WHERE id = ?", (isolation_id,))
        return self._update(
            "DELETE FROM isolated_dirs WHERE id = ? AND state = ?",
            (isolation_id, expected_state),
        )

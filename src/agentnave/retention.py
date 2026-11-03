"""Age-based workspace cleanup, serialized with App and server conversation writers."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
import sqlite3
import stat
import threading
import time
from collections.abc import Callable
from contextlib import ExitStack, closing
from datetime import UTC, datetime
from pathlib import Path
from typing import cast

from agentnave.db import NAME, SCHEMA_VERSION, add_handover_schema
from agentnave.locks import lock_file, release_and_remove
from agentnave.members import read_history_retention_days

_LOG = logging.getLogger(__name__)
_CANDIDATE = Path(__file__).with_name("retention-candidate.sql").read_text(encoding="utf-8")
_INTERVAL = 24 * 60 * 60
GLOBAL_LOCK = "history-retention.lock"
_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")


def _lock_path(root: Path, id_: str) -> Path | None:
    if not _ID.fullmatch(id_):
        return None
    path = root / "locks" / f"{id_}.lock"
    try:
        if not stat.S_ISREG(path.lstat().st_mode):
            return None
    except FileNotFoundError:
        pass
    return path


def _exists(connection: sqlite3.Connection, id_: str) -> bool:
    # The cleanup connection is in autocommit mode: every call gets a new snapshot.
    return (
        # NOCASE: a file name differing only in case is the same file on APFS and NTFS.
        connection.execute(
            "SELECT 1 FROM conversations WHERE id = ? COLLATE NOCASE", (id_,)
        ).fetchone()
        is not None
    )


def _make_removable(path: Path) -> None:
    """Let a seat tree be deleted: CLIs may leave read-only or unlistable directories, and on
    Windows read-only files. Symlinks are never followed or changed."""
    owner = stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR
    os.chmod(path, owner)
    for top, directories, files in os.walk(path):
        for name in directories:
            child = os.path.join(top, name)
            if not os.path.islink(child):
                os.chmod(child, owner)
        if os.name == "nt":
            for name in files:
                child = os.path.join(top, name)
                if not os.path.islink(child):
                    os.chmod(child, stat.S_IREAD | stat.S_IWRITE)


def _contained(directory: Path) -> bool:
    """A symlinked seats or locks directory leads outside the data root: never sweep it."""
    return not directory.is_symlink()


def _remove_seat(root: Path, id_: str) -> bool:
    path = root / "seats" / id_
    if not _contained(path.parent):
        return False
    try:
        if path.is_symlink():
            path.unlink()
        elif path.exists():
            _make_removable(path)
            shutil.rmtree(path)
        return True
    except Exception:
        _LOG.exception("History retention could not remove seat directory %s", path)
        return False


def _sweep_locks(
    connection: sqlite3.Connection, root: Path, now: float, stopped: Callable[[], bool]
) -> None:
    if not _contained(root / "locks"):
        return
    for path in (root / "locks").glob("*.lock"):
        if stopped():
            break
        try:
            id_ = path.stem
            if _lock_path(root, id_) is None or _exists(connection, id_):
                continue
            if now - path.lstat().st_mtime < 3600:
                continue
            with lock_file(path, "Conversation is in use", create=False) as guard:
                if not _exists(connection, id_):
                    release_and_remove(guard, path)
        except (FileNotFoundError, ValueError):
            continue
        except Exception:
            _LOG.exception("History retention skipped orphan lock %s", path)


def _sweep_seats(
    connection: sqlite3.Connection, root: Path, now: float, stopped: Callable[[], bool]
) -> None:
    if not _contained(root / "seats") or not _contained(root / "locks"):
        return
    for seat in (root / "seats").glob("*"):
        if stopped():
            break
        try:
            id_ = seat.name
            path = _lock_path(root, id_)
            if path is None or _exists(connection, id_):
                continue
            info = seat.lstat()
            if (
                not (stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode))
                or now - info.st_mtime < _INTERVAL
            ):
                continue
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with lock_file(path, "Conversation is in use") as guard:
                if not _exists(connection, id_) and _remove_seat(root, id_):
                    release_and_remove(guard, path)
        except (FileNotFoundError, ValueError):
            continue
        except Exception:
            _LOG.exception("History retention skipped orphan seat %s", seat)


def _expire_handovers(connection: sqlite3.Connection, cutoff: int) -> None:
    connection.execute("BEGIN IMMEDIATE")
    try:
        # Read and delete under the same write transaction, excluding a concurrent claim.
        for id_, released in connection.execute(
            "SELECT id, released FROM workspaces WHERE pending = 1 AND NOT EXISTS "
            "(SELECT 1 FROM conversations WHERE manager_session = workspaces.id)"
        ).fetchall():
            timestamp = _timestamp(released)
            if timestamp is not None and timestamp < cutoff:
                connection.execute("DELETE FROM workspaces WHERE id = ?", (id_,))
        connection.execute(
            "DELETE FROM former_bosses WHERE workspace NOT IN (SELECT id FROM workspaces)"
        )
        connection.commit()
    finally:
        if connection.in_transaction:
            connection.rollback()


def _workspace_ids(connection: sqlite3.Connection, anchor: str) -> list[str]:
    row = connection.execute(
        "SELECT manager_session FROM conversations WHERE id = ?", (anchor,)
    ).fetchone()
    if row is None:
        return []
    session = row[0]
    if not isinstance(session, str) or not re.fullmatch(r"s-[0-9a-fA-F]{32}", session):
        return [anchor]
    return [
        row[0]
        for row in connection.execute(
            "SELECT id FROM conversations WHERE manager_session = ? ORDER BY id", (session,)
        )
    ]


def _timestamp(value: object) -> int | None:
    if not isinstance(value, str) or not re.fullmatch(
        r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
        r"(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})",
        value,
    ):
        return None
    try:
        parsed = datetime.fromisoformat(value)
        if parsed.tzinfo is None:
            return None
        delta = parsed - datetime(1970, 1, 1, tzinfo=UTC)
        return (delta.days * 86400 + delta.seconds) * 1000 + delta.microseconds // 1000
    except (ValueError, OverflowError, OSError):
        return None


def _eligible(connection: sqlite3.Connection, ids: list[str], cutoff: int) -> bool:
    if connection.execute(
        "SELECT 1 FROM workspaces JOIN conversations ON manager_session = workspaces.id "
        "WHERE conversations.id = ? AND pending = 1",
        (ids[0],),
    ).fetchone():
        return False
    for id_ in ids:
        row = connection.execute(_CANDIDATE, (id_,)).fetchone()
        if row is None or row[1] is None or row[2]:
            return False
        try:
            view = cast(dict[str, object], json.loads(row[1]))
            summary = cast(dict[str, object], view["summary"])
            updated = _timestamp(summary["updated"])
        except (ValueError, TypeError, KeyError):
            _LOG.exception("History retention skipped conversation %s: unreadable view", id_)
            return False
        created = _timestamp(row[0])
        if created is None or updated is None or max(created, updated) >= cutoff:
            return False
    return True


def _remove(connection: sqlite3.Connection, root: Path, ids: list[str], cutoff: int) -> bool:
    # OS locks precede the SQLite write transaction, so active writers can finish.
    with ExitStack() as locks:
        for id_ in ids:
            path = _lock_path(root, id_)
            if path is None:
                return False
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            try:
                locks.enter_context(lock_file(path, "Conversation is in use"))
            except ValueError:
                return False
        connection.execute("BEGIN IMMEDIATE")
        try:
            if _workspace_ids(connection, ids[0]) != ids or not _eligible(connection, ids, cutoff):
                return False
            group = connection.execute(
                "SELECT manager_session FROM conversations WHERE id = ?", (ids[0],)
            ).fetchone()[0]
            connection.executemany(
                "DELETE FROM conversations WHERE id = ?", [(id_,) for id_ in ids]
            )
            connection.execute(
                "DELETE FROM workspaces WHERE pending != 1 AND NOT EXISTS "
                "(SELECT 1 FROM conversations WHERE manager_session = workspaces.id) "
                "AND (id IS ? OR title IS NULL "
                "OR NOT (pending = 1 OR owner != id OR previous_host IS NOT NULL))",
                (group,),
            )
            connection.execute(
                "DELETE FROM former_bosses WHERE workspace NOT IN (SELECT id FROM workspaces)"
            )
            connection.commit()
            for id_ in ids:
                _remove_seat(root, id_)
            return True
        finally:
            if connection.in_transaction:
                connection.rollback()


def cleanup(
    root: Path, *, now: float | None = None, stopped: Callable[[], bool] = lambda: False
) -> int:
    """Delete expired workspaces; lock contention skips this pass, not other services.

    Each workspace is atomic. Orphan resources expire even when history is kept forever.
    """
    days = read_history_retention_days(root)
    if not (root / NAME).is_file():
        return 0
    try:
        guard = lock_file(root / GLOBAL_LOCK, "Another service is cleaning history")
    except ValueError:
        return 0
    with (
        guard,
        closing(
            sqlite3.connect(
                (root / NAME).resolve().as_uri() + "?mode=rw", uri=True, isolation_level=None
            )
        ) as connection,
    ):
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute("PRAGMA synchronous = FULL")
        connection.execute("PRAGMA journal_size_limit = 8388608")
        if connection.execute("PRAGMA user_version").fetchone()[0] != SCHEMA_VERSION:
            return 0
        add_handover_schema(connection)
        current = time.time() if now is None else now
        _sweep_locks(connection, root, current, stopped)
        _sweep_seats(connection, root, current, stopped)
        if stopped():
            return 0
        _expire_handovers(connection, int((current - 90 * _INTERVAL) * 1000))
        if days is None:
            return 0
        cutoff = int(current * 1000) - days * _INTERVAL * 1000
        seen: set[str] = set()
        removed = 0
        for (anchor,) in connection.execute("SELECT id FROM conversations ORDER BY id").fetchall():
            if stopped():
                break
            if anchor in seen:
                continue
            try:
                ids = _workspace_ids(connection, anchor)
                seen.update(ids)
                if (
                    ids
                    and _eligible(connection, ids, cutoff)
                    and _remove(connection, root, ids, cutoff)
                ):
                    removed += 1
            except Exception:
                # A single damaged workspace must not prevent healthy workspaces expiring.
                # _remove rolls back its transaction and releases its locks before this log.
                _LOG.exception("History retention skipped workspace anchored by %s", anchor)
        return removed


class Retention:
    """One joined background thread; never uses the event loop's database connection."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self._stop = threading.Event()
        self._busy = threading.Event()
        self._thread = threading.Thread(target=self._run, name="history-retention")
        self._thread.start()

    def _run(self) -> None:
        while not self._stop.is_set():
            self._busy.set()
            try:
                cleanup(self.root, stopped=self._stop.is_set)
            except Exception:
                _LOG.exception("History retention pass failed; history preserved for retry")
            finally:
                self._busy.clear()
            if self._stop.wait(_INTERVAL):
                break

    async def close(self) -> None:
        self._stop.set()
        # Do not cancel an in-flight workspace transaction or let relay replacement kill it.
        await asyncio.to_thread(self._thread.join)

    def restart_safe(self) -> bool:
        return not self._busy.is_set()

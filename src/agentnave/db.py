"""The conversation database (ADR-0026): one SQLite file, written in small transactions.

Writers hold the conversation's OS lock (store.Record); this module only maps a Conversation
to rows and back. sqlite3 errors surface as OSError, like the file writes they replaced.
"""

from __future__ import annotations

import json
import sqlite3
import time
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import NamedTuple

from agentnave.store import Conversation, EntryRecord, RoundRecord, TaskRecord

SCHEMA_VERSION = 1
NAME = "agentnave.db"

# Shared with the tray app's tests, which build the same database.
_SCHEMA = Path(__file__).with_name("schema.sql").read_text(encoding="utf-8")

_HEADER = (
    "kind",
    "title",
    "created",
    "host",
    "host_source",
    "manager_session",
    "cwd",
    "review",
    "isolated",
    "seats",
    "sessions",
)


def _header(state: Conversation) -> tuple[object, ...]:
    return (
        state.kind,
        state.title,
        state.created,
        state.host,
        state.host_source,
        state.manager_session,
        state.cwd,
        int(state.review),
        int(state.isolated),
        json.dumps([seat.model_dump() for seat in state.seats], ensure_ascii=False),
        json.dumps(
            {key: value.model_dump() for key, value in state.sessions.items()},
            ensure_ascii=False,
        ),
    )


def _same_header(old: Conversation, new: Conversation) -> bool:
    return old.model_dump(exclude={"entries", "rounds", "tasks"}) == new.model_dump(
        exclude={"entries", "rounds", "tasks"}
    )


class Database:
    """One connection per server process; the file is created by the first write."""

    def __init__(self, root: Path) -> None:
        self.path = root / NAME
        self._connection: sqlite3.Connection | None = None

    def close(self) -> None:
        if self._connection is not None:
            self._connection.close()
            self._connection = None

    def _connect(self, *, create: bool) -> sqlite3.Connection | None:
        if self._connection is not None:
            return self._connection
        if not create and not self.path.is_file():
            return None
        try:
            self.path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
            connection = sqlite3.connect(self.path, isolation_level=None, timeout=5)
            try:
                self.path.chmod(0o600)
                connection.execute("PRAGMA journal_mode = WAL")
                connection.execute("PRAGMA synchronous = FULL")
                connection.execute("PRAGMA foreign_keys = ON")
                version = connection.execute("PRAGMA user_version").fetchone()[0]
                if version == 0:
                    connection.execute("BEGIN IMMEDIATE")
                    try:
                        # Another server may have created it while this one waited.
                        if connection.execute("PRAGMA user_version").fetchone()[0] == 0:
                            for statement in _SCHEMA.split(";"):
                                if statement.strip():
                                    connection.execute(statement)
                            connection.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")
                    except BaseException:
                        connection.rollback()
                        raise
                    connection.execute("COMMIT")
                    version = connection.execute("PRAGMA user_version").fetchone()[0]
                if version != SCHEMA_VERSION:
                    raise sqlite3.DatabaseError(
                        f"{self.path} has schema version {version}; this AgentNave reads "
                        f"version {SCHEMA_VERSION}"
                    )
                if not connection.execute(
                    "SELECT 1 FROM sqlite_master WHERE name = 'former_bosses'"
                ).fetchone():
                    # Created by an older version: add the tables it lacks (all idempotent).
                    connection.execute("BEGIN IMMEDIATE")
                    try:
                        for statement in _SCHEMA.split(";"):
                            if statement.strip():
                                connection.execute(statement)
                    except BaseException:
                        connection.rollback()
                        raise
                    connection.execute("COMMIT")
                _add_workspace_columns(connection)
            except BaseException:
                connection.close()
                raise
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        self._connection = connection
        return connection

    @contextmanager
    def _write(self) -> Generator[sqlite3.Connection]:
        connection = self._connect(create=True)
        assert connection is not None
        try:
            connection.execute("BEGIN IMMEDIATE")
            try:
                yield connection
            except BaseException:
                connection.rollback()
                raise
            connection.execute("COMMIT")
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc

    def _read(self) -> sqlite3.Connection | None:
        return self._connect(create=False)

    # Reads

    def revisions(self) -> dict[str, int]:
        """Every saved conversation id with its revision."""
        connection = self._read()
        if connection is None:
            return {}
        try:
            return dict(connection.execute("SELECT id, rev FROM conversations").fetchall())
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc

    def revision(self, conversation_id: str) -> int | None:
        connection = self._read()
        if connection is None:
            return None
        try:
            row = connection.execute(
                "SELECT rev FROM conversations WHERE id = ?", (conversation_id,)
            ).fetchone()
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        return None if row is None else int(row[0])

    def load(self, conversation_id: str) -> tuple[Conversation, int]:
        """The conversation and its revision; FileNotFoundError once it was deleted."""
        connection = self._read()
        if connection is None:
            raise FileNotFoundError(conversation_id)
        try:
            # One read transaction, so the rows belong to one revision.
            connection.execute("BEGIN")
            try:
                row = connection.execute(
                    f"SELECT {', '.join(_HEADER)}, rev FROM conversations WHERE id = ?",
                    (conversation_id,),
                ).fetchone()
                if row is None:
                    raise FileNotFoundError(conversation_id)
                entries = connection.execute(
                    "SELECT body FROM entries WHERE conversation_id = ? ORDER BY position",
                    (conversation_id,),
                ).fetchall()
                rounds = connection.execute(
                    "SELECT body FROM rounds WHERE conversation_id = ? ORDER BY number",
                    (conversation_id,),
                ).fetchall()
                tasks = connection.execute(
                    "SELECT body FROM tasks WHERE conversation_id = ? ORDER BY position",
                    (conversation_id,),
                ).fetchall()
            finally:
                connection.execute("COMMIT")
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        header = dict(zip(_HEADER, row[:-1], strict=True))
        state = Conversation.model_validate(
            {
                **header,
                "id": conversation_id,
                "review": bool(header["review"]),
                "isolated": bool(header["isolated"]),
                "seats": json.loads(str(header["seats"])),
                "sessions": json.loads(str(header["sessions"])),
                "entries": [json.loads(body) for (body,) in entries],
                "rounds": [json.loads(body) for (body,) in rounds],
                "tasks": [json.loads(body) for (body,) in tasks],
            }
        )
        return state, int(row[-1])

    # Workspaces (ADR-0033)

    def resolve(self, boss: str) -> tuple[str, Workspace | None]:
        """The workspace this boss writes now, and the row it was handed away in, if so."""
        connection = self._read()
        if connection is None:
            return boss, None
        try:
            return _resolve(connection, boss)
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc

    def workspace(self, workspace_id: str) -> Workspace | None:
        connection = self._read()
        if connection is None:
            return None
        try:
            return _workspace(connection, "id", workspace_id)
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc

    def waiting(self) -> list[Workspace]:
        """Workspaces marked for handover, oldest release first."""
        connection = self._read()
        if connection is None:
            return []
        try:
            rows = connection.execute(
                f"SELECT {', '.join(Workspace._fields)} FROM workspaces WHERE pending = 1 "
                "ORDER BY released, id"
            ).fetchall()
        except sqlite3.Error as exc:
            raise OSError(str(exc)) from exc
        return [_row(row) for row in rows]

    def register(self, workspace_id: str, boss: str, host: str | None, file: str | None) -> None:
        """Record the boss that created a conversation, and its transcript once it is known."""
        with self._write() as connection:
            connection.execute(
                "INSERT INTO workspaces (id, owner, owner_host, owner_file) VALUES (?, ?, ?, ?) "
                "ON CONFLICT (id) DO UPDATE SET owner_host = excluded.owner_host, "
                "owner_file = coalesce(excluded.owner_file, owner_file) "
                "WHERE owner = excluded.owner AND (owner_host IS NOT excluded.owner_host OR "
                "coalesce(excluded.owner_file, owner_file) IS NOT owner_file)",
                (workspace_id, boss, host, file),
            )

    def release(
        self,
        workspace_id: str,
        boss: str,
        host: str | None,
        file: str | None,
        note: str | None,
        released: str,
        title: str | None = None,
        cwd: str | None = None,
    ) -> None:
        """Mark the boss's own workspace as waiting for another boss."""
        with self._write() as connection:
            cursor = connection.execute(
                "INSERT INTO workspaces (id, owner, owner_host, owner_file, pending, note, "
                "released, title, cwd) VALUES (?, ?, ?, ?, 1, ?, ?, ?, ?) "
                "ON CONFLICT (id) DO UPDATE SET "
                "pending = 1, note = excluded.note, released = excluded.released, "
                "owner_host = excluded.owner_host, "
                "owner_file = coalesce(excluded.owner_file, owner_file), "
                "title = coalesce(excluded.title, title), cwd = coalesce(excluded.cwd, cwd) "
                "WHERE owner = excluded.owner",
                (workspace_id, boss, host, file, note, released, title, cwd),
            )
            if cursor.rowcount != 1:
                raise ValueError("this workspace was just taken over by another boss")

    def claim(
        self, workspace_id: str, boss: str, host: str | None, file: str | None
    ) -> tuple[Workspace, Workspace]:
        """Take a waiting workspace over (or withdraw one's own); the row before and after."""
        with self._write() as connection:
            before = _workspace(connection, "id", workspace_id)
            if before is None or (not before.pending and before.owner != boss):
                raise ValueError(f"workspace {workspace_id} is not waiting for a new boss")
            if before.owner != boss:
                current, away = _resolve(connection, boss)
                own = _workspace(connection, "id", current)
                if away is None and (
                    (own is not None and own.title and _handed_over(own))
                    or connection.execute(
                        "SELECT 1 FROM conversations WHERE manager_session = ? LIMIT 1",
                        (current,),
                    ).fetchone()
                ):
                    raise ValueError(
                        f"this chat already leads workspace {current}; take over from a new "
                        "chat, or hand that workspace over first"
                    )
                # An empty workspace of this boss gives way to the one it takes over.
                connection.execute("DELETE FROM workspaces WHERE owner = ?", (boss,))
                connection.execute("DELETE FROM former_bosses WHERE boss = ?", (boss,))
                connection.execute(
                    "UPDATE workspaces SET previous_host = owner_host, "
                    "previous_file = owner_file, owner = ?, owner_host = ?, owner_file = ?, "
                    "pending = 0 WHERE id = ?",
                    (boss, host, file, workspace_id),
                )
                if before.owner != workspace_id:
                    connection.execute(
                        "INSERT OR REPLACE INTO former_bosses (boss, workspace) VALUES (?, ?)",
                        (before.owner, workspace_id),
                    )
            else:
                # Only a withdrawal clears the note: asking again keeps what the boss took over.
                connection.execute(
                    "UPDATE workspaces SET note = CASE pending WHEN 1 THEN NULL ELSE note END, "
                    "released = CASE pending WHEN 1 THEN NULL ELSE released END, pending = 0, "
                    "owner_host = ?, owner_file = coalesce(?, owner_file) WHERE id = ?",
                    (host, file, workspace_id),
                )
            after = _workspace(connection, "id", workspace_id)
            assert after is not None
            return before, after

    # Writes

    def save(self, old: Conversation | None, new: Conversation, view: bytes | None) -> int:
        """Write what changed since old (None: a new conversation); return the new revision."""
        with self._write() as connection:
            if old is None:
                connection.execute(
                    f"INSERT INTO conversations (id, {', '.join(_HEADER)}, rev) "
                    f"VALUES (?, {', '.join('?' * len(_HEADER))}, 1)",
                    (new.id, *_header(new)),
                )
            else:
                if not _same_header(old, new):
                    connection.execute(
                        f"UPDATE conversations SET {', '.join(f'{c} = ?' for c in _HEADER)} "
                        "WHERE id = ?",
                        (*_header(new), new.id),
                    )
                connection.execute("UPDATE conversations SET rev = rev + 1 WHERE id = ?", (new.id,))
            row = connection.execute(
                "SELECT rev FROM conversations WHERE id = ?", (new.id,)
            ).fetchone()
            if row is None:
                raise FileNotFoundError(new.id)  # deleted while held: never rebuilt
            _save_entries(connection, new.id, old.entries if old else [], new.entries)
            _save_rounds(connection, new.id, old.rounds if old else [], new.rounds)
            _save_tasks(connection, new.id, old.tasks if old else [], new.tasks)
            if view is not None:
                _save_view(connection, new.id, view)
            return int(row[0])

    def save_view(self, conversation_id: str, view: bytes) -> None:
        """Replace the derived view when it differs; the conversation must still exist."""
        with self._write() as connection:
            _save_view(connection, conversation_id, view)


class Workspace(NamedTuple):
    """One workspaces row: who writes the workspace now, and the handover around it."""

    id: str
    owner: str
    owner_host: str | None
    owner_file: str | None
    pending: bool
    note: str | None
    released: str | None
    previous_host: str | None
    previous_file: str | None
    title: str | None
    cwd: str | None


def _add_workspace_columns(connection: sqlite3.Connection) -> None:
    """A database an older server created lacks them; older servers ignore them."""
    columns = {row[1] for row in connection.execute("PRAGMA table_info(workspaces)")}
    if {"title", "cwd"} <= columns:
        return
    connection.execute("BEGIN IMMEDIATE")
    try:
        columns = {row[1] for row in connection.execute("PRAGMA table_info(workspaces)")}
        for column in {"title", "cwd"} - columns:
            connection.execute(f"ALTER TABLE workspaces ADD COLUMN {column} TEXT")
    except BaseException:
        connection.rollback()
        raise
    connection.execute("COMMIT")


def _row(row: tuple[object, ...]) -> Workspace:
    item = Workspace(*row)  # pyright: ignore[reportArgumentType]
    return item._replace(pending=bool(item.pending))


def _workspace(connection: sqlite3.Connection, column: str, value: str) -> Workspace | None:
    row = connection.execute(
        f"SELECT {', '.join(Workspace._fields)} FROM workspaces WHERE {column} = ?", (value,)
    ).fetchone()
    return None if row is None else _row(row)


def _handed_over(row: Workspace) -> bool:
    """Whether the App shows this workspace's handover: the same rule as the tray's."""
    return bool(row.pending) or row.owner != row.id or row.previous_host is not None


def _resolve(connection: sqlite3.Connection, boss: str) -> tuple[str, Workspace | None]:
    owned = _workspace(connection, "owner", boss)
    if owned is not None:
        return owned.id, None
    former = connection.execute(
        "SELECT workspace FROM former_bosses WHERE boss = ?", (boss,)
    ).fetchone()
    if former is not None and (passed := _workspace(connection, "id", former[0])) is not None:
        return boss, passed
    own = _workspace(connection, "id", boss)
    return boss, own  # own is set only when this boss's workspace went to another boss


def _save_entries(
    connection: sqlite3.Connection,
    conversation_id: str,
    old: list[EntryRecord],
    new: list[EntryRecord],
) -> None:
    start = len(old)
    if new[:start] != old:
        # Entries only grow; anything else is rewritten whole rather than trusted.
        connection.execute("DELETE FROM entries WHERE conversation_id = ?", (conversation_id,))
        start = 0
    connection.executemany(
        "INSERT INTO entries (conversation_id, position, body) VALUES (?, ?, ?)",
        [
            (conversation_id, position, entry.model_dump_json())
            for position, entry in enumerate(new[start:], start)
        ],
    )


def _save_rounds(
    connection: sqlite3.Connection,
    conversation_id: str,
    old: list[RoundRecord],
    new: list[RoundRecord],
) -> None:
    before = {round_.id: round_ for round_ in old}
    for round_ in new:
        if before.get(round_.id) != round_:
            connection.execute(
                "INSERT INTO rounds (id, conversation_id, number, body) VALUES (?, ?, ?, ?) "
                "ON CONFLICT (id) DO UPDATE SET body = excluded.body",
                (round_.id, conversation_id, round_.number, round_.model_dump_json()),
            )
    gone = set(before) - {round_.id for round_ in new}
    connection.executemany("DELETE FROM rounds WHERE id = ?", [(item,) for item in gone])


def _save_tasks(
    connection: sqlite3.Connection,
    conversation_id: str,
    old: list[TaskRecord],
    new: list[TaskRecord],
) -> None:
    before = {task.id: task for task in old}
    for position, task in enumerate(new):
        if before.get(task.id) != task:
            connection.execute(
                "INSERT INTO tasks (id, conversation_id, position, provider, session_id, "
                "status, body) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET "
                "session_id = excluded.session_id, status = excluded.status, "
                "body = excluded.body",
                (
                    task.id,
                    conversation_id,
                    position,
                    task.provider,
                    task.session_id,
                    task.status,
                    task.model_dump_json(),
                ),
            )
    gone = set(before) - {task.id for task in new}
    connection.executemany("DELETE FROM tasks WHERE id = ?", [(item,) for item in gone])


def _save_view(connection: sqlite3.Connection, conversation_id: str, view: bytes) -> None:
    body = view.decode()
    written = time.time_ns() // 1_000_000
    connection.execute(
        "INSERT INTO views (conversation_id, rev, written_ms, body) VALUES (?, 1, ?, ?) "
        "ON CONFLICT (conversation_id) DO UPDATE SET rev = rev + 1, "
        "written_ms = excluded.written_ms, body = excluded.body WHERE body != excluded.body",
        (conversation_id, written, body),
    )

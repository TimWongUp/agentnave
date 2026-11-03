"""Age cleanup preserves whole workspaces unless every persisted member is safe."""

from __future__ import annotations

import asyncio
import json
import sqlite3
import threading
from datetime import UTC, datetime
from pathlib import Path

import pytest

from agentnave.db import Database
from agentnave.locks import lock_file
from agentnave.members import read_history_retention_days
from agentnave.retention import GLOBAL_LOCK, Retention, cleanup
from agentnave.store import Conversation

_NOW = datetime(2026, 11, 3, tzinfo=UTC).timestamp()
_OLD = "2020-01-01T00:00:00+00:00"
_WORKSPACE = "s-" + "a" * 32


def fixture(root: Path, id_: str, *, group: str | None = _WORKSPACE, updated: str = _OLD) -> None:
    db = Database(root)
    try:
        db.save(
            None,
            Conversation(id=id_, title=id_, kind="task", created=_OLD, manager_session=group),
            json.dumps({"summary": {"updated": updated, "status": "succeeded"}}).encode(),
        )
    finally:
        db.close()


def settings(root: Path, days: object = 30) -> None:
    (root / "settings.json").write_text(json.dumps({"version": 1, "history_retention_days": days}))


def remaining(root: Path) -> list[str]:
    with sqlite3.connect(root / "agentnave.db") as db:
        return [row[0] for row in db.execute("SELECT id FROM conversations ORDER BY id")]


@pytest.mark.parametrize("days", [None, 30, 90, 180])
def test_retention_setting_reads_only_explicit_valid_integers(
    tmp_path: Path, days: int | None
) -> None:
    assert read_history_retention_days(tmp_path) is None
    settings(tmp_path, days)
    assert read_history_retention_days(tmp_path) == days
    invalid: list[object] = [True, 30.0, "30", 31, -30, [], {}]
    for bad in invalid:
        settings(tmp_path, bad)
        assert read_history_retention_days(tmp_path) is None
    for bad_document in ["{", "[]", '{"version":true,"history_retention_days":30}']:
        (tmp_path / "settings.json").write_text(bad_document)
        assert read_history_retention_days(tmp_path) is None


def test_workspace_cleanup_cascades_and_forgets_handover_but_keeps_files(tmp_path: Path) -> None:
    settings(tmp_path)
    for id_ in ["old-a", "old-b"]:
        fixture(tmp_path, id_)
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        db.execute(
            "INSERT INTO workspaces (id, owner, pending, title) VALUES (?, ?, 1, 'keep')",
            (_WORKSPACE, _WORKSPACE),
        )
        db.execute(
            "INSERT INTO former_bosses (boss, workspace) VALUES ('former', ?)", (_WORKSPACE,)
        )
        db.execute("INSERT INTO entries VALUES ('old-a', 0, '{}')")
    seats = tmp_path / "seats/old-a/a"
    seats.mkdir(parents=True)
    (seats / "output.txt").write_text("keep")
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == []
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        for table in ["workspaces", "former_bosses", "entries", "views"]:
            assert db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] == 0
    assert (tmp_path / "locks/old-a.lock").exists()
    assert (seats / "output.txt").read_text() == "keep"


@pytest.mark.parametrize(
    "reason",
    [
        "recent",
        "created",
        "boundary",
        "running",
        "round",
        "pending",
        "held",
        "missing-view",
        "bad-time",
    ],
)
def test_one_unsafe_sibling_preserves_entire_workspace(tmp_path: Path, reason: str) -> None:
    settings(tmp_path)
    fixture(tmp_path, "old-a")
    fixture(tmp_path, "old-b")
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        if reason in {"recent", "boundary", "bad-time"}:
            timestamp = (
                "bad"
                if reason == "bad-time"
                else datetime.fromtimestamp(
                    _NOW - (30 * 86400 if reason == "boundary" else 0), UTC
                ).isoformat()
            )
            db.execute(
                "UPDATE views SET body = ? WHERE conversation_id = 'old-b'",
                (json.dumps({"summary": {"updated": timestamp}}),),
            )
        elif reason == "created":
            db.execute(
                "UPDATE conversations SET created = '2026-11-03T00:00:00Z' WHERE id = 'old-b'"
            )
        elif reason == "running":
            db.execute("INSERT INTO tasks VALUES ('t', 'old-b', 0, 'codex', NULL, 'running', '{}')")
        elif reason in {"round", "pending"}:
            body = {
                "status": "running" if reason == "round" else "finished",
                "seats": [{"review": "pending" if reason == "pending" else None}],
            }
            db.execute("INSERT INTO rounds VALUES ('r', 'old-b', 1, ?)", (json.dumps(body),))
        elif reason == "missing-view":
            db.execute("DELETE FROM views WHERE conversation_id = 'old-b'")
    (tmp_path / "locks").mkdir(exist_ok=True)
    guard = (
        lock_file(tmp_path / "locks/old-b.lock", "busy")
        if reason in {"held", "running", "round"}
        else None
    )
    try:
        assert cleanup(tmp_path, now=_NOW) == 0
        assert remaining(tmp_path) == ["old-a", "old-b"]
    finally:
        if guard is not None:
            guard.close()
    if reason in {"running", "round"}:
        # Once the writer crashed/released ownership, stale running rows are deletable.
        assert cleanup(tmp_path, now=_NOW) == 1
        assert remaining(tmp_path) == []


def test_no_setting_and_contended_global_lock_do_not_delete(tmp_path: Path) -> None:
    fixture(tmp_path, "old")
    assert cleanup(tmp_path, now=_NOW) == 0
    settings(tmp_path)
    with lock_file(tmp_path / GLOBAL_LOCK, "busy"):
        assert cleanup(tmp_path, now=_NOW) == 0
    assert remaining(tmp_path) == ["old"]
    assert cleanup(tmp_path, now=_NOW) == 1


def test_legacy_sessions_are_individual_workspaces(tmp_path: Path) -> None:
    settings(tmp_path)
    fixture(tmp_path, "old", group="legacy")
    fixture(tmp_path, "recent", group="legacy", updated="2026-11-03T00:00:00Z")
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == ["recent"]


@pytest.mark.asyncio
async def test_worker_does_not_block_loop_and_shutdown_joins_inflight_pass(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entered = threading.Event()
    release = threading.Event()
    calls: list[Path] = []

    def pass_(root: Path, **_kwargs: object) -> int:
        calls.append(root)
        entered.set()
        assert release.wait(5)
        return 0

    monkeypatch.setattr("agentnave.retention.cleanup", pass_)
    worker = Retention(tmp_path)
    try:
        assert await asyncio.to_thread(entered.wait, 2)
        assert not worker.restart_safe()
        closing = asyncio.create_task(worker.close())
        await asyncio.sleep(0)
        assert not closing.done()
        release.set()
        await closing
        assert worker.restart_safe()
        assert calls == [tmp_path]
    finally:
        release.set()
        await worker.close()


@pytest.mark.parametrize("change", ["activity", "sibling"])
def test_cleanup_rereads_after_taking_conversation_locks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    from typing import IO

    settings(tmp_path)
    fixture(tmp_path, "old")

    def take(path: Path, message: str) -> IO[bytes]:
        guard = lock_file(path, message)
        if path.name == "old.lock":
            if change == "sibling":
                fixture(tmp_path, "new-sibling")
            else:
                with sqlite3.connect(tmp_path / "agentnave.db") as db:
                    db.execute(
                        "UPDATE views SET body = ? WHERE conversation_id = 'old'",
                        (json.dumps({"summary": {"updated": "2026-11-03T00:00:00Z"}}),),
                    )
        return guard

    monkeypatch.setattr("agentnave.retention.lock_file", take)
    assert cleanup(tmp_path, now=_NOW) == 0
    assert "old" in remaining(tmp_path)


def test_bad_round_preserves_its_workspace_but_does_not_stop_later_cleanup(tmp_path: Path) -> None:
    settings(tmp_path)
    fixture(tmp_path, "broken-a")
    fixture(tmp_path, "broken-b")
    fixture(tmp_path, "z-good", group=None)
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        db.execute("INSERT INTO rounds VALUES ('r', 'broken-b', 1, '{broken')")
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == ["broken-a", "broken-b"]


@pytest.mark.parametrize("missing", ["tables", "columns"])
def test_first_cleanup_repairs_legacy_handover_schema(tmp_path: Path, missing: str) -> None:
    settings(tmp_path)
    fixture(tmp_path, "old")
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        if missing == "tables":
            db.execute("DROP TABLE workspaces")
            db.execute("DROP TABLE former_bosses")
        else:
            db.execute("ALTER TABLE workspaces DROP COLUMN title")
            db.execute("ALTER TABLE workspaces DROP COLUMN cwd")
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == []
    assert cleanup(tmp_path, now=_NOW) == 0
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        columns = {row[1] for row in db.execute("PRAGMA table_info(workspaces)")}
        assert {"title", "cwd"} <= columns
        assert db.execute("SELECT count(*) FROM former_bosses").fetchone()[0] == 0

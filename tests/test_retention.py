"""Age cleanup preserves whole workspaces unless every persisted member is safe."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import sqlite3
import stat
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


def test_workspace_cleanup_cascades_and_removes_seats_but_keeps_locks(tmp_path: Path) -> None:
    settings(tmp_path)
    for id_ in ["old-a", "old-b"]:
        fixture(tmp_path, id_)
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        db.execute(
            "INSERT INTO workspaces (id, owner, pending, title) VALUES (?, ?, 0, 'keep')",
            (_WORKSPACE, _WORKSPACE),
        )
        db.execute(
            "INSERT INTO former_bosses (boss, workspace) VALUES ('former', ?)", (_WORKSPACE,)
        )
        db.execute("INSERT INTO entries VALUES ('old-a', 0, '{}')")
    seats = tmp_path / "seats/old-a/a"
    seats.mkdir(parents=True)
    (seats / "output.txt").write_text("discard")
    (seats / "output.txt").chmod(0o400)
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == []
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        for table in ["workspaces", "former_bosses", "entries", "views"]:
            assert db.execute(f"SELECT count(*) FROM {table}").fetchone()[0] == 0
    assert (tmp_path / "locks/old-a.lock").exists()
    assert not seats.parent.exists()


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


def test_orphan_locks_expire_even_without_history_retention(tmp_path: Path) -> None:
    fixture(tmp_path, "saved")
    locks = tmp_path / "locks"
    locks.mkdir()
    for name in ["saved", "old", "busy", "new", "bad.id"]:
        path = locks / f"{name}.lock"
        path.touch()
        os.utime(path, (_NOW - 7200, _NOW - 7200))
    os.utime(locks / "new.lock", (_NOW, _NOW))
    (locks / "directory.lock").mkdir()
    (locks / "linked.lock").symlink_to(locks / "old.lock")
    with lock_file(locks / "busy.lock", "busy"):
        os.utime(locks / "busy.lock", (_NOW - 7200, _NOW - 7200))
        with lock_file(tmp_path / GLOBAL_LOCK, "busy"):
            assert cleanup(tmp_path, now=_NOW) == 0
            assert (locks / "old.lock").exists()
        assert cleanup(tmp_path, now=_NOW) == 0
    assert not (locks / "old.lock").exists()
    assert (locks / "linked.lock").is_symlink()
    assert all((locks / f"{name}.lock").exists() for name in ["saved", "busy", "new", "bad.id"])
    assert (locks / "directory.lock").is_dir()
    assert remaining(tmp_path) == ["saved"]


def test_missing_database_disables_resource_sweep(tmp_path: Path) -> None:
    path = tmp_path / "locks/old.lock"
    path.parent.mkdir()
    path.touch()
    os.utime(path, (_NOW - 7200, _NOW - 7200))
    assert cleanup(tmp_path, now=_NOW) == 0
    assert path.exists()
    assert not (tmp_path / "agentnave.db").exists()


def test_orphan_seats_expire_after_one_day_without_following_symlinks(tmp_path: Path) -> None:
    fixture(tmp_path, "saved")
    for name in ["old", "new", "saved", "busy", "bad.id"]:
        path = tmp_path / "seats" / name
        path.mkdir(parents=True)
        (path / "output").write_text("data")
        os.utime(path, (_NOW - 2 * 86400, _NOW - 2 * 86400))
    os.utime(tmp_path / "seats/new", (_NOW, _NOW))
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep").write_text("keep")
    (tmp_path / "seats/old/link").symlink_to(outside, target_is_directory=True)
    # Windows cannot age a symlink itself; the linked seat is then left out.
    linked = os.utime in os.supports_follow_symlinks
    if linked:
        (tmp_path / "seats/linked").symlink_to(outside, target_is_directory=True)
        os.utime(
            tmp_path / "seats/linked", (_NOW - 2 * 86400, _NOW - 2 * 86400), follow_symlinks=False
        )
    os.utime(tmp_path / "seats/old", (_NOW - 2 * 86400, _NOW - 2 * 86400))
    (tmp_path / "locks").mkdir()
    with lock_file(tmp_path / "locks/busy.lock", "busy"):
        assert cleanup(tmp_path, now=_NOW) == 0
    assert not (tmp_path / "seats/old").exists()
    assert not (tmp_path / "seats/linked").is_symlink()
    assert not (tmp_path / "locks/old.lock").exists()
    assert not (tmp_path / "locks/linked.lock").exists()
    assert (outside / "keep").read_text() == "keep"
    assert all((tmp_path / "seats" / name).exists() for name in ["new", "saved", "busy", "bad.id"])


@pytest.mark.skipif(os.name == "nt" or os.geteuid() == 0, reason="needs POSIX permissions")
def test_unreadable_orphan_seat_is_removed_without_ending_the_pass(tmp_path: Path) -> None:
    fixture(tmp_path, "saved")
    seat = tmp_path / "seats/orphan"
    (seat / "inner/deep").mkdir(parents=True)
    (seat / "inner/deep/output").write_text("data")
    (seat / "inner/deep/output").chmod(0o444)
    os.utime(seat, (_NOW - 2 * 86400, _NOW - 2 * 86400))
    # A read-only directory blocks unlinking its files; unlistable ones block walking.
    (seat / "inner/deep").chmod(0o555)
    (seat / "inner").chmod(0)
    seat.chmod(0)
    try:
        cleanup(tmp_path, now=_NOW)
    finally:
        for path in [seat, seat / "inner", seat / "inner/deep"]:
            if path.exists():
                path.chmod(0o700)
    assert not seat.exists()
    assert not (tmp_path / "locks/orphan.lock").exists()


@pytest.mark.skipif(os.name != "nt", reason="junctions are Windows only")
def test_orphan_seat_junctions_are_removed_without_touching_their_targets(tmp_path: Path) -> None:
    import subprocess

    def junction(link: Path) -> None:
        subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(outside)], check=True)

    fixture(tmp_path, "saved")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "keep").write_text("keep")
    (outside / "keep").chmod(stat.S_IREAD)
    (tmp_path / "seats/inner").mkdir(parents=True)
    junction(tmp_path / "seats/inner/link")
    junction(tmp_path / "seats/whole")
    for name in ["inner", "whole"]:
        os.utime(tmp_path / "seats" / name, (_NOW - 2 * 86400, _NOW - 2 * 86400))
    cleanup(tmp_path, now=_NOW)
    assert not (tmp_path / "seats/inner").exists()
    assert not os.path.lexists(tmp_path / "seats/whole")
    assert (outside / "keep").read_text() == "keep"
    assert not os.access(outside / "keep", os.W_OK)


def test_orphan_sweeps_spare_case_variants_and_symlinked_directories(tmp_path: Path) -> None:
    fixture(tmp_path, "keep")
    old = (_NOW - 2 * 86400, _NOW - 2 * 86400)
    (tmp_path / "seats/KEEP").mkdir(parents=True)
    (tmp_path / "locks").mkdir()
    (tmp_path / "locks/KEEP.lock").touch()
    for path in [tmp_path / "seats/KEEP", tmp_path / "locks/KEEP.lock"]:
        os.utime(path, old)
    assert cleanup(tmp_path, now=_NOW) == 0
    assert (tmp_path / "seats/KEEP").is_dir() and (tmp_path / "locks/KEEP.lock").is_file()

    outside = tmp_path / "outside"
    (outside / "notes").mkdir(parents=True)
    (outside / "cache.lock").touch()
    for path in [outside / "notes", outside / "cache.lock"]:
        os.utime(path, old)
    for name in ["seats", "locks"]:
        shutil.rmtree(tmp_path / name)
        (tmp_path / name).symlink_to(outside, target_is_directory=True)
    assert cleanup(tmp_path, now=_NOW) == 0
    assert (outside / "notes").is_dir() and (outside / "cache.lock").is_file()


@pytest.mark.parametrize("resource", ["lock", "seat"])
def test_orphan_sweep_rechecks_database_after_acquiring_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, resource: str
) -> None:
    from typing import IO

    fixture(tmp_path, "existing")
    (tmp_path / "locks").mkdir()
    path = tmp_path / "locks/racing.lock" if resource == "lock" else tmp_path / "seats/racing"
    if resource == "lock":
        path.touch()
    else:
        path.mkdir(parents=True)
    os.utime(path, (_NOW - 2 * 86400, _NOW - 2 * 86400))

    def take(path: Path, message: str, *, create: bool = True) -> IO[bytes]:
        guard = lock_file(path, message, create=create)
        if path.name == "racing.lock":
            fixture(tmp_path, "racing", group=None)
        return guard

    monkeypatch.setattr("agentnave.retention.lock_file", take)
    assert cleanup(tmp_path, now=_NOW) == 0
    assert path.exists()
    assert "racing" in remaining(tmp_path)


def test_empty_pending_handovers_expire_at_ninety_days_even_without_history_retention(
    tmp_path: Path,
) -> None:
    fixture(tmp_path, "saved", group="nonempty")
    db = Database(tmp_path)
    boundary = datetime.fromtimestamp(_NOW - 90 * 86400, UTC).isoformat()
    try:
        for id_, released in [
            ("expired", _OLD),
            ("new", "2026-11-03T00:00:00Z"),
            ("boundary", boundary),
            ("claimed", _OLD),
            ("nonempty", _OLD),
            ("bad-time", "yesterday"),
        ]:
            db.release(id_, id_, "codex", None, "handover", released, "title", str(tmp_path))
        db.claim("claimed", "next-boss", "codex", None)
        with sqlite3.connect(tmp_path / "agentnave.db") as connection:
            connection.executemany(
                "INSERT INTO former_bosses VALUES (?, ?)",
                [("expired-boss", "expired"), ("kept-boss", "nonempty")],
            )
        assert cleanup(tmp_path, now=_NOW) == 0
        assert db.workspace("expired") is None
        assert all(
            db.workspace(id_) is not None
            for id_ in ["new", "boundary", "claimed", "nonempty", "bad-time"]
        )
        with sqlite3.connect(tmp_path / "agentnave.db") as connection:
            assert connection.execute("SELECT boss FROM former_bosses").fetchall() == [
                ("kept-boss",)
            ]
    finally:
        db.close()


def test_invalid_retention_warns_each_pass_but_dispatch_settings_are_unchanged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    from agentnave.members import read_max_running

    fixture(tmp_path, "saved")
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    settings(tmp_path, "90")
    for _ in range(2):
        assert cleanup(tmp_path, now=_NOW) == 0
    assert caplog.text.count("History cleanup is disabled") == 2
    assert "30, 90 or 180" in caplog.text
    assert str(tmp_path / "settings.json") in caplog.text
    assert read_max_running() == 8
    (tmp_path / "settings.json").write_text('{"version":true,"history_retention_days":30}')
    assert cleanup(tmp_path, now=_NOW) == 0
    assert caplog.text.count("History cleanup is disabled") == 3
    assert remaining(tmp_path) == ["saved"]


def test_seat_removal_failure_logs_without_rolling_back_committed_history(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    settings(tmp_path)
    fixture(tmp_path, "old")
    seat = tmp_path / "seats/old"
    seat.mkdir(parents=True)

    def fail(*_args: object, **_kwargs: object) -> None:
        raise PermissionError("read-only directory")

    monkeypatch.setattr("agentnave.retention.shutil.rmtree", fail)
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == []
    assert seat.exists()
    assert "could not remove seat directory" in caplog.text


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


@pytest.mark.parametrize("change", ["activity", "sibling", "handover"])
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
            elif change == "handover":
                db = Database(tmp_path)
                try:
                    db.release(
                        _WORKSPACE, _WORKSPACE, "codex", None, "new note", "2026-11-03T00:00:00Z"
                    )
                finally:
                    db.close()
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


def test_pending_handover_preserves_note_until_claimed(tmp_path: Path) -> None:
    settings(tmp_path)
    fixture(tmp_path, "old")
    db = Database(tmp_path)
    try:
        db.release(
            _WORKSPACE, _WORKSPACE, "codex", None, "new handover note", "2026-11-03T00:00:00Z"
        )
        assert cleanup(tmp_path, now=_NOW) == 0
        waiting = db.workspace(_WORKSPACE)
        assert waiting is not None and waiting.pending and waiting.note == "new handover note"
        empty_workspace = "s-" + "b" * 32
        db.release(
            empty_workspace,
            empty_workspace,
            "codex",
            None,
            "empty workspace note",
            "2026-11-03T00:00:00Z",
        )
        db.claim(_WORKSPACE, "new-boss", "codex", None)
        assert cleanup(tmp_path, now=_NOW) == 1
        waiting_empty = db.workspace(empty_workspace)
        assert waiting_empty is not None and waiting_empty.note == "empty workspace note"
    finally:
        db.close()


@pytest.mark.parametrize("broken", ["utf8", "deep-json"])
def test_bad_view_skips_whole_workspace_and_logs_while_cleaning_healthy_ones(
    tmp_path: Path, caplog: pytest.LogCaptureFixture, broken: str
) -> None:
    settings(tmp_path)
    fixture(tmp_path, "a-broken")
    fixture(tmp_path, "b-sibling")
    fixture(tmp_path, "z-good", group=None)
    with sqlite3.connect(tmp_path / "agentnave.db") as db:
        if broken == "utf8":
            db.execute(
                "UPDATE views SET body = CAST(X'80' AS TEXT) WHERE conversation_id = 'a-broken'"
            )
        else:
            body = (
                '{"summary":{"updated":"2020-01-01T00:00:00Z"},"x":'
                + "[" * 10000
                + "0"
                + "]" * 10000
                + "}"
            )
            db.execute("UPDATE views SET body = ? WHERE conversation_id = 'a-broken'", (body,))
    assert cleanup(tmp_path, now=_NOW) == 1
    assert remaining(tmp_path) == ["a-broken", "b-sibling"]
    assert "a-broken" in caplog.text


@pytest.mark.asyncio
async def test_background_pass_survives_unexpected_exception_and_retries_next_period(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    completed = threading.Event()
    calls: list[Path] = []

    def pass_(root: Path, **_kwargs: object) -> int:
        calls.append(root)
        if len(calls) == 1:
            raise RecursionError("nested view")
        completed.set()
        return 0

    monkeypatch.setattr("agentnave.retention.cleanup", pass_)
    monkeypatch.setattr("agentnave.retention._INTERVAL", 0.02)
    worker = Retention(tmp_path)
    try:
        assert await asyncio.to_thread(completed.wait, 1)
        assert "History retention pass failed" in caplog.text
    finally:
        await worker.close()
    assert len(calls) >= 2

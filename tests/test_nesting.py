from __future__ import annotations

import asyncio
import json
import os
import sys
from collections.abc import Sequence
from pathlib import Path

import pytest

from agentnave import nesting
from agentnave.nesting import (
    _detect_posix,  # pyright: ignore[reportPrivateUsage]
    _ProcessInfo,  # pyright: ignore[reportPrivateUsage]
)
from agentnave.processes import spawn_process, terminate_process_tree


def test_posix_signals_use_script_position_and_validate_marker() -> None:
    supervisor = ("/usr/bin/python3.12", "-I", "/pkg/agentnave/supervisor.py", "8")
    table = {
        20: (_ProcessInfo(10, 501), ("codex", "prompt mentions /pkg/agentnave/supervisor.py")),
        10: (_ProcessInfo(1, 501), ("python3", "-c", "print('supervisor.py')")),
        30: (_ProcessInfo(1, 501), supervisor),
    }

    def info(pid: int) -> _ProcessInfo:
        if pid not in table:
            raise ProcessLookupError(pid)
        return table[pid][0]

    def argv(pid: int) -> Sequence[str]:
        return table[pid][1]

    assert _detect_posix(None, 20, 501, info, argv).status == "not_nested"
    # Both nonexistent and live-but-unrelated marker PIDs are stale.
    assert _detect_posix(99, 20, 501, info, argv).status == "not_nested"
    assert _detect_posix(10, 20, 501, info, argv).status == "not_nested"
    assert _detect_posix(30, 20, 501, info, argv).status == "nested"
    table[10] = (_ProcessInfo(1, 501), supervisor)
    assert _detect_posix(None, 20, 501, info, argv).status == "nested"
    # A non-Python CLI or a different package's script must not match either.
    table[10] = (_ProcessInfo(1, 501), ("codex", "/pkg/agentnave/supervisor.py"))
    assert _detect_posix(None, 20, 501, info, argv).status == "not_nested"
    table[10] = (_ProcessInfo(1, 501), ("python3", "/other/supervisor.py"))
    assert _detect_posix(None, 20, 501, info, argv).status == "not_nested"


def test_posix_unreadable_same_user_fails_closed_but_foreign_user_does_not() -> None:
    table = {20: _ProcessInfo(10, 501), 10: _ProcessInfo(1, 0)}

    def argv(pid: int) -> Sequence[str]:
        if pid == 10:
            raise PermissionError("argv denied")
        return ("codex",)

    assert _detect_posix(None, 20, 501, table.__getitem__, argv).status == "not_nested"
    table[10] = _ProcessInfo(1, 501)
    unavailable = _detect_posix(None, 20, 501, table.__getitem__, argv)
    assert unavailable.status == "unavailable" and "argv denied" in unavailable.reason

    def denied_info(_pid: int) -> _ProcessInfo:
        raise PermissionError("process identity denied")

    assert _detect_posix(None, 20, 501, denied_info, argv).status == "unavailable"
    table[10] = _ProcessInfo(20, 501)
    assert _detect_posix(None, 20, 501, table.__getitem__, lambda _: ()).status == "unavailable"
    assert (
        _detect_posix(None, 300, 501, lambda p: _ProcessInfo(p - 1, 501), lambda _: ()).status
        == "unavailable"
    )


def test_windows_marker_requires_live_pid(monkeypatch: pytest.MonkeyPatch) -> None:
    def alive(pid: int) -> bool:
        return pid == 20

    monkeypatch.setattr(nesting.sys, "platform", "win32")
    monkeypatch.setattr(nesting, "_windows_alive", alive)
    monkeypatch.setenv(nesting.SUPERVISOR_ENV, "20")
    assert nesting.detect_nesting().status == "nested"
    monkeypatch.setenv(nesting.SUPERVISOR_ENV, "99")
    assert nesting.detect_nesting().status == "not_nested"
    monkeypatch.setenv(nesting.SUPERVISOR_ENV, "not-a-pid")
    assert nesting.detect_nesting().status == "not_nested"
    monkeypatch.delenv(nesting.SUPERVISOR_ENV)
    assert nesting.detect_nesting().status == "not_nested"


@pytest.mark.asyncio
async def test_supervisor_sets_own_marker_and_ancestry_survives_filtered_env(
    tmp_path: Path,
) -> None:
    original = os.environ.get(nesting.SUPERVISOR_ENV)
    code = (
        "import json, os; from agentnave.nesting import detect_nesting; "
        "marked_status = detect_nesting().status; "
        "marker = os.environ.pop('AGENTNAVE_SUPERVISOR'); "
        "print(json.dumps([marker, os.getppid(), marked_status, detect_nesting().status]), flush=True)"
    )
    supervised = await spawn_process((sys.executable, "-c", code), tmp_path)
    try:
        async with asyncio.timeout(10):
            completion = await supervised.completion
            assert completion.returncode == 0
            assert supervised.process.stdout is not None
            marker, parent, marked_status, status = json.loads(
                await supervised.process.stdout.readline()
            )
            assert marker == str(supervised.process.pid)
            assert parent == supervised.process.pid
            assert marked_status == "nested"
            if os.name == "posix":
                assert status == "nested"
            else:
                assert status == "not_nested"  # Windows only has the marker signal.
    finally:
        await terminate_process_tree(supervised.process, grace_seconds=0.01)
    assert os.environ.get(nesting.SUPERVISOR_ENV) == original

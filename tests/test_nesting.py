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


def test_host_env_priority_validation_and_windows_fallback(monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave import host

    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.setattr(host.sys, "platform", "win32")
    assert host.detect_host() == host.Host()
    for name, provider in (
        ("codex", "codex"),
        ("workbuddy", "codebuddy"),
        ("gemini", None),
        ("opencode", None),
        ("hermes", None),
    ):
        monkeypatch.setenv("AGENTNAVE_HOST", name)
        identified = host.detect_host()
        assert identified.name == name and identified.source == "env"
        assert identified.provider == provider
    for value in ("unknown", "", "CODEX"):
        monkeypatch.setenv("AGENTNAVE_HOST", value)
        with pytest.raises(ValueError, match="Unknown AGENTNAVE_HOST"):
            host.detect_host()


def test_host_parent_matching_scripts_boundaries_and_unreadable_argv() -> None:
    from agentnave.host import Host, _detect_parent  # pyright: ignore[reportPrivateUsage]

    parents = {80: 70, 70: 60, 60: 50, 50: 40, 40: 1}
    table: dict[int, tuple[str, ...]] = {
        80: ("/usr/bin/python3.12", "/venv/bin/agentnave-studio-mcp"),
        70: ("/bin/zsh",),
        60: ("/usr/bin/env",),
        50: ("/usr/bin/node", "/pkg/server.js"),
        40: ("/Applications/WorkBuddy.app/Contents/MacOS/Electron",),
    }
    assert _detect_parent(80, parents.__getitem__, table.__getitem__) == Host("workbuddy", "parent")
    for executable, name in (
        ("/usr/bin/codex", "codex"),
        ("claude", "claude"),
        ("codebuddy", "codebuddy"),
        ("grok", "grok"),
        ("agy", "antigravity"),
        ("pi", "pi"),
        ("gemini", "gemini"),
        ("opencode", "opencode"),
        ("hermes", "hermes"),
        ("/Applications/Codex.app/Contents/MacOS/Electron", "codex"),
        ("/Applications/Claude.app/Contents/MacOS/Claude", "claude"),
        ("/Applications/Antigravity.app/Contents/MacOS/Electron", "antigravity"),
        ("/Users/test/.local/share/claude/versions/2.1.295", "claude"),
    ):
        table[40] = (executable,)
        assert _detect_parent(80, parents.__getitem__, table.__getitem__).name == name
    table[40] = ("/Applications/Antigravity.app/Contents/MacOS/Electron",)
    for interpreter, script, expected in (
        ("node", "/pkg/codebuddy.js", "codebuddy"),
        ("bun", "/pkg/pi.mjs", "pi"),
        ("python", "/pkg/hermes.py", "hermes"),
        ("python3.12", "/pkg/gemini.py", "gemini"),
        ("deno", "/pkg/opencode.cjs", "opencode"),
        ("uvx", "hermes", "hermes"),
        ("npx", "codebuddy", "codebuddy"),
    ):
        table[50] = (interpreter, "--quiet", script, "prompt mentions claude")
        assert _detect_parent(80, parents.__getitem__, table.__getitem__).name == expected
    # An unrecognized non-wrapper is a boundary, even with an outer GUI host.
    table[50] = ("custom-cli", "codex")
    assert _detect_parent(80, parents.__getitem__, table.__getitem__) == Host()
    table[50] = ("node", "other.js", "codex")
    table[40] = ("custom-cli",)
    assert _detect_parent(80, parents.__getitem__, table.__getitem__) == Host()
    assert (
        _detect_parent(100, lambda p: p - 1, lambda p: ("codex",) if p == 68 else ("sh",)) == Host()
    )
    assert _detect_parent(20, lambda _: 20, lambda _: ("sh",)) == Host()

    def argv(pid: int) -> Sequence[str]:
        if pid == 80:
            raise PermissionError("unreadable argv")
        return ("codebuddy",)

    assert _detect_parent(80, parents.__getitem__, argv) == Host("codebuddy", "parent")

    def denied_parent(_pid: int) -> int:
        raise PermissionError("unreadable parent")

    assert _detect_parent(80, denied_parent, argv) == Host()


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
            if os.name == "posix":
                assert marker == str(supervised.process.pid)
                assert parent == supervised.process.pid
            else:
                # venv python.exe launchers sit between Popen, the supervisor and the provider.
                assert marker.isdigit()
            assert marked_status == "nested"
            if os.name == "posix":
                assert status == "nested"
            else:
                assert status == "not_nested"  # Windows only has the marker signal.
    finally:
        await terminate_process_tree(supervised.process, grace_seconds=0.01)
    assert os.environ.get(nesting.SUPERVISOR_ENV) == original

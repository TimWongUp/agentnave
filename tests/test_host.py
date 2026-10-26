from __future__ import annotations

import json
import os
import sys
from collections.abc import Sequence
from pathlib import Path

import pytest

from agentnave import process_info
from agentnave.process_info import _ProcessInfo  # pyright: ignore[reportPrivateUsage]


def test_native_session_model_uses_exact_chat_latest_turn_not_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.host import Host, session_model

    monkeypatch.setenv("CODEX_HOME", str(tmp_path))
    session = "01a14d5a-d23d-7263-b50d-ca6b5ac429dd"
    folder = tmp_path / "sessions" / "2026" / "10" / "18"
    folder.mkdir(parents=True)
    path = folder / f"rollout-2026-10-18T01-00-00-{session}.jsonl"

    def turn(model: str, timestamp: str) -> str:
        return (
            json.dumps(
                {"type": "turn_context", "timestamp": timestamp, "payload": {"model": model}}
            )
            + "\n"
        )

    path.write_text(
        turn("old-model", "2026-01-01T00:00:00Z")
        + turn("current-model", "2026-01-01T01:00:00Z")
        # Generated images make single transcript lines larger than the line limit.
        + json.dumps({"type": "event_msg", "image": "x" * 3_000_000})
        + "\n"
        + turn("future-model", "2999-01-01T00:00:00Z")
        + '{"partial":'
    )
    (tmp_path / "config.toml").write_text('model = "wrong-default"')
    assert session_model(Host("codex"), session) == "current-model"
    from datetime import UTC, datetime

    from agentnave.host import _lines_from_end  # pyright: ignore[reportPrivateUsage]

    lines = tmp_path / "lines.txt"
    lines.write_bytes(b"old\n" + b"x" * 9 + b"\nnew\n")
    for chunk in (3, 4, 64):
        assert list(_lines_from_end(lines, chunk=chunk, longest=8)) == [b"new", b"old"]
        assert list(_lines_from_end(lines, chunk=chunk, longest=9)) == [b"new", b"x" * 9, b"old"]

    assert session_model(Host("codex"), session, since=datetime(2026, 2, 1, tzinfo=UTC)) is None
    assert session_model(Host("codex"), "01a14d5a-d23d-7263-b50d-ca6b5ac429de") is None
    assert session_model(Host("codex"), "../" + session) is None
    assert session_model(Host("grok"), session) is None
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    claude = tmp_path / "projects" / "project" / f"{session}.jsonl"
    claude.parent.mkdir(parents=True)
    claude.write_text(
        json.dumps(
            {
                "type": "assistant",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {"model": "claude-main"},
            }
        )
        + "\n"
        + json.dumps(
            {
                "type": "assistant",
                "timestamp": "2026-01-01T01:00:00Z",
                "parent_tool_use_id": "tool-1",
                "message": {"model": "child"},
            }
        )
    )
    assert session_model(Host("claude"), session) == "claude-main"


def test_host_env_priority_validation_and_windows_fallback(monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave import host

    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.setattr(host.sys, "platform", "win32")
    assert host.detect_host() == host.Host()
    for name, provider in (
        ("codex", "codex"),
        ("workbuddy", "codebuddy"),
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


def test_macos_info_reads_privileged_ancestors() -> None:
    if sys.platform != "darwin":
        pytest.skip("macOS proc_pidinfo flavors")
    # Root-owned processes (launchd, a Terminal `login`) deny full BSD info to us. Their ppid
    # must still be readable for best-effort host identification.
    assert process_info._macos_info(1).uid == 0  # pyright: ignore[reportPrivateUsage]
    own = process_info._macos_info(os.getpid())  # pyright: ignore[reportPrivateUsage]
    assert own == _ProcessInfo(os.getppid(), os.geteuid())

from __future__ import annotations

import json
import os
import sys
from collections.abc import Sequence
from pathlib import Path

import pytest

from agentnave import process_info
from agentnave.process_info import _ProcessInfo  # pyright: ignore[reportPrivateUsage]


def test_session_copied_between_project_folders_reads_the_live_transcript(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.host import Host, session_file, session_model

    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    session = "01a14d5a-d23d-7263-b50d-ca6b5ac429dd"

    def transcript(project: str, model: str, modified: int) -> Path:
        path = tmp_path / "projects" / project / f"{session}.jsonl"
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps(
                {
                    "type": "assistant",
                    "timestamp": "2026-01-01T00:00:00Z",
                    "message": {"model": model},
                }
            )
            + "\n"
        )
        os.utime(path, (modified, modified))
        return path

    transcript("a-stale-worktree", "stale-model", 1_000)
    live = transcript("main", "live-model", 2_000)
    transcript("z-stale-worktree", "other-stale-model", 1_500)

    assert session_model(Host("claude"), session) == "live-model"
    assert session_file(Host("claude"), session) == live


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
    # Without an argument or injected ID, Claude Code's own session ID names the boss chat.
    from agentnave.host import host_session_id

    monkeypatch.delenv("AGENTNAVE_HOST_SESSION_ID")
    monkeypatch.setenv("CLAUDE_CODE_SESSION_ID", session)
    assert session_model(Host("claude"), host_session_id(Host("claude"))) == "claude-main"
    assert host_session_id(Host("codex")) is None
    monkeypatch.setenv("AGENTNAVE_HOST_SESSION_ID", "injected")
    assert host_session_id(Host("claude")) == "injected"

    # Pi, Grok, Antigravity and Hermes keep their own records under the IDs their shells expose.
    import sqlite3

    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path / "pi"))
    pi = tmp_path / "pi" / "sessions" / "--project--" / f"2026-01-01T00-00-00-000Z_{session}.jsonl"
    pi.parent.mkdir(parents=True)
    pi.write_text(
        json.dumps(
            {
                "type": "message",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {"role": "assistant", "model": "zcode/GLM-5.3", "responseModel": "glm"},
            }
        )
    )
    assert session_model(Host("pi"), session) == "glm"
    monkeypatch.setenv("CODEBUDDY_CONFIG_DIR", str(tmp_path / "codebuddy"))
    buddy = tmp_path / "codebuddy" / "projects" / "project" / f"{session}.jsonl"
    buddy.parent.mkdir(parents=True)
    line = {"type": "message", "role": "assistant", "timestamp": 1767225600000}
    buddy.write_text(json.dumps(line | {"providerData": {"model": "glm-5.3-flash"}}))
    assert session_model(Host("codebuddy"), session) == "glm-5.3-flash"
    # WorkBuddy, CodeBuddy's desktop app, keeps the same records.
    assert session_model(Host("workbuddy"), session) == "glm-5.3-flash"
    monkeypatch.setenv("GROK_HOME", str(tmp_path / "grok"))
    grok = tmp_path / "grok" / "sessions" / "%2Fproject" / session / "chat_history.jsonl"
    grok.parent.mkdir(parents=True)
    grok.write_text(json.dumps({"type": "assistant", "model_id": "grok-build"}) + "\n{}")
    assert session_model(Host("grok"), session) == "grok-build"
    monkeypatch.setattr(Path, "home", staticmethod(lambda: tmp_path))
    agy = tmp_path / ".gemini" / "antigravity-cli" / "conversations" / f"{session}.db"
    agy.parent.mkdir(parents=True)
    model = b"gemini-high"
    step = b"\xe2\x01" + bytes([len(model)]) + model  # field 28 inside field 3
    with sqlite3.connect(agy) as db:
        db.execute("CREATE TABLE gen_metadata (idx integer, data blob)")
        db.executemany(
            "INSERT INTO gen_metadata VALUES (?, ?)",
            [(0, b"\x1a" + bytes([len(step)]) + step), (1, b"\x0a\x02no")],
        )
    db.close()
    assert session_model(Host("antigravity"), session) == "gemini-high"
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / "hermes"))
    (tmp_path / "hermes").mkdir()
    hermes = "20261015_160609_65ab47af"
    with sqlite3.connect(tmp_path / "hermes" / "state.db") as db:
        db.execute("CREATE TABLE sessions (id text, model text)")
        db.execute("INSERT INTO sessions VALUES (?, ?)", (hermes, "deepseek-flash"))
    db.close()
    assert session_model(Host("hermes"), hermes) == "deepseek-flash"
    assert session_model(Host("hermes"), "../state") is None
    # A relative HERMES_HOME has no file URI; it reads as unknown instead of failing the call.
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("HERMES_HOME", "hermes")
    assert session_model(Host("hermes"), hermes) is None
    # Malformed protobuf ends the scan instead of growing one varint without bound.
    from agentnave.host import _proto_fields  # pyright: ignore[reportPrivateUsage]

    assert list(_proto_fields(b"\xff" * 100_000)) == []


def test_host_env_priority_validation_and_windows_fallback(monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave import host

    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.setattr(host.sys, "platform", "win32")
    assert host.detect_host() == host.Host()
    for name, provider in (
        ("codex", "codex"),
        ("workbuddy", "codebuddy"),
        ("deepseek", "deepseek"),
        ("hermes", None),
    ):
        monkeypatch.setenv("AGENTNAVE_HOST", name)
        identified = host.detect_host()
        assert identified.name == name and identified.source == "env"
        assert identified.provider == provider
    for value in ("unknown", "", "CODEX", "opencode"):
        monkeypatch.setenv("AGENTNAVE_HOST", value)
        with pytest.raises(ValueError, match="Unknown AGENTNAVE_HOST"):
            host.detect_host()


def test_retired_host_history_still_loads() -> None:
    from agentnave.store import Conversation

    for retired in ("opencode", "gemini"):
        saved = {"id": "c", "title": "t", "kind": "task", "host": retired}
        assert Conversation.model_validate(saved).host == retired


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
        ("dsh", "deepseek"),
        ("hermes", "hermes"),
        ("/Applications/Codex.app/Contents/MacOS/Electron", "codex"),
        ("/Applications/Claude.app/Contents/MacOS/Claude", "claude"),
        ("/Applications/Antigravity.app/Contents/MacOS/Electron", "antigravity"),
        ("/Applications/DeepSeek Harness.app/Contents/MacOS/DeepSeek Harness", "deepseek"),
        ("/Users/test/.local/share/claude/versions/2.1.295", "claude"),
    ):
        table[40] = (executable,)
        assert _detect_parent(80, parents.__getitem__, table.__getitem__).name == name
    table[40] = ("/Applications/Antigravity.app/Contents/MacOS/Electron",)
    for interpreter, script, expected in (
        ("node", "/pkg/codebuddy.js", "codebuddy"),
        ("bun", "/pkg/pi.mjs", "pi"),
        ("python", "/pkg/hermes.py", "hermes"),
        ("deno", "/pkg/grok.cjs", "grok"),
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


def test_zcode_reads_only_native_assistant_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import sqlite3

    from agentnave.host import Host, _match, session_model  # pyright: ignore[reportPrivateUsage]

    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    database = tmp_path / ".zcode/cli/db/db.sqlite"
    database.parent.mkdir(parents=True)
    session = "sess_11111111-1111-4111-8111-111111111111"
    with sqlite3.connect(database) as db:
        db.execute(
            "CREATE TABLE message (session_id TEXT, time_created INTEGER, sequence INTEGER, data TEXT)"
        )
        for sid, role, model, sequence in [
            (session, "assistant", "actual", 1),
            (session, "user", "requested", 2),
            ("other", "assistant", "other", 3),
            ("session_0123456789abcdef0123456789abcdef", "assistant", "desktop-model", 4),
        ]:
            db.execute(
                "INSERT INTO message VALUES (?, 1000, ?, ?)",
                (sid, sequence, json.dumps({"role": role, "modelId": model})),
            )
    assert session_model(Host("zcode"), session) == "actual"
    assert (
        session_model(Host("zcode"), "session_0123456789abcdef0123456789abcdef") == "desktop-model"
    )
    assert session_model(Host("zcode"), "../bad") is None
    assert _match("/Applications/ZCode.app/Contents/MacOS/ZCode") == "zcode"

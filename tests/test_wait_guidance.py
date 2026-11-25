from __future__ import annotations

import os
from pathlib import Path
from typing import Any, cast

import pytest
from mcp import Client

from agentnave import mcp_runtime, wait_guidance
from agentnave.host import Host
from agentnave.mcp_server import mcp
from agentnave.process_info import read_process_executable
from agentnave.push_capability import PUSH_AGAIN, PUSH_NEXT


def _clear_claude_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in (
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
        "CLAUDE_CODE_DISABLE_MCP_TASK_BACKGROUND",
        "CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS",
    ):
        monkeypatch.delenv(name, raising=False)


def _always_push(_root: Path, _provider: str | None) -> bool:
    return True


def _never_push(_root: Path, _provider: str | None) -> bool:
    return False


def _watch_none(_host: str) -> bool:
    return False


def _watch_all(_host: str) -> bool:
    return True


@pytest.mark.skipif(os.name == "nt", reason="POSIX executable path")
def test_read_process_executable_returns_current_python() -> None:
    exe = read_process_executable(os.getpid())
    assert Path(exe).is_file()
    # Framework builds (python.org, Homebrew) re-exec into Python.app, so compare by name.
    assert Path(exe).name.lower().startswith("python")


def test_probe_watch_host_cli_agy_rules() -> None:
    # Chain: mcp_server -> python relay (30) -> sh launcher (20) -> agy (10)
    parents = {30: 20, 20: 10, 10: 1}
    exes = {
        30: "/opt/venv/bin/python3.12",
        20: "/bin/sh",
        10: "/Users/tim/.local/bin/agy",
    }
    argvs: dict[int, tuple[str, ...]] = {
        30: ("python3.12", "-P", "-c", "from agentnave.mcp_server import main"),
        20: ("/bin/sh", "/Users/tim/.agentnave/service/bin/agentnave-mcp"),
        10: ("agy",),
    }

    def parent(pid: int) -> int:
        return parents[pid]

    def executable(pid: int) -> str:
        return exes[pid]

    def argv(pid: int) -> tuple[str, ...]:
        return argvs[pid]

    # Interactive agy >= 1.5.14 (including multi-digit minor like 1.10.0) succeeds.
    assert wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "agy 1.5.14\n",
        platform="darwin",
    )
    assert wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "1.10.0\n",
        platform="linux",
    )

    # Older version or version failure returns False.
    assert not wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "agy 1.5.13\n",
        platform="darwin",
    )
    assert not wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: None,
        platform="darwin",
    )

    # Windows always returns False.
    assert not wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "agy 1.5.14\n",
        platform="win32",
    )

    # Non-interactive flags (-p/--print/--prompt, including `=` form) and `acp` subcommand return False.
    for bad_argv in (
        ("agy", "-p", "hi"),
        ("agy", "-p=hi"),
        ("agy", "--print", "hi"),
        ("agy", "--print=true"),
        ("agy", "--prompt", "hi"),
        ("agy", "--prompt=hi"),
        ("agy", "acp"),
        ("agy", "--settings", "{}", "acp"),
        ("agy", "--subclient_type", "hub"),
        ("agy", "--subclient_type=hub"),
    ):
        argvs[10] = bad_argv
        assert not wait_guidance.probe_watch_host_cli(
            "antigravity",
            parent_pid=30,
            read_parent=parent,
            read_executable=executable,
            read_argv=argv,
            read_version=lambda _exe: "agy 1.5.14\n",
            platform="darwin",
        ), bad_argv

    # T3 Code starts agy as `agy acp ...`: allow_acp accepts it, other gates still apply.
    argvs[10] = ("agy", "acp", "--gemini-dir", "/t3/profile")
    for version, expected in (("1.5.16\n", True), ("1.5.13\n", False)):
        assert (
            wait_guidance.probe_watch_host_cli(
                "antigravity",
                parent_pid=30,
                read_parent=parent,
                read_executable=executable,
                read_argv=argv,
                read_version=lambda _exe, v=version: v,
                platform="darwin",
                allow_acp=True,
            )
            is expected
        )
    argvs[10] = ("agy", "acp", "-p", "hi")
    assert not wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "1.5.16\n",
        platform="darwin",
        allow_acp=True,
    )

    # Antigravity.app (both language_server and any binary inside Antigravity.app) returns False.
    argvs[10] = ("language_server_macos_arm64", "--subclient_type", "hub")
    exes[10] = "/Applications/Antigravity.app/Contents/Resources/app/language_server_macos_arm64"
    assert not wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "2.34.0\n",
        platform="darwin",
    )
    argvs[10] = ("agy",)
    exes[10] = "/Applications/Antigravity.app/Contents/MacOS/agy"
    assert not wait_guidance.probe_watch_host_cli(
        "antigravity",
        parent_pid=30,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "1.5.14\n",
        platform="darwin",
    )


def test_probe_watch_host_cli_grok_rules_and_versioned_binary() -> None:
    parents = {20: 10, 10: 1}
    # Symlink target on macOS/Linux resolves to versioned binary name `grok-1.0.92`.
    exes = {
        20: "/Users/tim/.agentnave/service/bin/agentnave-mcp",
        10: "/Users/tim/.grok/bin/grok-1.0.92",
    }
    argvs: dict[int, tuple[str, ...]] = {
        20: ("agentnave-mcp",),
        10: ("grok",),
    }

    def parent(pid: int) -> int:
        return parents[pid]

    def executable(pid: int) -> str:
        return exes[pid]

    def argv(pid: int) -> tuple[str, ...]:
        return argvs[pid]

    assert wait_guidance.probe_watch_host_cli(
        "grok",
        parent_pid=20,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "grok 1.0.92 (b06756fd6256)\n",
        platform="darwin",
    )
    exes[10] = "/usr/local/bin/grok"
    assert wait_guidance.probe_watch_host_cli(
        "grok",
        parent_pid=20,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "grok 1.0.92 (b06756fd6256)\n",
        platform="linux",
    )
    # Below minimum version returns False.
    assert not wait_guidance.probe_watch_host_cli(
        "grok",
        parent_pid=20,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "grok 1.0.91 (abcdef)\n",
        platform="darwin",
    )
    # Headless flags return False.
    for bad_argv in (
        ("grok", "-p", "hi"),
        ("grok", "--print"),
        ("grok", "--prompt=hi"),
        ("grok", "--prompt-file", "/tmp/p.txt"),
        ("grok", "--prompt-file=/tmp/p.txt"),
    ):
        argvs[10] = bad_argv
        assert not wait_guidance.probe_watch_host_cli(
            "grok",
            parent_pid=20,
            read_parent=parent,
            read_executable=executable,
            read_argv=argv,
            read_version=lambda _exe: "grok 1.0.92 (b06756fd6256)\n",
            platform="darwin",
        ), bad_argv
    # Unrelated binary name returns False.
    argvs[10] = ("grok-exporter",)
    exes[10] = "/usr/local/bin/grok-exporter"
    assert not wait_guidance.probe_watch_host_cli(
        "grok",
        parent_pid=20,
        read_parent=parent,
        read_executable=executable,
        read_argv=argv,
        read_version=lambda _exe: "grok 1.0.92\n",
        platform="darwin",
    )


def test_watch_host_cli_supported_is_cached_and_ignores_agentnave_host_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[str] = []

    def fake_probe(host_name: str, *, allow_acp: bool = False) -> bool:
        calls.append(host_name)
        return host_name == "grok"

    monkeypatch.setattr(wait_guidance, "_WATCH_HOST_CACHE", {})
    monkeypatch.setattr(wait_guidance, "probe_watch_host_cli", fake_probe)
    monkeypatch.setenv("AGENTNAVE_HOST", "antigravity")
    assert wait_guidance.watch_host_cli_supported("antigravity") is False
    assert wait_guidance.watch_host_cli_supported("antigravity") is False
    assert wait_guidance.watch_host_cli_supported("grok") is True
    assert wait_guidance.watch_host_cli_supported("grok") is True
    assert calls == ["antigravity", "grok"]


def test_claude_guidance_branches_and_never_uses_push(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _clear_claude_env(monkeypatch)
    host = Host("claude", "env")

    # cli entrypoint with background enabled -> long wait, even when App push reports True.
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
    assert wait_guidance.long_wait_enabled_for_host(host)
    assert (
        wait_guidance.build_wait_guidance(
            host, root=tmp_path, pid=1, started_at=10.0, can_push=_always_push
        )
        == wait_guidance.CLAUDE_LONG_WAIT_NEXT
    )
    assert (
        wait_guidance.build_wait_guidance(
            host, root=tmp_path, pid=1, started_at=10.0, again=True, can_push=_always_push
        )
        == wait_guidance.CLAUDE_LONG_WAIT_AGAIN
    )

    # Boundary: 1500000ms is not > 1500000 so long wait stays enabled; 1500001 disables it.
    monkeypatch.setenv("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS", "1500000")
    assert wait_guidance.long_wait_enabled_for_host(host)
    for var, value in (
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1"),
        ("CLAUDE_CODE_DISABLE_MCP_TASK_BACKGROUND", "true"),
        ("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS", "1500001"),
    ):
        _clear_claude_env(monkeypatch)
        monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
        monkeypatch.setenv(var, value)
        assert not wait_guidance.long_wait_enabled_for_host(host)
        next_text = wait_guidance.build_wait_guidance(
            host, root=tmp_path, pid=1, started_at=10.0, can_push=_always_push
        )
        assert next_text == wait_guidance.CLAUDE_SHORT_WAIT_NEXT
        assert "wait_seconds=270" in next_text
        again_text = wait_guidance.build_wait_guidance(
            host, root=tmp_path, pid=1, started_at=10.0, again=True, can_push=_always_push
        )
        assert again_text == wait_guidance.CLAUDE_SHORT_WAIT_AGAIN
        assert "wait_seconds=270" in again_text

    # Non-cli Claude (e.g. desktop app) uses generic foreground WAIT_NEXT.
    _clear_claude_env(monkeypatch)
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")
    assert not wait_guidance.long_wait_enabled_for_host(host)
    assert (
        wait_guidance.build_wait_guidance(
            host, root=tmp_path, pid=1, started_at=10.0, can_push=_always_push
        )
        == wait_guidance.WAIT_NEXT
    )


def test_agy_and_grok_watch_guidance_wording_and_fallback(tmp_path: Path) -> None:
    agy = Host("antigravity", "env")
    grok = Host("grok", "env")

    # When probe returns False, both fall back to foreground wait (never PUSH_NEXT).
    assert (
        wait_guidance.build_wait_guidance(
            agy,
            root=tmp_path,
            pid=42,
            started_at=1700000000.25,
            boss_key="s-boss",
            can_push=_always_push,
            watch_supported=_watch_none,
        )
        == wait_guidance.WAIT_NEXT
    )
    assert (
        wait_guidance.build_wait_guidance(
            grok,
            root=tmp_path,
            pid=42,
            started_at=1700000000.25,
            boss_key="s-boss",
            can_push=_always_push,
            watch_supported=_watch_none,
        )
        == wait_guidance.WAIT_NEXT
    )

    # agy watch guidance: auto-backgrounds after 10s, handles foreground return within 10s.
    agy_text = wait_guidance.build_wait_guidance(
        agy,
        root=tmp_path,
        pid=42,
        started_at=1700000000.25,
        boss_key="s-boss",
        can_push=_always_push,
        watch_supported=_watch_all,
    )
    assert "-I -m agentnave.inbox_watch" in agy_text
    assert "42 1700000000.250000 s-boss" in agy_text
    assert "10s" in agy_text
    assert "run_terminal_command" not in agy_text
    assert "wait(wait_seconds=0)" in agy_text
    for outcome in ("ready", "expired", "already_watching", "all_done", "service_gone"):
        assert outcome in agy_text

    # grok watch guidance: explicit run_terminal_command + block_until_ms: 0.
    grok_text = wait_guidance.build_wait_guidance(
        grok,
        root=tmp_path,
        pid=42,
        started_at=1700000000.25,
        boss_key="s-boss",
        can_push=_always_push,
        watch_supported=_watch_all,
    )
    assert "-I -m agentnave.inbox_watch" in grok_text
    assert "42 1700000000.250000 s-boss" in grok_text
    assert "run_terminal_command" in grok_text
    assert "block_until_ms: 0" in grok_text
    assert "wait(wait_seconds=0)" in grok_text
    for outcome in ("ready", "expired", "already_watching", "all_done", "service_gone"):
        assert outcome in grok_text

    # Again variant asks to handle prompt_pending / review_needed first.
    grok_again = wait_guidance.build_wait_guidance(
        grok,
        root=tmp_path,
        pid=42,
        started_at=1700000000.25,
        boss_key="s-boss",
        again=True,
        can_push=_always_push,
        watch_supported=_watch_all,
    )
    assert grok_again.startswith("Still pending:")
    assert "block_until_ms: 0" in grok_again


def test_codex_push_and_pi_and_other_hosts_foreground_wait(tmp_path: Path) -> None:
    # Codex keeps PUSH_NEXT/PUSH_AGAIN when inside push-capable App.
    codex = Host("codex", "env")
    assert (
        wait_guidance.build_wait_guidance(
            codex, root=tmp_path, pid=1, started_at=1.0, can_push=_always_push
        )
        == PUSH_NEXT
    )
    assert (
        wait_guidance.build_wait_guidance(
            codex, root=tmp_path, pid=1, started_at=1.0, again=True, can_push=_always_push
        )
        == PUSH_AGAIN
    )
    assert (
        wait_guidance.build_wait_guidance(
            codex, root=tmp_path, pid=1, started_at=1.0, can_push=_never_push
        )
        == wait_guidance.CODEX_WAIT_NEXT
    )
    assert (
        wait_guidance.build_wait_guidance(
            codex, root=tmp_path, pid=1, started_at=1.0, again=True, can_push=_never_push
        )
        == wait_guidance.CODEX_WAIT_AGAIN
    )
    assert wait_guidance.CODEX_WAIT_AGAIN in wait_guidance.BLOCKING_WAIT_AGAIN
    for token in (
        "functions.exec",
        "yield_time_ms",
        "300000",
        "functions.wait",
        "cell_id",
        "already_waiting",
    ):
        assert token in wait_guidance.CODEX_WAIT_NEXT
        assert token in wait_guidance.CODEX_WAIT_AGAIN

    # Pi and other hosts never use PUSH_NEXT or watch command even if can_push/watch_supported are True.
    for name in ("pi", "codebuddy", "deepseek", "zcode", None):
        host = Host(name, "env") if name else Host()
        assert (
            wait_guidance.build_wait_guidance(
                host,
                root=tmp_path,
                pid=1,
                started_at=1.0,
                boss_key="s-boss",
                can_push=_always_push,
                watch_supported=_watch_all,
            )
            == wait_guidance.WAIT_NEXT
        )
        assert (
            wait_guidance.build_wait_guidance(
                host,
                root=tmp_path,
                pid=1,
                started_at=1.0,
                boss_key="s-boss",
                again=True,
                can_push=_always_push,
                watch_supported=_watch_all,
            )
            == wait_guidance.WAIT_AGAIN
        )


@pytest.mark.asyncio
async def test_mcp_wait_hides_1500_when_claude_disables_background_tasks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _clear_claude_env(monkeypatch)
    monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
    monkeypatch.setenv("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1")
    async with Client(mcp) as client:
        tools = {tool.name: tool for tool in (await client.list_tools()).tools}
        wait_seconds = cast(dict[str, object], tools["wait"].input_schema["properties"])[
            "wait_seconds"
        ]
        assert "1500" not in str(wait_seconds)
        rejected = await client.call_tool("wait", {"ids": ["missing"], "wait_seconds": 1500})
        assert rejected.is_error is True
        assert "background tasks enabled" in str(rejected.content)


@pytest.mark.asyncio
async def test_mcp_codex_blocking_wait_includes_exec_yield_and_seq_increment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import push_capability

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex", "env"))
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(push_capability, "app_can_push", _never_push)
    monkeypatch.setattr(mcp_runtime, "app_can_push", _never_push)
    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "cwd": str(tmp_path),
                "prompt": "Private instruction for you only:\nSLEEP",
            },
        )
        assert not started.is_error, started.content
        started_payload = cast(dict[str, object], started.structured_content)
        assert started_payload["next"] == wait_guidance.CODEX_WAIT_NEXT
        elapsed = await client.call_tool(
            "wait",
            {"ids": [started_payload["invocation_id"]], "wait_seconds": 0, "seq": 1},
        )
        assert not elapsed.is_error, elapsed.content
        elapsed_payload = cast(dict[str, object], elapsed.structured_content)
        assert elapsed_payload["next"] == f"{wait_guidance.CODEX_WAIT_AGAIN} Next wait: seq=2."
        await client.call_tool("cancel", {"id": started_payload["invocation_id"]})


def test_t3_main_wait_guidance_for_claude_and_codex_ignores_cli_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _clear_claude_env(monkeypatch)
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
    claude = Host("claude", "env")
    codex = Host("codex", "env")

    # T3 main thread + Claude: Bash run_in_background + timeout 7200000 + t3_background_wait.
    claude_next = wait_guidance.build_wait_guidance(
        claude,
        root=tmp_path,
        pid=42,
        started_at=1700000000.5,
        boss_key="s-boss",
        can_push=_always_push,
        watch_supported=_watch_none,
        t3_main=True,
    )
    assert "-I -m agentnave.inbox_watch" in claude_next
    assert "42 1700000000.500000 s-boss" in claude_next
    assert "Bash" in claude_next
    assert "run_in_background=true" in claude_next
    assert "timeout=7200000" in claude_next
    assert "t3_background_wait" in claude_next
    assert "T3 will wake you" in claude_next
    assert "start the watch command again" in claude_next
    for outcome in ("ready", "expired", "already_watching", "all_done", "service_gone"):
        assert outcome in claude_next
    assert claude_next not in wait_guidance.BLOCKING_WAIT_AGAIN

    claude_again = wait_guidance.build_wait_guidance(
        claude,
        root=tmp_path,
        pid=42,
        started_at=1700000000.5,
        boss_key="s-boss",
        again=True,
        can_push=_always_push,
        watch_supported=_watch_none,
        t3_main=True,
    )
    assert claude_again.startswith("Still pending:")
    assert "t3_background_wait" in claude_again

    # T3 main thread + Codex: background terminal without polling, ignores can_push and watch_supported.
    codex_next = wait_guidance.build_wait_guidance(
        codex,
        root=tmp_path,
        pid=42,
        started_at=1700000000.5,
        boss_key="s-boss",
        can_push=_always_push,
        watch_supported=_watch_none,
        t3_main=True,
    )
    assert "-I -m agentnave.inbox_watch" in codex_next
    assert "background terminal" in codex_next
    assert "without polling" in codex_next
    assert "t3_background_wait" not in codex_next
    assert "T3 will wake you" in codex_next
    assert "start the watch command again" in codex_next
    for outcome in ("ready", "expired", "already_watching", "all_done", "service_gone"):
        assert outcome in codex_next

    codex_again = wait_guidance.build_wait_guidance(
        codex,
        root=tmp_path,
        pid=42,
        started_at=1700000000.5,
        boss_key="s-boss",
        again=True,
        can_push=_always_push,
        watch_supported=_watch_none,
        t3_main=True,
    )
    assert codex_again.startswith("Still pending:")
    assert "background terminal" in codex_again

    # Without boss_key or when t3_main=False, existing host guidance is unchanged.
    assert (
        wait_guidance.build_wait_guidance(
            codex,
            root=tmp_path,
            pid=42,
            started_at=1700000000.5,
            boss_key=None,
            can_push=_never_push,
            t3_main=True,
        )
        == wait_guidance.CODEX_WAIT_NEXT
    )
    assert (
        wait_guidance.build_wait_guidance(
            claude,
            root=tmp_path,
            pid=42,
            started_at=1700000000.5,
            boss_key="s-boss",
            can_push=_always_push,
            t3_main=False,
        )
        == wait_guidance.CLAUDE_LONG_WAIT_NEXT
    )
    # T3 main thread + Antigravity: run_command with a short WaitMsBeforeAsync, self-wakes, no
    # t3_background_wait; an unsupported agy falls back to the ordinary blocking wait.
    agy = Host("antigravity", "env")
    agy_kwargs: dict[str, Any] = {
        "root": tmp_path,
        "pid": 42,
        "started_at": 1700000000.5,
        "boss_key": "s-boss",
        "can_push": _never_push,
        "watch_supported": _watch_none,
        "t3_main": True,
    }
    agy_next = wait_guidance.build_wait_guidance(
        agy, t3_watch_supported=lambda name: name == "antigravity", **agy_kwargs
    )
    assert "-I -m agentnave.inbox_watch" in agy_next
    assert "42 1700000000.500000 s-boss" in agy_next
    assert "run_command" in agy_next
    assert "WaitMsBeforeAsync=500" in agy_next
    assert "do not call a blocking wait, poll, or call t3-code's t3_background_wait" in agy_next
    assert "Antigravity notifies you" in agy_next
    assert "start the watch command again" in agy_next
    assert agy_next not in wait_guidance.BLOCKING_WAIT_AGAIN
    agy_again = wait_guidance.build_wait_guidance(
        agy, again=True, t3_watch_supported=lambda _name: True, **agy_kwargs
    )
    assert agy_again.startswith("Still pending:")
    assert "WaitMsBeforeAsync=500" in agy_again
    assert (
        wait_guidance.build_wait_guidance(agy, t3_watch_supported=lambda _name: False, **agy_kwargs)
        == wait_guidance.WAIT_NEXT
    )

    # Hosts without a T3 wait path are unchanged even with t3_main=True.
    assert (
        wait_guidance.build_wait_guidance(
            Host("pi", "env"),
            root=tmp_path,
            pid=42,
            started_at=1700000000.5,
            boss_key="s-boss",
            t3_main=True,
        )
        == wait_guidance.WAIT_NEXT
    )

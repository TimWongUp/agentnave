"""Identify the calling host without launching commands or inspecting prompt arguments."""

from __future__ import annotations

import os
import re
import sys
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import get_args

from agentnave.models import Host as Host
from agentnave.models import HostName as HostName
from agentnave.models import HostSource as HostSource
from agentnave.models import RetiredHostName as RetiredHostName
from agentnave.process_info import read_process_argv, read_process_parent

# Hosts that start each chat's MCP server with that chat's native session ID in its environment.
# Codex passes MCP servers only a fixed variable allowlist, so its thread ID never arrives here.
NATIVE_SESSION_ENV = {"claude": "CLAUDE_CODE_SESSION_ID"}


def host_session_id(host: Host) -> str | None:
    """The boss chat this server runs for when the caller names none.

    A per-chat AGENTNAVE_HOST_SESSION_ID wins; otherwise the host's native session ID, which
    also lets session_model find the boss model.
    """
    injected = os.environ.get("AGENTNAVE_HOST_SESSION_ID")
    if injected is not None:
        return injected
    name = NATIVE_SESSION_ENV.get(host.name or "")
    if name is None:
        return None
    return os.environ.get(name, "").strip() or None


def _match(executable: str) -> HostName | None:
    path = Path(executable)
    parts = [part.lower() for part in path.parts]
    if any(parts[i : i + 2] == ["claude", "versions"] for i in range(len(parts) - 1)):
        return "claude"
    apps: dict[str, HostName] = {
        "codex.app": "codex",
        "claude.app": "claude",
        "workbuddy.app": "workbuddy",
        "antigravity.app": "antigravity",
        "deepseek harness.app": "deepseek",
        "zcode.app": "zcode",
        "trae cn.app": "trae",
        "trae.app": "trae",
    }
    for part in path.parts:
        if part.lower() in apps:
            return apps[part.lower()]
    name = path.name.lower()
    if path.suffix.lower() in {".js", ".mjs", ".cjs", ".py"}:
        name = path.stem.lower()
    if name in {"traecli", "traecli.exe", "traex", "traex.exe"}:
        return "trae"
    if name == "agy":
        return "antigravity"
    if name == "dsh":
        return "deepseek"
    for host in get_args(HostName.__value__):
        if name == host and host not in {"workbuddy", "antigravity"}:
            return host
    return None


def _is_wrapper(executable: str) -> bool:
    name = Path(executable).name.lower()
    return (
        name
        in {
            "node",
            "bun",
            "deno",
            "uv",
            "uvx",
            "npx",
            "env",
            "sh",
            "bash",
            "zsh",
            "fish",
            "dash",
        }
        or re.fullmatch(r"python(?:3(?:\.\d+)?)?", name) is not None
    )


def _detect_parent(
    parent_pid: int,
    read_parent: Callable[[int], int],
    read_argv: Callable[[int], Sequence[str]],
) -> Host:
    seen: set[int] = set()
    pid = parent_pid
    for _ in range(32):
        if pid <= 1 or pid in seen:
            break
        seen.add(pid)
        try:
            argv = read_argv(pid)
        except (OSError, ValueError):
            argv = ()  # Parent identity may still be readable; continue up the chain.
        if argv:
            if (host := _match(argv[0])) is not None:
                return Host(host, "parent")
            if not _is_wrapper(argv[0]):
                return Host()
            script = next((arg for arg in argv[1:] if not arg.startswith("-")), None)
            if script is not None and (host := _match(script)) is not None:
                return Host(host, "parent")
        try:
            pid = read_parent(pid)
        except (OSError, ValueError):
            break
    return Host()


def detect_host() -> Host:
    app_id = os.environ.get("AGENTNAVE_HOST_APP", "").strip() or None
    if "AGENTNAVE_HOST" in os.environ:
        value = os.environ["AGENTNAVE_HOST"]
        for name in get_args(HostName.__value__):
            if value == name:
                return Host(name, "env", app_id)
        raise ValueError(f"Unknown AGENTNAVE_HOST: {value!r}")
    if sys.platform == "win32":
        return Host(app_id=app_id)
    host = _detect_parent(os.getppid(), read_process_parent, read_process_argv)
    return Host(host.name, host.source, app_id)

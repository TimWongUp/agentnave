"""Identify the calling host without launching commands or inspecting prompt arguments."""

from __future__ import annotations

import os
import re
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, get_args

from agentnave.nesting import read_process_argv, read_process_parent

type HostName = Literal[
    "codex",
    "claude",
    "codebuddy",
    "workbuddy",
    "grok",
    "antigravity",
    "pi",
    "gemini",
    "opencode",
    "hermes",
]
type HostSource = Literal["env", "parent", "unknown"]


@dataclass(frozen=True)
class Host:
    name: HostName | None = None
    source: HostSource = "unknown"

    @property
    def provider(self) -> str | None:
        if self.name == "workbuddy":
            return "codebuddy"
        if self.name in {"gemini", "opencode", "hermes"}:
            return None
        return self.name


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
    }
    for part in path.parts:
        if part.lower() in apps:
            return apps[part.lower()]
    name = path.name.lower()
    if path.suffix.lower() in {".js", ".mjs", ".cjs", ".py"}:
        name = path.stem.lower()
    if name == "agy":
        return "antigravity"
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
    if "AGENTNAVE_HOST" in os.environ:
        value = os.environ["AGENTNAVE_HOST"]
        for name in get_args(HostName.__value__):
            if value == name:
                return Host(name, "env")
        raise ValueError(f"Unknown AGENTNAVE_HOST: {value!r}")
    if sys.platform == "win32":
        return Host()
    return _detect_parent(os.getppid(), read_process_parent, read_process_argv)

"""Identify the calling host without launching commands or inspecting prompt arguments."""

from __future__ import annotations

import os
import re
import sys
from collections.abc import Callable, Iterator, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Literal, get_args
from uuid import UUID

from agentnave.adapters.base import model_from_event, object_dict, parse_json_object
from agentnave.process_info import read_process_argv, read_process_parent

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


def session_model(
    host: Host, session_id: str | None, *, since: datetime | None = None
) -> str | None:
    """Read this native session's latest model, never a CLI-wide default or other chat."""
    if host.name not in {"codex", "claude"} or session_id is None:
        return None
    try:
        native_id = str(UUID(session_id))
    except ValueError:
        return None
    if native_id != session_id.lower():
        return None
    if host.name == "codex":
        root = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "sessions"
        pattern = f"*/*/*/rollout-*-{native_id}.jsonl"
    else:
        root = Path(os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude"))) / "projects"
        pattern = f"*/{native_id}.jsonl"
    cutoff = datetime.now(UTC)
    marker = b'"turn_context"' if host.name == "codex" else b'"assistant"'
    try:
        paths = list(root.glob(pattern))
        if len(paths) != 1 or not paths[0].resolve().is_relative_to(root.resolve()):
            return None
        # Transcripts append in time order and can hold huge lines (generated images), so read
        # from the end and stop at the newest qualifying turn instead of parsing everything.
        for line in _lines_from_end(paths[0]):
            if marker not in line:
                continue
            event = parse_json_object(line.decode("utf-8", "replace"))
            if event is None:
                continue
            try:
                stamp = datetime.fromisoformat(str(event.get("timestamp", "")))
            except ValueError:
                continue
            if stamp.tzinfo is None or stamp > cutoff:
                continue
            if since is not None and stamp < since:
                return None
            if host.name == "codex" and event.get("type") == "turn_context":
                value = object_dict(event.get("payload")).get("model")
                model = value if isinstance(value, str) and value.strip() else None
            else:
                model = model_from_event("claude", event)
            if (
                model
                and model not in {"unknown", "<synthetic>"}
                and len(model) <= 200
                and model.isprintable()
            ):
                return model.strip()
    except OSError:
        return None
    return None


def _lines_from_end(
    path: Path, *, chunk: int = 1 << 20, longest: int = 1 << 20, budget: int = 256 << 20
) -> Iterator[bytes]:
    """Yield complete lines newest first; lines over `longest` bytes are skipped unread."""
    with path.open("rb") as stream:
        position = stream.seek(0, os.SEEK_END)
        stop = max(0, position - budget)
        tail = b""
        oversized = False
        while position > stop:
            size = min(chunk, position - stop)
            position -= size
            stream.seek(position)
            block = stream.read(size) + tail
            parts = block.split(b"\n")
            tail = parts[0]
            for part in reversed(parts[1:]):
                if oversized:
                    oversized = False  # this part ends the line that was too long
                    continue
                if part and len(part) <= longest:
                    yield part
            if len(tail) > longest:
                tail = b""
                oversized = True
        if tail and not oversized and position == 0 and len(tail) <= longest:
            yield tail


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

"""Identify the calling host without launching commands or inspecting prompt arguments."""

from __future__ import annotations

import os
import re
import sqlite3
import sys
from collections.abc import Callable, Iterator, Sequence
from contextlib import closing
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
    "deepseek",
    "hermes",
]
# Hosts no longer connected or detected; saved history may still name them.
type RetiredHostName = Literal["gemini", "opencode"]
type HostSource = Literal["env", "parent", "unknown"]


@dataclass(frozen=True)
class Host:
    name: HostName | None = None
    source: HostSource = "unknown"

    @property
    def provider(self) -> str | None:
        if self.name == "workbuddy":
            return "codebuddy"
        if self.name == "hermes":
            return None
        return self.name


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


def session_file(host: Host, session_id: str | None) -> Path | None:
    """This native session's own transcript file, when the host keeps one we can find."""
    native_id = _native_id(session_id)
    if native_id is None:
        return None
    try:
        found = _transcript(host, native_id)
    except (OSError, ValueError, RuntimeError):  # as in session_model: an unknown home, a loop
        return None
    return None if found is None else found[0]


def session_model(
    host: Host, session_id: str | None, *, since: datetime | None = None
) -> str | None:
    """Read this native session's latest model, never a CLI-wide default or other chat."""
    if session_id is None:
        return None
    try:
        if host.name == "hermes":
            return _hermes_model(session_id)
        native_id = _native_id(session_id)
        if native_id is None:
            return None
        if host.name == "antigravity":
            return _antigravity_model(native_id)
        return _transcript_model(host, native_id, since)
    except (OSError, sqlite3.Error, ValueError, RuntimeError):
        # A relative HERMES_HOME (no file URI) or a missing home folder reads as unknown.
        return None


def _native_id(session_id: str | None) -> str | None:
    try:
        native_id = str(UUID(session_id or ""))
    except ValueError:
        return None
    return native_id if session_id and native_id == session_id.lower() else None


def _transcript(host: Host, native_id: str) -> tuple[Path, bytes] | None:
    """The session's transcript and the marker of lines that may name its model."""
    if host.name == "codex":
        root = _home("CODEX_HOME", ".codex") / "sessions"
        pattern, marker = f"*/*/*/rollout-*-{native_id}.jsonl", b'"turn_context"'
    elif host.name == "claude":
        root = _home("CLAUDE_CONFIG_DIR", ".claude") / "projects"
        pattern, marker = f"*/{native_id}.jsonl", b'"assistant"'
    elif host.name == "pi":
        root = _home("PI_CODING_AGENT_DIR", ".pi/agent") / "sessions"
        pattern, marker = f"*/*_{native_id}.jsonl", b'"assistant"'
    elif host.name in {"codebuddy", "workbuddy"}:
        # WorkBuddy is CodeBuddy's desktop app and keeps the same records in its own folder.
        default = ".workbuddy" if host.name == "workbuddy" else ".codebuddy"
        root = _home("CODEBUDDY_CONFIG_DIR", default) / "projects"
        pattern, marker = f"*/{native_id}.jsonl", b'"assistant"'
    elif host.name == "grok":
        root = _home("GROK_HOME", ".grok") / "sessions"
        pattern, marker = f"*/{native_id}/chat_history.jsonl", b'"model_id"'
    else:
        return None
    paths = list(root.glob(pattern))
    if len(paths) != 1 or not paths[0].resolve().is_relative_to(root.resolve()):
        return None
    return paths[0], marker


def _transcript_model(host: Host, native_id: str, since: datetime | None) -> str | None:
    found = _transcript(host, native_id)
    if found is None:
        return None
    path, marker = found
    cutoff = datetime.now(UTC)
    # Transcripts append in time order and can hold huge lines (generated images), so read
    # from the end and stop at the newest qualifying turn instead of parsing everything.
    for line in _lines_from_end(path):
        if marker not in line:
            continue
        event = parse_json_object(line.decode("utf-8", "replace"))
        if event is None:
            continue
        # Grok's chat history carries no timestamps; its entries are this session's own turns.
        if host.name != "grok":
            raw = event.get("timestamp")
            try:
                # CodeBuddy stamps lines in epoch milliseconds, the others in ISO 8601.
                if isinstance(raw, int) and not isinstance(raw, bool):
                    stamp = datetime.fromtimestamp(raw / 1000, UTC)
                else:
                    stamp = datetime.fromisoformat(str(raw or ""))
            except (ValueError, OverflowError, OSError):
                continue
            if stamp.tzinfo is None or stamp > cutoff:
                continue
            if since is not None and stamp < since:
                return None
        value: object = None
        if host.name == "codex":
            if event.get("type") == "turn_context":
                value = object_dict(event.get("payload")).get("model")
        elif host.name == "claude":
            value = model_from_event("claude", event)
        elif host.name in {"codebuddy", "workbuddy"}:
            if event.get("role") == "assistant":
                value = object_dict(event.get("providerData")).get("model")
        elif host.name == "pi":
            value = model_from_event("pi", {"type": "message_end", "message": event.get("message")})
        elif event.get("type") == "assistant":
            value = event.get("model_id")
        if (model := _clean_model(value)) is not None:
            return model
    return None


def _antigravity_model(native_id: str) -> str | None:
    # The desktop app and the agy CLI keep conversations apart, one SQLite file each.
    root = Path.home() / ".gemini"
    for folder in ("antigravity", "antigravity-cli"):
        path = root / folder / "conversations" / f"{native_id}.db"
        if not path.is_file():
            continue
        with closing(_read_only(path)) as db:
            # Each generation's metadata is a protobuf whose field 3.28 names the model it used.
            for (data,) in db.execute("SELECT data FROM gen_metadata ORDER BY idx DESC"):
                if not isinstance(data, bytes):
                    continue
                for field, step in _proto_fields(data):
                    if field != 3:
                        continue
                    for inner, value in _proto_fields(step):
                        if inner == 28:
                            return _clean_model(value.decode("utf-8", "replace"))
        return None
    return None


def _hermes_model(session_id: str) -> str | None:
    if not re.fullmatch(r"\d{8}_\d{6}_[0-9a-f]{4,32}", session_id):
        return None
    path = _home("HERMES_HOME", ".hermes") / "state.db"
    if not path.is_file():
        return None
    with closing(_read_only(path)) as db:
        row = db.execute("SELECT model FROM sessions WHERE id = ?", (session_id,)).fetchone()
    return _clean_model(row[0]) if row is not None else None


def _home(variable: str, default: str) -> Path:
    return Path(os.environ.get(variable) or Path.home() / default)


def _read_only(path: Path) -> sqlite3.Connection:
    # Without a -wal file a read-only open can fail; the main file is then complete as it is.
    mode = "mode=ro" if path.with_name(path.name + "-wal").exists() else "immutable=1"
    return sqlite3.connect(f"{path.as_uri()}?{mode}", uri=True, timeout=1)


def _proto_fields(data: bytes) -> Iterator[tuple[int, bytes]]:
    """Length-delimited fields of one protobuf message; stops at the first malformed byte."""
    index = 0
    try:
        while index < len(data):
            key, index = _varint(data, index)
            wire = key & 7
            if wire == 0:
                _, index = _varint(data, index)
            elif wire == 1:
                index += 8
            elif wire == 5:
                index += 4
            elif wire == 2:
                size, index = _varint(data, index)
                if index + size > len(data):
                    return
                yield key >> 3, data[index : index + size]
                index += size
            else:
                return
    except IndexError:
        return


def _varint(data: bytes, index: int) -> tuple[int, int]:
    value = shift = 0
    while True:
        if shift > 63:
            raise IndexError("varint longer than ten bytes")  # malformed: stop parsing
        byte = data[index]
        index += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if byte < 0x80:
            return value, index


def _clean_model(value: object) -> str | None:
    if not isinstance(value, str):
        return None
    model = value.strip()
    if (
        model
        and model not in {"unknown", "<synthetic>"}
        and len(model) <= 200
        and model.isprintable()
    ):
        return model
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
        "deepseek harness.app": "deepseek",
    }
    for part in path.parts:
        if part.lower() in apps:
            return apps[part.lower()]
    name = path.name.lower()
    if path.suffix.lower() in {".js", ".mjs", ".cjs", ".py"}:
        name = path.stem.lower()
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
    if "AGENTNAVE_HOST" in os.environ:
        value = os.environ["AGENTNAVE_HOST"]
        for name in get_args(HostName.__value__):
            if value == name:
                return Host(name, "env")
        raise ValueError(f"Unknown AGENTNAVE_HOST: {value!r}")
    if sys.platform == "win32":
        return Host()
    return _detect_parent(os.getppid(), read_process_parent, read_process_argv)

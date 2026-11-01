"""Identify the calling host without launching commands or inspecting prompt arguments."""

from __future__ import annotations

import hashlib
import json
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

from agentnave.adapters.base import clean_model, object_dict, parse_json_object
from agentnave.adapters.claude import assistant_model
from agentnave.adapters.pi import message_model
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
    "zcode",
]
# Hosts no longer connected or detected; saved history may still name them.
type RetiredHostName = Literal["gemini", "opencode"]
type HostSource = Literal["env", "parent", "unknown"]


@dataclass(frozen=True)
class Host:
    name: HostName | None = None
    source: HostSource = "unknown"
    app_id: str | None = None

    @property
    def provider(self) -> str | None:
        if self.name == "workbuddy":
            return "codebuddy"
        if self.name == "hermes":
            return None
        return self.name


_HERMES_SESSIONS = "sessions"
_ZCODE_SESSIONS = "session"

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


def boss_key(host: str | None, session_id: str) -> str:
    """A boss chat's key on disk: a hash, so its native session ID alone is never stored."""
    identity = json.dumps([host, session_id], ensure_ascii=False).encode()
    return "s-" + hashlib.sha256(identity).hexdigest()[:32]


def session_file(host: Host, session_id: str | None) -> Path | None:
    """Where this native session's records are: its own file, or the database a host shares."""
    if session_id is None:
        return None
    try:
        if host.name == "deepseek":
            return _deepseek_file(session_id)
        if host.name == "hermes":
            return _shared_file(_hermes_database(), _HERMES_SESSIONS, session_id)
        if host.name == "zcode":
            return _shared_file(zcode_database(), _ZCODE_SESSIONS, session_id)
        native_id = _native_id(session_id)
        if native_id is None:
            return None
        if host.name == "antigravity":
            return _antigravity_file(native_id)
        found = _transcript(host, native_id)
    except (OSError, sqlite3.Error, ValueError, RuntimeError):
        # As in session_model: an unknown home, a loop, a database in use.
        return None
    return None if found is None else found[0]


def shared_session_id(host: str | None, key: str) -> str | None:
    """The session behind a boss key, for a host whose sessions share one database."""
    try:
        if host == "hermes":
            database, sessions = _hermes_database(), _HERMES_SESSIONS
        elif host == "zcode":
            database, sessions = zcode_database(), _ZCODE_SESSIONS
        else:
            return None
        if not database.is_file():
            return None
        with closing(_read_only(database)) as db:
            for (session,) in db.execute(f"SELECT id FROM {sessions}"):  # noqa: S608
                if isinstance(session, str) and boss_key(host, session) == key:
                    return session
    except (OSError, sqlite3.Error, ValueError, RuntimeError):
        return None
    return None


def _deepseek_file(session_id: str) -> Path | None:
    if not re.fullmatch(r"session-[0-9a-fA-F-]{36}", session_id):
        return None
    root = _home("DSH_HOME", ".dsh") / "sessions"
    # The file of an older format stays behind when a session is carried over.
    files = [
        path
        for path in root.glob(f"*/{session_id}/session*.jsonl*")
        if path.suffix in (".jsonl", ".zstd") and path.resolve().is_relative_to(root.resolve())
    ]
    return max(files, key=lambda path: path.stat().st_mtime_ns) if files else None


def _antigravity_file(native_id: str) -> Path | None:
    root = Path.home() / ".gemini"
    for folder in ("antigravity", "antigravity-cli"):
        path = root / folder / "brain" / native_id / ".system_generated/logs/transcript_full.jsonl"
        if path.is_file():
            return path
    return None


def _shared_file(database: Path, sessions: str, session_id: str) -> Path | None:
    if not database.is_file():
        return None
    with closing(_read_only(database)) as db:
        found = db.execute(
            f"SELECT 1 FROM {sessions} WHERE id = ?",  # noqa: S608
            (session_id,),
        ).fetchone()
    return database if found else None


def session_model(
    host: Host, session_id: str | None, *, since: datetime | None = None
) -> str | None:
    """Read this native session's latest model, never a CLI-wide default or other chat."""
    if session_id is None:
        return None
    try:
        if host.name == "hermes":
            return _hermes_model(session_id)
        if host.name == "zcode":
            return _zcode_model(session_id, since)
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
    paths = [path for path in root.glob(pattern) if path.resolve().is_relative_to(root.resolve())]
    if not paths:
        return None
    # A chat moved between worktrees leaves a stale copy in the old project's folder.
    return max(paths, key=lambda path: path.stat().st_mtime_ns), marker


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
            value = assistant_model(event)
        elif host.name in {"codebuddy", "workbuddy"}:
            if event.get("role") == "assistant":
                value = object_dict(event.get("providerData")).get("model")
        elif host.name == "pi":
            value = message_model(object_dict(event.get("message")))
        elif event.get("type") == "assistant":
            value = event.get("model_id")
        if (model := clean_model(value)) is not None:
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
                            return clean_model(value.decode("utf-8", "replace"))
        return None
    return None


def _hermes_model(session_id: str) -> str | None:
    if not re.fullmatch(r"\d{8}_\d{6}_[0-9a-f]{4,32}", session_id):
        return None
    path = _hermes_database()
    if not path.is_file():
        return None
    with closing(_read_only(path)) as db:
        row = db.execute("SELECT model FROM sessions WHERE id = ?", (session_id,)).fetchone()
    return clean_model(row[0]) if row is not None else None


def _hermes_database() -> Path:
    return _home("HERMES_HOME", ".hermes") / "state.db"


def _zcode_model(session_id: str, since: datetime | None) -> str | None:
    if not re.fullmatch(r"(?:sess_[0-9a-fA-F-]{36}|session_[0-9a-fA-F]{32})", session_id):
        return None
    path = zcode_database()
    if not path.is_file():
        return None
    with closing(_read_only(path)) as db:
        rows = db.execute(
            "SELECT data FROM message WHERE session_id = ? AND time_created >= ? "
            "ORDER BY time_created DESC, sequence DESC",
            (session_id, int(since.timestamp() * 1000) if since else 0),
        )
        for (raw,) in rows:
            message = parse_json_object(raw) or {}
            if message.get("role") == "assistant":
                return clean_model(message.get("modelId"))
    return None


def zcode_database() -> Path:
    config_path = Path.home() / ".zcode/cli/config.json"
    if config_path.is_file():
        config = parse_json_object(config_path.read_text()) or {}
        override = object_dict(config.get("storage")).get("sessionDbPath")
        if isinstance(override, str) and override:
            return Path(override).expanduser()
    return Path.home() / ".zcode/cli/db/db.sqlite"


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
        "zcode.app": "zcode",
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

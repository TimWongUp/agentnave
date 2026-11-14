"""Read Provider-native sessions and models without depending on host detection or adapters."""

from __future__ import annotations

import hashlib
import json
import os
import re
import sqlite3
import sys
from collections.abc import Callable, Iterator
from contextlib import closing
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import cast
from uuid import UUID

from agentnave.models import Host

_DEVIN_SESSIONS = "sessions"
_HERMES_SESSIONS = "sessions"
_ZCODE_SESSIONS = "session"


def parse_json_object(text: str) -> dict[str, object] | None:
    try:
        raw = cast(object, json.loads(text, parse_int=_parse_json_int))
    except (ValueError, RecursionError):
        return None
    if not isinstance(raw, dict):
        return None
    mapping = cast(dict[object, object], raw)
    return {str(key): value for key, value in mapping.items()}


def _parse_json_int(value: str) -> int | str:
    try:
        return int(value)
    except ValueError:
        return value


def object_dict(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        return {}
    mapping = cast(dict[object, object], value)
    return {str(key): item for key, item in mapping.items()}


def clean_model(value: object) -> str | None:
    """A model name fit to show: printable, bounded, and not a placeholder."""
    # A control character anywhere rejects the value rather than being stripped away.
    if not isinstance(value, str) or len(value) > 200 or not value.isprintable():
        return None
    model = value.strip()
    return model if model and model not in {"unknown", "<synthetic>"} else None


def assistant_model(event: dict[str, object]) -> str | None:
    """The main assistant's model; a subagent's messages carry parent_tool_use_id."""
    if event.get("parent_tool_use_id") is not None:
        return None
    kind = event.get("type")
    if kind == "assistant":
        return clean_model(object_dict(event.get("message")).get("model"))
    if kind == "stream_event":
        nested = object_dict(event.get("event"))
        if nested.get("type") == "message_start":
            return clean_model(object_dict(nested.get("message")).get("model"))
    return None


def message_model(message: dict[str, object]) -> str | None:
    """The model that served an assistant message, not the one requested."""
    if message.get("role") != "assistant":
        return None
    return clean_model(message.get("responseModel") or message.get("model"))


def extract_devin_turn(
    payload: dict[str, object] | None,
) -> tuple[str | None, str, str | None]:
    """Return (session_id, last_agent_message, last_agent_model) from a Devin ATIF export."""
    if not payload:
        return None, "", None
    raw_session = payload.get("session_id")
    session_id = raw_session if isinstance(raw_session, str) and raw_session else None
    steps = payload.get("steps")
    if not isinstance(steps, list):
        return session_id, "", clean_model(object_dict(payload.get("agent")).get("model_name"))
    output = ""
    model: str | None = None
    for item in reversed(cast(list[object], steps)):
        step = object_dict(item)
        if step.get("source") != "agent":
            continue
        raw_message = step.get("message")
        if not output and isinstance(raw_message, str):
            output = raw_message
        if model is None:
            extra = object_dict(step.get("extra"))
            model = clean_model(extra.get("generation_model")) or clean_model(
                step.get("model_name")
            )
        if output and model is not None:
            break
    if model is None:
        model = clean_model(object_dict(payload.get("agent")).get("model_name"))
    return session_id, output, model


def boss_key(host: str | None, session_id: str) -> str:
    """A boss chat's key on disk: a hash, so its native session ID alone is never stored."""
    identity = json.dumps([host, session_id], ensure_ascii=False).encode()
    return "s-" + hashlib.sha256(identity).hexdigest()[:32]


def session_file(host: Host, session_id: str | None) -> Path | None:
    """Where this native session's records are: its own file, or the database a host shares."""
    if session_id is None:
        return None
    try:
        handler = _HOSTS.get(host.name or "")
        return handler.file(session_id) if handler else None
    except (OSError, sqlite3.Error, ValueError, RuntimeError):
        # As in session_model: an unknown home, a loop, a database in use.
        return None


def shared_session_id(host: str | None, key: str) -> str | None:
    """The session behind a boss key, for a host whose sessions share one database."""
    try:
        handler = _HOSTS.get(host or "")
        if handler is None or handler.database is None:
            return None
        database, sessions = handler.database(), handler.table
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
        handler = _HOSTS.get(host.name or "")
        return handler.model(session_id, since) if handler else None
    except (OSError, sqlite3.Error, ValueError, RuntimeError):
        # A relative HERMES_HOME (no file URI) or a missing home folder reads as unknown.
        return None


def _native_id(session_id: str | None) -> str | None:
    try:
        native_id = str(UUID(session_id or ""))
    except ValueError:
        return None
    return native_id if session_id and native_id == session_id.lower() else None


def _codex_rollout_matches(path: Path, native_id: str) -> bool:
    stem = path.stem
    if stem.endswith(f"-{native_id}"):
        return True
    prefix, sep, tail = stem.rpartition(f"-{native_id}_")
    return bool(prefix and sep and _native_id(tail) is not None)


@dataclass(frozen=True)
class _Transcript:
    variable: str
    default: str
    folder: str
    pattern: str
    marker: bytes
    model: Callable[[dict[str, object]], object]
    timestamped: bool = True
    matches: Callable[[Path, str], bool] = lambda _path, _id: True


def _transcript(spec: _Transcript, native_id: str) -> tuple[Path, bytes] | None:
    """The session's transcript and the marker of lines that may name its model."""
    root = _home(spec.variable, spec.default) / spec.folder
    pattern, marker = spec.pattern.format(id=native_id), spec.marker
    paths = [
        path
        for path in root.glob(pattern)
        if path.resolve().is_relative_to(root.resolve()) and spec.matches(path, native_id)
    ]
    if not paths:
        return None
    # A chat moved between worktrees leaves a stale copy in the old project's folder.
    return max(paths, key=lambda path: path.stat().st_mtime_ns), marker


def _transcript_model(spec: _Transcript, native_id: str, since: datetime | None) -> str | None:
    found = _transcript(spec, native_id)
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
        if spec.timestamped:
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
        value = spec.model(event)
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
    # The gateway names its chats by time; the CLI and desktop app use local-<hash>.
    if not re.fullmatch(r"\d{8}_\d{6}_[0-9a-f]{4,32}|local-[0-9a-f]{64}", session_id):
        return None
    path = _hermes_database()
    if not path.is_file():
        return None
    with closing(_read_only(path)) as db:
        # sessions.model changes only when a turn ends, so mid-turn it still names the last
        # turn's model; each API call lands in session_model_usage right away.
        try:
            used = db.execute(
                "SELECT model FROM session_model_usage WHERE session_id = ? AND task = '' "
                "ORDER BY last_seen DESC LIMIT 1",
                (session_id,),
            ).fetchone()
        except sqlite3.OperationalError:  # Older Hermes has no such table.
            used = None
        row = db.execute("SELECT model FROM sessions WHERE id = ?", (session_id,)).fetchone()
    return clean_model(used[0] if used else None) or clean_model(row[0] if row else None)


def _hermes_default() -> Path:
    home = Path.home()
    if sys.platform == "win32":
        local = os.environ.get("LOCALAPPDATA", "").strip()
        installed = (Path(local) if local else home / "AppData/Local") / "hermes"
        if installed.is_dir():
            return installed
    return home / ".hermes"


def _active_hermes_profile(root: Path) -> Path | None:
    parent = root.parent
    if parent == root or parent.name == "profiles":
        return None
    try:
        raw = (root / "active_profile").read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None
    name = raw.strip().lstrip("\ufeff")
    if name == "default" or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", name):
        return None
    profile = root / "profiles" / name
    if profile.is_dir() and not (root / "profiles/.deleted" / name).exists():
        return profile
    return None


def _hermes_database() -> Path:
    raw = os.environ.get("HERMES_HOME", "").strip()
    root = Path(raw) if raw else _hermes_default()
    return (_active_hermes_profile(root) or root) / "state.db"


def _devin_root() -> Path:
    if override := os.environ.get("DEVIN_SESSIONS_DIR"):
        return Path(override).expanduser()
    if os.name == "nt":
        base = Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData/Local")
    else:
        base = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    return base / "devin/cli"


def _devin_database() -> Path:
    return _devin_root() / "sessions.db"


def _devin_file(session_id: str) -> Path | None:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}", session_id):
        return None
    root = _devin_root()
    transcript = root / "transcripts" / f"{session_id}.json"
    if transcript.is_file() and transcript.resolve().is_relative_to(root.resolve()):
        return transcript
    return _shared_file(_devin_database(), _DEVIN_SESSIONS, session_id)


def _devin_model(session_id: str, since: datetime | None) -> str | None:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,127}", session_id):
        return None
    root = _devin_root()
    transcript = root / "transcripts" / f"{session_id}.json"
    if (
        transcript.is_file()
        and transcript.resolve().is_relative_to(root.resolve())
        and (since is None or transcript.stat().st_mtime >= since.timestamp() - 5)
    ):
        payload = parse_json_object(transcript.read_text(encoding="utf-8", errors="replace"))
        if (model := extract_devin_turn(payload)[2]) is not None:
            return model
    database = _devin_database()
    if not database.is_file():
        return None
    with closing(_read_only(database)) as db:
        row = db.execute(
            "SELECT model, last_activity_at FROM sessions WHERE id = ?",
            (session_id,),
        ).fetchone()
    if not row:
        return None
    raw_model, last_activity = row
    if (
        since is not None
        and isinstance(last_activity, int)
        and not isinstance(last_activity, bool)
        and last_activity < int(since.timestamp()) - 5
    ):
        return None
    return clean_model(raw_model)


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
    return Path(os.environ.get(variable, "").strip() or Path.home() / default)


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


@dataclass(frozen=True)
class _HostSessions:
    file: Callable[[str], Path | None]
    model: Callable[[str, datetime | None], str | None]
    database: Callable[[], Path] | None = None
    table: str = ""


def _codex_model(event: dict[str, object]) -> object:
    return (
        object_dict(event.get("payload")).get("model")
        if event.get("type") == "turn_context"
        else None
    )


def _codebuddy_model(event: dict[str, object]) -> object:
    # A turn in progress can already name its model in reasoning and function calls.
    if event.get("role") == "assistant" or event.get("type") in {"reasoning", "function_call"}:
        return object_dict(event.get("providerData")).get("model")
    return None


def _grok_model(event: dict[str, object]) -> object:
    return event.get("model_id") if event.get("type") == "assistant" else None


def _transcript_host(spec: _Transcript) -> _HostSessions:
    def file(session_id: str) -> Path | None:
        native_id = _native_id(session_id)
        found = _transcript(spec, native_id) if native_id is not None else None
        return found[0] if found else None

    def model(session_id: str, since: datetime | None) -> str | None:
        native_id = _native_id(session_id)
        return _transcript_model(spec, native_id, since) if native_id is not None else None

    return _HostSessions(file, model)


def _uuid_file(file: Callable[[str], Path | None], session_id: str) -> Path | None:
    native_id = _native_id(session_id)
    return file(native_id) if native_id is not None else None


def _uuid_model(session_id: str, since: datetime | None) -> str | None:
    native_id = _native_id(session_id)
    return _antigravity_model(native_id) if native_id is not None else None


_HOSTS = {
    "codex": _transcript_host(
        _Transcript(
            "CODEX_HOME",
            ".codex",
            "sessions",
            "*/*/*/rollout-*-{id}*.jsonl",
            b'"turn_context"',
            _codex_model,
            matches=_codex_rollout_matches,
        )
    ),
    "claude": _transcript_host(
        _Transcript(
            "CLAUDE_CONFIG_DIR",
            ".claude",
            "projects",
            "*/{id}.jsonl",
            b'"assistant"',
            assistant_model,
        )
    ),
    "pi": _transcript_host(
        _Transcript(
            "PI_CODING_AGENT_DIR",
            ".pi/agent",
            "sessions",
            "*/*_{id}.jsonl",
            b'"assistant"',
            lambda event: message_model(object_dict(event.get("message"))),
        )
    ),
    "codebuddy": _transcript_host(
        _Transcript(
            "CODEBUDDY_CONFIG_DIR",
            ".codebuddy",
            "projects",
            "*/{id}.jsonl",
            b'"providerData"',
            _codebuddy_model,
        )
    ),
    "workbuddy": _transcript_host(
        _Transcript(
            "WORKBUDDY_CONFIG_DIR",
            ".workbuddy",
            "projects",
            "*/{id}.jsonl",
            b'"providerData"',
            _codebuddy_model,
        )
    ),
    "grok": _transcript_host(
        _Transcript(
            "GROK_HOME",
            ".grok",
            "sessions",
            "*/{id}/chat_history.jsonl",
            b'"model_id"',
            _grok_model,
            timestamped=False,
        )
    ),
    "antigravity": _HostSessions(
        lambda session_id: _uuid_file(_antigravity_file, session_id), _uuid_model
    ),
    "deepseek": _HostSessions(_deepseek_file, lambda _session_id, _since: None),
    "devin": _HostSessions(_devin_file, _devin_model, _devin_database, _DEVIN_SESSIONS),
    "hermes": _HostSessions(
        lambda session_id: _shared_file(_hermes_database(), _HERMES_SESSIONS, session_id),
        lambda session_id, _since: _hermes_model(session_id),
        _hermes_database,
        _HERMES_SESSIONS,
    ),
    "zcode": _HostSessions(
        lambda session_id: _shared_file(zcode_database(), _ZCODE_SESSIONS, session_id),
        _zcode_model,
        zcode_database,
        _ZCODE_SESSIONS,
    ),
}

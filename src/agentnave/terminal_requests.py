"""Private file transport for requests to an ancestor tray (ADR-0109)."""

from __future__ import annotations

import json
import os
import re
import stat
import tempfile
from pathlib import Path
from typing import cast

from agentnave.process_info import read_process_started
from agentnave.push_capability import MAX_PUSH_BYTES, TrayIdentity

_ID = re.compile(r"[A-Za-z0-9_-]{1,64}\Z")


def read_tray(root: Path, tray: TrayIdentity) -> dict[str, object] | None:
    """Read a bounded snapshot only while it belongs to the same live tray process."""
    path = root / "push" / f"{tray.pid}.json"
    try:
        if abs(read_process_started(tray.pid) - tray.started_at) >= 0.01:
            return None
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_PUSH_BYTES:
                return None
            if info.st_mtime + 0.01 < tray.started_at or path.is_symlink():
                return None
            data: object = json.load(stream)
        if abs(read_process_started(tray.pid) - tray.started_at) >= 0.01:
            return None
        if not isinstance(data, dict):
            return None
        value = cast(dict[str, object], data)
        return value if value.get("version") == 1 and value.get("pid") == tray.pid else None
    except (OSError, ValueError, TypeError):
        return None


def receipt(
    snapshot: dict[str, object], request_id: str, invocation_id: str
) -> dict[str, object] | None:
    """Return only a receipt tied to this invocation, never another task's data."""
    entries = snapshot.get("requests")
    if not isinstance(entries, dict):
        return None
    entry = cast(dict[str, object], entries).get(request_id)
    if not isinstance(entry, dict):
        return None
    result = cast(dict[str, object], entry)
    if result.get("invocation_id") != invocation_id:
        return None
    if result.get("state") not in ("accepted", "started", "finished"):
        return None
    return result


def publish_request(root: Path, request_id: str, document: dict[str, object]) -> Path:
    """Publish once: an existing ID must never be replaced or submitted again."""
    if not _ID.fullmatch(request_id) or document.get("request_id") != request_id:
        raise ValueError("invalid request ID")
    directory = root / "spawn-requests"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("spawn-requests is not a private directory")
    if os.name != "nt":
        info = directory.stat()
        if info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError("spawn-requests must be owned by this user and private")
    path = directory / f"{request_id}.json"
    fd, temporary = tempfile.mkstemp(prefix=".request-", dir=directory)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(document, stream, ensure_ascii=False, separators=(",", ":"))
            stream.flush()
            os.fsync(stream.fileno())
        # A hard link is atomic and refuses an existing ID, unlike os.replace.
        os.link(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)
    return path

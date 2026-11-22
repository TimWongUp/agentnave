"""Publish the current process's pending inbox metadata for the App."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import TypedDict


class InboxItem(TypedDict):
    key: str
    kind: str
    role: str | None
    status: str


class BossInbox(TypedDict):
    waiting: bool
    items: list[InboxItem]


class InboxSnapshot:
    """Replace the file only when the externally visible state changes."""

    def __init__(self, root: Path, pid: int) -> None:
        self.path = root / "inbox" / f"{pid}.json"
        self.pid = pid
        self._last: dict[str, object] | None = None

    def publish(
        self, bosses: dict[str, BossInbox], settled_requests: list[dict[str, object]] | None = None
    ) -> None:
        live = {key: value for key, value in bosses.items() if value["waiting"] or value["items"]}
        document: dict[str, object] = {"version": 1, "pid": self.pid, "bosses": live}
        if settled_requests:
            document["settled_requests"] = settled_requests
        if document == self._last:
            return
        if not live and not settled_requests:
            self.path.unlink(missing_ok=True)
            self._last = document
            return
        payload = json.dumps(document, ensure_ascii=False)
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd, temporary = tempfile.mkstemp(prefix=f".{self.pid}.", dir=self.path.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as out:
                out.write(payload)
            os.replace(temporary, self.path)
        finally:
            Path(temporary).unlink(missing_ok=True)
        # Keep a copy, since callers may mutate their lists after publishing.
        self._last = json.loads(json.dumps(document))

    def close(self) -> None:
        self.path.unlink(missing_ok=True)
        self._last = None

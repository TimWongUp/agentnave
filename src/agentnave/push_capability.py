"""Determine whether this server is descended from a push-capable App."""

from __future__ import annotations

import json
import os
import stat
import sys
from pathlib import Path
from typing import cast

from agentnave.process_info import read_process_parent, read_process_started

PUSH_NEXT = (
    "The App will prompt you in the terminal when results are ready; you may end this turn. "
    "When prompted, call inbox wait(wait_seconds=0) with the same manager_session_id."
)
PUSH_AGAIN = (
    "Handle ready prompts and reviews. The App will prompt you again when remaining items are "
    "ready; you may end this turn and then call inbox wait(wait_seconds=0)."
)


def app_can_push(root: Path, provider: str | None) -> bool:
    if sys.platform == "win32" or provider is None:
        return False
    seen: set[int] = set()
    chain: list[tuple[int, int, float]] = []
    pid = os.getppid()
    for _ in range(128):
        if pid <= 1 or pid in seen:
            break
        seen.add(pid)
        try:
            started = read_process_started(pid)
            parent = read_process_parent(pid)
            chain.append((pid, parent, started))
            path = root / "push" / f"{pid}.json"
            # Re-read the entire chain to reject replacement during the probe.
            if _matches(path, pid, provider, started) and all(
                abs(read_process_started(one) - birth) < 0.01 and read_process_parent(one) == up
                for one, up, birth in chain
            ):
                return True
            pid = parent
        except (OSError, ValueError, IndexError, StopIteration):
            break
    return False


def _matches(path: Path, pid: int, provider: str, started: float) -> bool:
    try:
        with path.open("rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > 16384:
                return False
            # A file from a former process must predate the current process's start.
            if started > info.st_mtime + 0.01:
                return False
            value: object = json.load(stream)
        if path.is_symlink() or not isinstance(value, dict):
            return False
        value = cast(dict[str, object], value)
        providers = value.get("providers")
        return (
            value.get("version") == 1
            and value.get("pid") == pid
            and isinstance(providers, list)
            and provider in cast(list[object], providers)
        )
    except (OSError, ValueError, TypeError):
        return False

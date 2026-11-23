"""Read-only inbox watcher for hosts that self-wake on background commands (ADR-0114).

Launched verbatim by a boss via ``<python> -I -m agentnave.inbox_watch <data_root> <pid>
<started_at> <boss_key>``. Imports only the standard library and lightweight helpers so
startup stays fast under ``-I``; never connects over MCP, receives results, or prints bodies.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import signal
import stat
import sys
import time
from pathlib import Path
from typing import Literal, cast

from agentnave.locks import lock_file, release_and_remove
from agentnave.process_info import read_process_started

DEFAULT_TIMEOUT_SECONDS = 7200.0
DEFAULT_POLL_INTERVAL_SECONDS = 0.25
MAX_INBOX_BYTES = 1024 * 1024
# Tolerate small Linux CLOCK_BOOTTIME / wall-clock drift while still rejecting PID reuse.
STARTED_TOLERANCE_SECONDS = 1.0
_SAFE_KEY = re.compile(r"[A-Za-z0-9_-]{1,64}\Z")

type WatchOutcome = Literal[
    "ready",
    "all_done",
    "service_gone",
    "expired",
    "already_watching",
]


def _lock_path(root: Path, boss_key: str) -> Path:
    token = (
        boss_key
        if _SAFE_KEY.fullmatch(boss_key)
        else hashlib.sha256(boss_key.encode("utf-8")).hexdigest()[:32]
    )
    return root / "inbox" / f".watch-{token}.lock"


def _service_alive(pid: int, expected_started: float) -> bool:
    if pid <= 0:
        return False
    try:
        live_started = read_process_started(pid)
    except (OSError, ValueError, IndexError):  # IndexError: truncated /proc/<pid>/stat
        return False
    return abs(live_started - expected_started) < STARTED_TOLERANCE_SECONDS


def _read_snapshot(
    path: Path,
) -> tuple[Literal["ok", "missing", "retry"], dict[str, object] | None]:
    """Re-open by path on every poll so atomic os.replace updates are observed immediately."""
    try:
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
        fd = os.open(path, flags)
    except FileNotFoundError:
        return "missing", None
    except OSError:
        return "retry", None
    try:
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_INBOX_BYTES:
                return "retry", None
            raw: object = json.load(stream)
    except (OSError, ValueError, TypeError):
        return "retry", None
    if not isinstance(raw, dict):
        return "retry", None
    return "ok", cast(dict[str, object], raw)


def _int_field(mapping: dict[str, object], name: str) -> int:
    value = mapping.get(name, 0)
    if isinstance(value, bool) or not isinstance(value, int):
        return 0
    return value


def _evaluate_snapshot(
    data: dict[str, object], pid: int, started_at: float, boss_key: str
) -> Literal["ready", "all_done", "service_gone"] | None:
    if data.get("version") != 1 or data.get("pid") != pid:
        return "service_gone"
    file_started = data.get("started_at")
    if isinstance(file_started, (int, float)) and not isinstance(file_started, bool):
        try:
            drift = abs(float(file_started) - started_at)
        except OverflowError:
            return "service_gone"
        if drift >= STARTED_TOLERANCE_SECONDS:
            return "service_gone"
    bosses = data.get("bosses")
    if not isinstance(bosses, dict):
        return None
    entry = cast(dict[str, object], bosses).get(boss_key)
    if entry is None:
        return "all_done"
    if not isinstance(entry, dict):
        return None
    boss = cast(dict[str, object], entry)
    waiting = boss.get("waiting") is True
    in_flight = _int_field(boss, "in_flight")
    event_seq = _int_field(boss, "event_seq")
    delivered_seq = _int_field(boss, "delivered_seq")
    items = boss.get("items")
    has_items = isinstance(items, list) and len(cast(list[object], items)) > 0
    # Unreceived terminal/attention events take priority over in_flight reaching zero.
    if event_seq > delivered_seq and not waiting:
        return "ready"
    if in_flight <= 0 and event_seq <= delivered_seq and not has_items and not waiting:
        return "all_done"
    return None


def watch(
    root: Path,
    pid: int,
    started_at: float,
    boss_key: str,
    *,
    timeout: float = DEFAULT_TIMEOUT_SECONDS,
    interval: float = DEFAULT_POLL_INTERVAL_SECONDS,
) -> WatchOutcome:
    """Wait until the boss's inbox has a ready event, finishes, expires, or loses its server."""
    lock_path = _lock_path(root, boss_key)
    lock_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if lock_path.is_symlink():
        return "already_watching"
    try:
        lock_handle = lock_file(lock_path, "already_watching")
    except ValueError:
        return "already_watching"

    snapshot_path = root / "inbox" / f"{pid}.json"
    deadline = time.monotonic() + max(0.0, timeout)
    poll = max(0.01, interval)
    try:
        while True:
            if not _service_alive(pid, started_at):
                return "service_gone"
            status, data = _read_snapshot(snapshot_path)
            if status == "missing":
                if not _service_alive(pid, started_at):
                    return "service_gone"
                return "all_done"
            if status == "ok" and data is not None:
                outcome = _evaluate_snapshot(data, pid, started_at, boss_key)
                if outcome is not None:
                    if outcome == "all_done" and not _service_alive(pid, started_at):
                        return "service_gone"
                    return outcome
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return "expired"
            time.sleep(min(poll, remaining))
    finally:
        release_and_remove(lock_handle, lock_path)


def _terminate(signum: int, _frame: object) -> None:
    raise SystemExit(128 + signum)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -I -m agentnave.inbox_watch")
    parser.add_argument("data_root", type=Path)
    parser.add_argument("pid", type=int)
    parser.add_argument("started_at", type=float)
    parser.add_argument("boss_key")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--interval", type=float, default=DEFAULT_POLL_INTERVAL_SECONDS)
    args = parser.parse_args(argv)
    if hasattr(signal, "SIGTERM"):
        signal.signal(signal.SIGTERM, _terminate)
    outcome = watch(
        args.data_root,
        args.pid,
        args.started_at,
        args.boss_key,
        timeout=args.timeout,
        interval=args.interval,
    )
    sys.stdout.write(f"{outcome}\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

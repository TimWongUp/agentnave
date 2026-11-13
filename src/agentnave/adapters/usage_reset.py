"""Reset times from a CLI's own usage query, for quota failures that name none (ADR-0081).

Each query runs only the CLI's own command, never reads its credentials, and gives up quietly: a
timeout, an error or an output this module cannot read all mean no reset time.
"""

from __future__ import annotations

import json
import queue
import re
import subprocess
import threading
from collections.abc import Mapping
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

from agentnave.native_sessions import object_dict

QUERY_SECONDS = 30.0
_MAX_OUTPUT = 1 << 20


def run_text(argv: list[str], cwd: Path) -> str | None:
    """Stdout of a usage command that exits cleanly, or None."""
    try:
        done = subprocess.run(
            argv,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=QUERY_SECONDS,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if done.returncode != 0 or len(done.stdout) > _MAX_OUTPUT:
        return None
    return done.stdout.decode(errors="replace")


def _instant(moment: datetime | None, now: datetime) -> datetime | None:
    return moment if moment is not None and moment > now else None


# Antigravity: `agy -p /usage` prints one tab-separated row per limit window:
# group, window, remaining percent (or "disabled"), reset instant in RFC 3339.


def antigravity_reset(text: str, model: str | None, now: datetime) -> datetime | None:
    groups: dict[str, list[tuple[bool, datetime | None]]] = {}
    for line in text.splitlines():
        cells = [cell.strip() for cell in line.split("\t")]
        if len(cells) < 3 or not (remaining := re.fullmatch(r"(\d+(?:\.\d+)?)%", cells[2])):
            continue
        try:
            stamp = datetime.fromisoformat(cells[3]) if len(cells) > 3 else None
        except ValueError:
            stamp = None
        groups.setdefault(cells[0], []).append((float(remaining[1]) <= 0, stamp))
    resets: dict[str, datetime | None] = {}
    for name, windows in groups.items():
        spent = [stamp for exhausted, stamp in windows if exhausted]
        if spent:
            # A group is usable again only once every window that ran out has reset.
            resets[name] = None if None in spent else max(s for s in spent if s is not None)
    family = (model or "").lower().split("-")[0]
    named = [name for name in groups if family and family in name.lower()]
    if named:
        return _instant(resets.get(named[0]), now) if len(named) == 1 else None
    # No group can be tied to the model, so only a CLI whose every group ran out is blocked.
    if not groups or set(resets) != set(groups) or None in resets.values():
        return None
    return _instant(min(r for r in resets.values() if r is not None), now)


# Codex: the app-server answers `account/rateLimits/read` with used percentages and Unix-second
# reset times for its primary and secondary windows.


def query_codex(executable: str, cwd: Path) -> Mapping[str, Any] | None:
    try:
        process = subprocess.Popen(
            [executable, "app-server"],
            cwd=cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
    except OSError:
        return None
    lines: queue.Queue[bytes] = queue.Queue()

    def read() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            lines.put(line)
        lines.put(b"")

    threading.Thread(target=read, daemon=True).start()

    def call(ident: int, method: str, params: dict[str, Any] | None = None) -> Any:
        assert process.stdin is not None
        message: dict[str, Any] = {"method": method, "id": ident}
        if params is not None:
            message["params"] = params
        process.stdin.write(json.dumps(message).encode() + b"\n")
        process.stdin.flush()
        while True:
            line = lines.get(timeout=QUERY_SECONDS)
            if not line:
                return None
            try:
                reply = json.loads(line)
            except ValueError:
                continue
            fields = object_dict(reply)
            if fields.get("id") == ident:
                return fields.get("result")

    try:
        client = {"clientInfo": {"name": "agentnave", "title": "AgentNave", "version": "0"}}
        if call(1, "initialize", client) is None:
            return None
        assert process.stdin is not None
        process.stdin.write(b'{"method":"initialized"}\n')
        process.stdin.flush()
        result = call(2, "account/rateLimits/read")
    except (OSError, queue.Empty, ValueError):
        return None
    finally:
        process.kill()
        process.wait()
    return object_dict(result) or None


def codex_reset(result: Mapping[str, Any], now: datetime) -> datetime | None:
    limits = object_dict(result.get("rateLimits"))
    resets: list[datetime] = []
    for window in ("primary", "secondary"):
        item = object_dict(limits.get(window))
        used, at = item.get("usedPercent"), item.get("resetsAt")
        if not isinstance(used, int | float) or used < 100:
            continue
        if not isinstance(at, int) or isinstance(at, bool):
            return None
        try:
            resets.append(datetime.fromtimestamp(at, UTC))
        except (OverflowError, OSError, ValueError):
            return None
    return _instant(max(resets), now) if resets else None


# Claude: `claude -p /usage` prints "Current session: 92% used · resets Nov 13 at 6:20pm (TZ)".
# The date has no year and the time zone is the user's own, so it is read as local time.

_CLAUDE_ROW = re.compile(
    r"^\s*(Current [^:\n]+?):\s*(\d+(?:\.\d+)?)%\s*used\s*[·•|-]\s*resets\s+([^\n(]+?)"
    r"(?:\s*\([^)\n]*\))?\s*$",
    re.MULTILINE,
)
_CLAUDE_WEEK_MODEL = re.compile(r"(?i)current week \((?!all models\))([^)]+)\)")
_CLAUDE_TIME = re.compile(
    r"(?i)(?:(?P<month>[a-z]{3})[a-z]*\.?\s+(?P<day>\d{1,2})(?:,?\s*(?P<year>\d{4}))?\s*(?:at|,)?\s*)?"
    r"(?P<hour>\d{1,2})(?::(?P<minute>\d\d))?\s*(?P<half>[ap])m"
)
_MONTHS = ("jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec")


def _claude_time(text: str, now: datetime) -> datetime | None:
    if not (match := _CLAUDE_TIME.fullmatch(text.strip())):
        return None
    hour = int(match["hour"]) % 12 + (12 if match["half"].lower() == "p" else 0)
    minute = int(match["minute"] or 0)
    local = now.astimezone()
    try:
        if match["month"] is None:
            moment = local.replace(hour=hour, minute=minute, second=0, microsecond=0)
            if moment <= local:
                moment += timedelta(days=1)
            return moment.astimezone(UTC)
        month = _MONTHS.index(match["month"].lower()) + 1
        year = int(match["year"] or local.year)
        # Naive local time: the system zone settles daylight saving.
        moment = datetime(year, month, int(match["day"]), hour, minute).astimezone()
        if match["year"] is None and moment < local - timedelta(days=1):
            moment = datetime(year + 1, month, int(match["day"]), hour, minute).astimezone()
    except ValueError:
        return None
    return moment.astimezone(UTC)


def claude_reset(text: str, model: str | None, now: datetime) -> datetime | None:
    resets: list[datetime] = []
    for label, used, when in _CLAUDE_ROW.findall(text):
        if float(used) < 100:
            continue
        specific = _CLAUDE_WEEK_MODEL.fullmatch(label.strip())
        # A per-model weekly limit blocks only that model, which a CLI-wide run cannot name.
        if specific and (not model or specific[1].strip().lower() not in model.lower()):
            continue
        if (moment := _claude_time(when, now)) is None:
            return None
        resets.append(moment)
    return _instant(max(resets), now) if resets else None

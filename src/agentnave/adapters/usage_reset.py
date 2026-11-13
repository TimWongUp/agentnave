"""Reset times read from the output of a CLI's own usage query (ADR-0081).

The service runs the query under its process supervisor (`usage_probe`); this module only knows
the commands' output. An output it cannot read means no reset time.
"""

from __future__ import annotations

import json
import re
from collections.abc import Mapping
from datetime import UTC, datetime, timedelta
from typing import Any

from agentnave.native_sessions import object_dict


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


# The handshake and the question go out together; the answer is the line replying to id 2.
CODEX_REQUESTS = b"".join(
    json.dumps(message).encode() + b"\n"
    for message in (
        {
            "method": "initialize",
            "id": 1,
            "params": {"clientInfo": {"name": "agentnave", "title": "AgentNave", "version": "0"}},
        },
        {"method": "initialized"},
        {"method": "account/rateLimits/read", "id": 2},
    )
)


def _codex_answer(stdout: bytes) -> dict[str, object] | None:
    for line in stdout.splitlines():
        try:
            fields = object_dict(json.loads(line))
        except ValueError:
            continue
        if fields.get("id") == 2:
            return object_dict(fields.get("result"))
    return None


def codex_answered(stdout: bytes) -> bool:
    return _codex_answer(stdout) is not None


def codex_output_reset(stdout: bytes, now: datetime) -> datetime | None:
    answer = _codex_answer(stdout)
    return None if answer is None else codex_reset(answer, now)


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
            # Naive local time again: a fixed offset would be wrong across a daylight saving change.
            today = datetime(local.year, local.month, local.day, hour, minute)
            moment = today.astimezone()
            if moment <= local:
                moment = (today + timedelta(days=1)).astimezone()
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

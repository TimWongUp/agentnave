"""Parse explicit reset hints from a provider's own error payload."""

from __future__ import annotations

import re
from datetime import UTC, datetime, timedelta
from email.utils import parsedate_to_datetime

_EPOCH = re.compile(
    r'(?i)(?:usage limit reached\s*\||["\']?resets_at["\']?\s*[:=]\s*["\']?)(\d{10})(?![\d.])'
)
_ABSOLUTE = re.compile(
    r'(?i)(?:resets?\s+at|try again at|["\']?resets_at["\']?\s*[:=])\s*["\']?(\d{4}-\d\d-\d\d[T ]\d\d:\d\d(?::\d\d(?:\.\d+)?)?(?:Z|[+-]\d\d:\d\d))'
)
_RELATIVE = re.compile(
    r"(?i)(?:try again in|resets? in)\s+(\d+(?:\.\d+)?)\s*(seconds?|minutes?|hours?|days?)\b"
)
_RETRY = re.compile(
    r"""(?i)\bretry-after["']?\s*[:=]\s*["']?((?:Mon|Tue|Wed|Thu|Fri|Sat|Sun),\s*\d{1,2}\s+[A-Za-z]{3}\s+\d{4}\s+\d{2}:\d{2}:\d{2}\s+GMT|\d+)(?![\d.])"""
)


def parse_reset(text: str, now: datetime) -> datetime | None:
    text = text.replace('\\"', '"')
    if match := _EPOCH.search(text):
        try:
            return datetime.fromtimestamp(int(match[1]), UTC)
        except (OverflowError, OSError, ValueError):
            return None
    if match := _ABSOLUTE.search(text):
        try:
            return datetime.fromisoformat(match[1])
        except ValueError:
            return None
    if match := _RELATIVE.search(text):
        scale = {"second": 1, "minute": 60, "hour": 3600, "day": 86400}[
            match[2].lower().rstrip("s")
        ]
        try:
            return now + timedelta(seconds=float(match[1]) * scale)
        except OverflowError:
            return None
    if match := _RETRY.search(text):
        value = match[1].strip().strip("\"' ,}")
        try:
            if value.isdecimal():
                return now + timedelta(seconds=int(value))
            result = parsedate_to_datetime(value)
            return result if result.tzinfo else None
        except (OverflowError, TypeError, ValueError):
            return None
    return None

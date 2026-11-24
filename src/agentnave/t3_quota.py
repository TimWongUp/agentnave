"""Which T3 Code usage windows are spent for one model, as T3 itself judges them (ADR-0121).

Ported from t3code `exhaustedUsageWindow` (packages/shared/src/usageLimits.ts) and
`windowLimitsModel` (apps/server/src/provider/claudeUsageLimits.ts),
https://github.com/pingdotgg/t3code.

MIT License

Copyright (c) 2026 T3 Tools Inc

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute,
sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT
OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
"""

from __future__ import annotations

import math
import re
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import cast

_CLAUDE_SCOPED = "seven_day_"
# Paid overage past the plan windows; spending it up does not stop the plan's own allowance.
_CLAUDE_OVERAGE = "monthly_spend"
_CURSOR_AUTO = re.compile(r"^(auto|default|grok|composer)")
_ANTIGRAVITY_FAMILY = re.compile(r"^(gemini|claude|gpt)-")
_ANTIGRAVITY_POOL = re.compile(r"^(gemini|claude|gpt|3p)-")
# Gemini groups metered on their own, apart from the default Gemini pool.
_ANTIGRAVITY_OWN_GROUPS = ("argon", "carbon")


@dataclass(frozen=True)
class UsageWindow:
    """One usage window that is full and has not reset yet."""

    id: str
    resets_at: datetime | None


def _parse_reset_time(raw: object) -> datetime | None:
    if not isinstance(raw, str) or not raw.strip():
        return None
    try:
        parsed = datetime.fromisoformat(raw.strip())
    except ValueError:
        return None
    return None if parsed.tzinfo is None else parsed.astimezone(UTC)


def _names(raw: object) -> list[str] | None:
    if not isinstance(raw, list):
        return None
    return [item for item in cast(list[object], raw) if isinstance(item, str)]


def _windows(limits: object) -> list[dict[object, object]]:
    if not isinstance(limits, dict):
        return []
    raw = cast(dict[object, object], limits).get("windows")
    if not isinstance(raw, list):
        return []
    entries = [
        cast(dict[object, object], item)
        for item in cast(list[object], raw)
        if isinstance(item, dict)
    ]
    return [entry for entry in entries if isinstance(entry.get("id"), str)]


def _full(entry: dict[object, object], model: str, now: datetime) -> UsageWindow | None:
    used = entry.get("usedPercent")
    if not isinstance(used, (int, float)) or isinstance(used, bool):
        return None
    if not math.isfinite(used) or used < 100:
        return None
    model_ids = _names(entry.get("modelIds"))
    if model_ids is not None and model not in model_ids:
        return None
    provider_ids = _names(entry.get("providerIds"))
    if provider_ids is not None and model.split("/")[0] not in provider_ids:
        return None
    resets_at = _parse_reset_time(entry.get("resetsAt"))
    # Like T3: a full window without a readable reset time stays full until T3 says otherwise.
    if resets_at is not None and resets_at <= now:
        return None
    return UsageWindow(id=cast(str, entry["id"]), resets_at=resets_at)


def _token(name: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", name.lower())


def _claude_limits(window_id: str, model: str) -> bool:
    if window_id == _CLAUDE_OVERAGE:
        return False
    if not window_id.startswith(_CLAUDE_SCOPED):
        return True
    return window_id.removeprefix(_CLAUDE_SCOPED) in _token(model)


def _antigravity_limits(window_id: str, model: str) -> bool:
    for group in _ANTIGRAVITY_OWN_GROUPS:
        if window_id.startswith(f"gemini-{group}-"):
            return f"-{group}" in model
        if window_id.startswith("gemini-") and f"-{group}" in model:
            return False
    pool = _ANTIGRAVITY_POOL.match(window_id)
    family = _ANTIGRAVITY_FAMILY.match(model)
    if pool is None or family is None or pool[1] == family[1]:
        return True
    return pool[1] == "3p" and family[1] in ("claude", "gpt")


def exhausted_windows(
    driver: object, limits: object, model: str, now: datetime
) -> list[UsageWindow]:
    """Full windows that stop `model` on this account; empty when it can run or is unknown."""
    entries = _windows(limits)
    full = [window for entry in entries if (window := _full(entry, model, now)) is not None]
    match driver:
        case "claudeAgent":
            return [window for window in full if _claude_limits(window.id, model)]
        case "antigravity":
            return [window for window in full if _antigravity_limits(window.id, model)]
        case "codex":
            # Seat-based plans spend credits only past their plan windows.
            has_plan = any(entry["id"] in ("primary", "secondary") for entry in entries)
            return [window for window in full if window.id != "individual" or not has_plan]
        case "cursor":
            api = [window for window in full if window.id == "apiPercentUsed"]
            auto = [window for window in full if window.id == "autoPercentUsed"]
            return (auto and api) if _CURSOR_AUTO.match(model) else api
        case "opencode":
            own = model.startswith("opencode-go/")
            return [window for window in full if own or not window.id.startswith("go_")]
        case _:
            return full

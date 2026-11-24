"""Read-only probe of T3 Code per-instance cache files for candidate filtering (ADR-0121).

Privacy boundary: only `driver`, `enabled`, `installed`, `status`, `auth.status`, and
`usageLimits.windows[].{id, usedPercent, resetsAt, modelIds, providerIds}` are inspected. Other `auth` fields (such as
emails or account metadata) are never read, stored, logged, or formatted.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import cast

from agentnave.t3 import t3_home
from agentnave.t3_quota import exhausted_windows

_MAX_CACHE_BYTES = 4 * 1024 * 1024
_EXCLUDED_STATUSES = frozenset({"error", "disabled"})


@dataclass(frozen=True)
class InstanceAvailability:
    """Availability verdict from caches/<instance>.json; unknown/unreadable stays available."""

    available: bool
    reason: str | None = None
    resets_at: datetime | None = None


_UNKNOWN = InstanceAvailability(available=True)


def _safe_cache_path(home: Path, instance: str) -> Path | None:
    if (
        not instance
        or len(instance) > 200
        or instance in (".", "..")
        or "/" in instance
        or "\\" in instance
        or "\x00" in instance
        or any(ord(ch) < 32 or ord(ch) == 127 for ch in instance)
        or Path(instance).name != instance
    ):
        return None
    caches_dir = home / "caches"
    candidate = caches_dir / f"{instance}.json"
    if candidate.parent != caches_dir:
        return None
    try:
        resolved_dir = caches_dir.resolve()
        resolved_file = candidate.resolve()
    except OSError:
        return None
    if resolved_file.parent != resolved_dir:
        return None
    return resolved_file


def _quota_block(
    raw: dict[object, object], model: str, now: datetime
) -> InstanceAvailability | None:
    spent = exhausted_windows(raw.get("driver"), raw.get("usageLimits"), model, now)
    if not spent:
        return None
    names = ", ".join(sorted({window.id for window in spent}))
    resets = [window.resets_at for window in spent if window.resets_at is not None]
    return InstanceAvailability(
        available=False,
        reason=f"quota exhausted ({names})",
        # Every full window must reset first; one without a reset time leaves it unknown.
        resets_at=max(resets) if len(resets) == len(spent) else None,
    )


def format_utc(moment: datetime) -> str:
    """Format a UTC datetime as RFC 3339 with Z suffix."""
    return moment.astimezone(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def check_instance(
    instance: str,
    model: str,
    *,
    home: Path | None = None,
    now: datetime | None = None,
) -> InstanceAvailability:
    """Check whether a T3 instance should be excluded for `model` based on its cache file.

    Missing files, invalid JSON, or unexpected field types are treated as unknown (not excluded)
    so T3 remains the final judge.
    """
    path = _safe_cache_path(t3_home() if home is None else home, instance)
    if path is None:
        return _UNKNOWN
    try:
        if not path.is_file() or path.stat().st_size > _MAX_CACHE_BYTES:
            return _UNKNOWN
        loaded = cast(object, json.loads(path.read_text(encoding="utf-8")))
    except (OSError, ValueError, RecursionError):
        return _UNKNOWN
    if not isinstance(loaded, dict):
        return _UNKNOWN
    data = cast(dict[object, object], loaded)
    if data.get("enabled") is False:
        return InstanceAvailability(available=False, reason="instance is disabled")
    if data.get("installed") is False:
        return InstanceAvailability(available=False, reason="provider is not installed")
    status = data.get("status")
    if isinstance(status, str) and status in _EXCLUDED_STATUSES:
        return InstanceAvailability(available=False, reason=f"instance status is {status}")
    auth = data.get("auth")
    if isinstance(auth, dict):
        auth_status = cast(dict[object, object], auth).get("status")
        if isinstance(auth_status, str) and auth_status == "unauthenticated":
            return InstanceAvailability(available=False, reason="instance is unauthenticated")
    quota = _quota_block(data, model, datetime.now(UTC) if now is None else now.astimezone(UTC))
    if quota is not None:
        return quota
    return _UNKNOWN

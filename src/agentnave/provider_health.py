"""Best-effort, CLI-level circuit breaker for recent account blockers."""

from __future__ import annotations

import json
from collections.abc import Iterable
from contextlib import suppress
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, Literal, TypedDict, cast

from agentnave.locks import lock_file, release_and_remove
from agentnave.models import InvocationResult, InvocationStatus
from agentnave.store import write_file

type Code = Literal["auth", "billing", "quota"]


class Health(TypedDict):
    code: Code
    until: str


_TTL = timedelta(minutes=15)
_PRIORITY = {"quota": 0, "billing": 1, "auth": 2}
_CODES: dict[str, Code] = {
    "authentication_failed": "auth",
    "invalid_api_key": "auth",
    "billing_error": "billing",
    "insufficient_quota": "quota",
}
_TEXT: dict[Code, tuple[str, ...]] = {
    "auth": (
        "not logged in",
        "authentication failed",
        "401 unauthorized",
        "invalid api key",
        "no api key",
        "login required",
        "please use /login",
        "sign in to your account",
    ),
    "billing": (
        "billing",
        "payment required",
        "credit card",
        "insufficient funds",
        "insufficient balance",
    ),
    "quota": (
        "usage limit",
        "quota",
        "limit reached",
        "limit exceeded",
        "weekly limit",
        "5-hour limit",
        "monthly limit",
        "out of credits",
        "you've hit your limit",
    ),
}


def _root(root: Path | None) -> Path:
    # members also reads health; defer its data-root helper to avoid an import cycle.
    from agentnave.members import data_root

    return root if root is not None else data_root()


def _read(path: Path) -> dict[str, Any] | None:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
        return cast(dict[str, Any], document) if isinstance(document, dict) else None
    except (OSError, ValueError):
        return None


def _active(document: dict[str, Any] | None, now: datetime) -> dict[str, Health]:
    if document is None or type(document.get("version")) is not int:
        return {}
    if document["version"] != 1 or not isinstance(document.get("providers"), dict):
        return {}
    result: dict[str, Health] = {}
    for provider, item in cast(dict[str, Any], document["providers"]).items():
        if not provider or not isinstance(item, dict):
            continue
        entry = cast(dict[str, Any], item)
        code, until = entry.get("code"), entry.get("until")
        if not isinstance(code, str) or code not in _PRIORITY or not isinstance(until, str):
            continue
        try:
            end = datetime.fromisoformat(until)
            if end.tzinfo is None or not now < end <= now + timedelta(days=1):
                continue
        except ValueError:
            continue
        result[provider] = {"code": cast(Code, code), "until": until}
    return result


def active(root: Path | None = None) -> dict[str, Health]:
    return _active(_read(_root(root) / "provider-health.json"), datetime.now(UTC))


def reason(provider: str, health: Health) -> str:
    return (
        f"{provider} CLI recently reported {health['code']}; cooling down until {health['until']}. "
        "See list_members for the seat replacement or use another available CLI."
    )


def check(root: Path, providers: Iterable[str]) -> None:
    health = active(root)
    for provider in providers:
        if provider in health:
            raise ValueError(reason(provider, health[provider]))


def record(root: Path | None, provider: str, code: Code) -> None:
    _update(_root(root), provider, code)


def clear(root: Path | None, provider: str) -> None:
    _update(_root(root), provider, None)


def _update(root: Path, provider: str, code: Code | None) -> None:
    path = root / "provider-health.json"
    lock_path = root / "locks" / "provider-health.lock"
    try:
        lock_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        handle = lock_file(lock_path, "Provider health is busy")
    except (OSError, ValueError):
        return
    try:
        document = _read(path)
        # Preserve files written by a newer server, even when this server cannot read them.
        if document is not None and (
            type(document.get("version")) is not int or document["version"] != 1
        ):
            return
        now = datetime.now(UTC)
        providers = _active(document, now)
        before = document.get("providers") if document is not None else None
        if code is None:
            providers.pop(provider, None)
            if (not providers and before is None) or providers == before:
                return
        elif provider in providers:
            previous = providers[provider]
            if _PRIORITY[code] > _PRIORITY[previous["code"]]:
                previous["code"] = code
        else:
            providers[provider] = {
                "code": code,
                "until": (now + _TTL).isoformat().replace("+00:00", "Z"),
            }
        write_file(path, json.dumps({"version": 1, "providers": providers}).encode())
    except OSError:
        pass
    finally:
        with suppress(OSError):
            release_and_remove(handle, lock_path)


def classify(
    status: InvocationStatus,
    error_code: str | None,
    error_message: str | None,
    reported_errors: Iterable[str],
) -> Code | None:
    if status is InvocationStatus.SUCCEEDED:
        return None
    codes = {*reported_errors, error_code}
    known: set[Code] = {_CODES[code] for code in codes if code in _CODES}
    if known:
        return max(known, key=lambda code: _PRIORITY[code])
    if (
        codes.intersection(
            {
                "permission_denied",
                "output_limit_exceeded",
                "internal_error",
                "launch_error",
                "cancelled",
            }
        )
        or status is InvocationStatus.CANCELLED
    ):
        return None
    message = (error_message or "").lower()
    transient = bool(codes.intersection({"rate_limit", "429"})) or any(
        text in message for text in ("429", "rate limit", "too many requests")
    )
    hard_quota = any(
        text in message
        for text in ("quota", "usage limit", "weekly limit", "5-hour limit", "monthly limit")
    )
    for category, phrases in _TEXT.items():
        if category == "quota" and transient and not hard_quota:
            continue
        if any(text in message for text in phrases):
            return category
    return None


def observe_result(
    root: Path | None,
    result: InvocationResult,
    error_message: str | None,
    reported_errors: Iterable[str],
) -> InvocationResult:
    if root is not None:
        if result.status is InvocationStatus.SUCCEEDED:
            clear(root, result.provider)
        elif category := classify(
            result.status,
            result.error.code if result.error else None,
            error_message or (result.error.message if result.error else None),
            reported_errors,
        ):
            record(root, result.provider, category)
    return result

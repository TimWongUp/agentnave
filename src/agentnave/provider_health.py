"""Best-effort, CLI and model circuit breaker for recent account blockers."""

from __future__ import annotations

import json
from collections.abc import Iterable
from contextlib import suppress
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, Literal, NotRequired, TypedDict, cast

from agentnave.locks import lock_file, release_and_remove
from agentnave.models import InvocationResult, InvocationStatus
from agentnave.store import write_file

type Code = Literal["auth", "billing", "quota"]


class Health(TypedDict):
    code: Code
    until: str | None
    reset: NotRequired[str]


_MAX_TTL = timedelta(days=8)
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


def _entry(item: object, now: datetime) -> Health | None:
    if not isinstance(item, dict):
        return None
    item = cast(dict[str, Any], item)
    code, until = item.get("code"), item.get("until")
    if code == "auth" and until is None:
        return {"code": "auth", "until": None}
    if not isinstance(code, str) or code not in _PRIORITY or not isinstance(until, str):
        return None
    try:
        end = datetime.fromisoformat(until)
        if end.tzinfo is None or not now < end <= now + _MAX_TTL:
            return None
    except ValueError:
        return None
    result: Health = {"code": cast(Code, code), "until": until}
    if code == "quota" and isinstance(item.get("reset"), str):
        try:
            parsed_reset = datetime.fromisoformat(item["reset"])
            if parsed_reset.tzinfo is not None:
                result["reset"] = item["reset"]
        except ValueError:
            pass
    return result


def _active(document: dict[str, Any] | None, now: datetime) -> dict[tuple[str, str | None], Health]:
    if document is None or type(document.get("version")) is not int:
        return {}
    if document["version"] != 1 or not isinstance(document.get("providers"), dict):
        return {}
    result: dict[tuple[str, str | None], Health] = {}
    for provider, item in cast(dict[str, Any], document["providers"]).items():
        if not provider or not isinstance(item, dict):
            continue
        item = cast(dict[str, Any], item)
        # Version 1 previously stored a single CLI-wide record.
        if legacy := _entry(item, now):
            result[provider, None] = legacy
        if default := _entry(item.get("default"), now):
            result[provider, None] = default
        models = item.get("models")
        if isinstance(models, dict):
            for model, value in cast(dict[str, object], models).items():
                if model and (health := _entry(value, now)):
                    result[provider, model] = health
    return result


def active(root: Path | None = None) -> dict[tuple[str, str | None], Health]:
    return _active(_read(_root(root) / "provider-health.json"), datetime.now(UTC))


def matching(
    health: dict[tuple[str, str | None], Health], provider: str, model: str | None
) -> tuple[str | None, Health] | None:
    if (entry := health.get((provider, None))) is not None:
        return None, entry
    if model and (entry := health.get((provider, model))) is not None:
        return model, entry
    return None


def reason(provider: str, health: Health, model: str | None = None) -> str:
    target = f"{provider} CLI" + (f" model {model}" if model else "")
    duration = f"until {health['until']}" if health["until"] else "until cleared"
    return (
        f"{target} recently reported {health['code']}; cooling down {duration}. "
        "See list_members for a replacement. The user can clear it in App, "
        "or ask you to call clear_provider_cooldown."
    )


def model_option(options: dict[str, Any] | Any) -> str | None:
    model = options.get("model")
    return model if isinstance(model, str) and model else None


def check(root: Path, providers: Iterable[tuple[str, str | None]]) -> None:
    health = active(root)
    for provider, model in providers:
        if match := matching(health, provider, model):
            raise ValueError(reason(provider, match[1], match[0]))


def record(
    root: Path | None,
    provider: str,
    code: Code,
    model: str | None = None,
    reset: datetime | None = None,
) -> None:
    _update(_root(root), provider, code, model, reset)


def clear(
    root: Path | None, provider: str, model: str | None = None, *, all_models: bool = True
) -> bool:
    return _update(
        _root(root), provider, None, model, None, all_models=all_models and model is None
    )


def _update(
    root: Path,
    provider: str,
    code: Code | None,
    model: str | None,
    reset: datetime | None,
    *,
    all_models: bool = False,
) -> bool:
    path = root / "provider-health.json"
    lock_path = root / "locks" / "provider-health.lock"
    try:
        lock_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        handle = lock_file(lock_path, "Provider health is busy")
    except (OSError, ValueError):
        return False
    try:
        return _update_locked(path, provider, code, model, reset, all_models)
    except OSError:
        return False
    finally:
        with suppress(OSError):
            release_and_remove(handle, lock_path)


def _update_locked(
    path: Path,
    provider: str,
    code: Code | None,
    model: str | None,
    reset: datetime | None,
    all_models: bool,
) -> bool:
    document = _read(path)
    if document is not None and (
        type(document.get("version")) is not int or document["version"] != 1
    ):
        return False
    if document is None:
        if path.exists():
            return False
        document = cast(dict[str, Any], {"version": 1, "providers": {}})
    providers = document.get("providers")
    if not isinstance(providers, dict):
        return False
    providers = cast(dict[str, Any], providers)
    raw = providers.get(provider)
    if code is None and all_models:
        if provider not in providers:
            return True
        del providers[provider]
    else:
        if code is None and not isinstance(raw, dict):
            return True
        item = cast(dict[str, Any], raw) if isinstance(raw, dict) else {}
        # Keep old CLI-wide records as a default when adding model-specific records.
        if "code" in item:
            item = {"default": item}
        models = item.get("models")
        if not isinstance(models, dict):
            models = {}
        models = cast(dict[str, Any], models)
        previous = _entry(models.get(model) if model else item.get("default"), datetime.now(UTC))
        if code is None:
            if model:
                removed = models.pop(model, None) is not None
                removed_default = item.pop("default", None) is not None
                if not removed and not removed_default:
                    return True
            else:
                if "default" not in item:
                    return True
                del item["default"]
        else:
            now = datetime.now(UTC)
            if code == "quota":
                if reset is None or reset.tzinfo is None or reset <= now:
                    return True
                until = min(reset.astimezone(UTC), now + _MAX_TTL)
            else:
                until = now + timedelta(minutes=30) if code == "billing" else None
            entry = _merged_health(previous, code, until, reset)
            if entry == previous:
                return True
            if model:
                models[model] = dict(entry)
            else:
                item["default"] = dict(entry)
        if models:
            item["models"] = models
        else:
            item.pop("models", None)
        if item:
            providers[provider] = item
        else:
            providers.pop(provider, None)
    write_file(path, json.dumps(document).encode())
    return True


def _merged_health(
    previous: Health | None, code: Code, until: datetime | None, reset: datetime | None
) -> Health:
    if previous:
        old_code = previous["code"]
        if _PRIORITY[old_code] > _PRIORITY[code]:
            code = old_code
        old_until = previous["until"]
        if old_until is None:
            until = None
        elif until is not None:
            # A repeated quota reset beyond the cap must not extend it each time.
            if (
                code == "quota"
                and reset is not None
                and (previous_reset := previous.get("reset")) is not None
                and reset <= datetime.fromisoformat(previous_reset)
            ):
                return previous
            until = max(until, datetime.fromisoformat(old_until))
    if code == "auth":
        until = None
    entry: Health = {
        "code": code,
        "until": until.isoformat().replace("+00:00", "Z") if until else None,
    }
    if code == "quota" and reset is not None:
        entry["reset"] = reset.astimezone(UTC).isoformat().replace("+00:00", "Z")
    return entry


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
    model: str | None = None,
    reset: datetime | None = None,
) -> InvocationResult:
    if root is not None:
        if result.status is InvocationStatus.SUCCEEDED:
            clear(root, result.provider, model, all_models=False)
        elif category := classify(
            result.status,
            result.error.code if result.error else None,
            error_message or (result.error.message if result.error else None),
            reported_errors,
        ):
            record(root, result.provider, category, model, reset)
    return result

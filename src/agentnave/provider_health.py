"""Best-effort, CLI, account and model circuit breaker for recent account blockers."""

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
# A quota failure that names no reset time cools its model down for this long.
_QUOTA_GUESS = timedelta(hours=1)
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


# Active records by (provider, account, model): account None is the default account; the model
# is None for a whole account, a model, or "\0group:<group>" for a quota group.
type Key = tuple[str, str | None, str | None]


def _level(
    item: dict[str, Any], provider: str, account: str | None, now: datetime
) -> dict[Key, Health]:
    """One account's records: the provider object itself for the default account, or the
    object under its `accounts.<id>`."""
    result: dict[Key, Health] = {}
    # Version 1 previously stored a single CLI-wide record.
    if legacy := _entry(item, now):
        result[provider, account, None] = legacy
    if default := _entry(item.get("default"), now):
        result[provider, account, None] = default
    models = item.get("models")
    if isinstance(models, dict):
        for model, value in cast(dict[str, object], models).items():
            if model and (health := _entry(value, now)):
                result[provider, account, model] = health
    groups = item.get("groups")
    if isinstance(groups, dict):
        for group, value in cast(dict[str, object], groups).items():
            if group and (health := _entry(value, now)):
                result[provider, account, "\0group:" + group] = health
    return result


def _active(document: dict[str, Any] | None, now: datetime) -> dict[Key, Health]:
    if document is None or type(document.get("version")) is not int:
        return {}
    if document["version"] != 1 or not isinstance(document.get("providers"), dict):
        return {}
    result: dict[Key, Health] = {}
    for provider, item in cast(dict[str, Any], document["providers"]).items():
        if not provider or not isinstance(item, dict):
            continue
        item = cast(dict[str, Any], item)
        result.update(_level(item, provider, None, now))
        named = item.get("accounts")
        if isinstance(named, dict):
            for account, level in cast(dict[str, object], named).items():
                if account and isinstance(level, dict):
                    result.update(_level(cast(dict[str, Any], level), provider, account, now))
    return result


def active(root: Path | None = None) -> dict[Key, Health]:
    return _active(_read(_root(root) / "provider-health.json"), datetime.now(UTC))


def matching(
    health: dict[Key, Health], provider: str, model: str | None, account: str | None = None
) -> tuple[str | None, Health] | None:
    """The record that cools this account's model down, and what it names (None: the account)."""
    if (entry := health.get((provider, account, None))) is not None:
        return None, entry
    if (
        model
        and (group := quota_group(provider, model))
        and (entry := health.get((provider, account, "\0group:" + group))) is not None
    ):
        return group, entry
    if model and (entry := health.get((provider, account, model))) is not None:
        return model, entry
    return None


def quota_group(provider: str, model: str) -> str | None:
    from agentnave.adapters import get_adapter

    try:
        return get_adapter(provider).quota_group(model)
    except ValueError:
        return None


# 配额阻断时给 boss 的去向：先在同 CLI 其他未冷却额度组的模型上原位续接，再考虑跨 CLI 交接。
QUOTA_RESUME_HINT = (
    "If this CLI has another model in a quota group that is not cooling down, cancel and resume "
    "the same session_id with provider_options.model set to it "
    "(see agentnave-manager references/models.md); otherwise hand the work over to another CLI."
)


def blocker_hint(code: str) -> str:
    """The quota hint for a blocker known only by its code, with a leading space; else empty."""
    if classify(InvocationStatus.BLOCKED, code, None, [code]) == "quota":
        return " " + QUOTA_RESUME_HINT
    return ""


def reason(
    provider: str,
    health: Health,
    model: str | None = None,
    *,
    resume: bool = False,
    account: str | None = None,
) -> str:
    target = (
        f"{provider} CLI"
        + (f" account {account}" if account else "")
        + (f" model {model}" if model else "")
    )
    duration = f"until {health['until']}" if health["until"] else "until cleared"
    # 续接的是原生会话：换 CLI 会丢上下文，所以不指向 list_members 的跨 CLI replacement。
    next_step = (
        "To keep this thread's context, resume the same session_id with provider_options.model "
        "set to another model of this CLI that is not cooling down (models in the same quota "
        "group cool down together; see agentnave-manager references/models.md); if none is "
        "available, hand the work over to another CLI. "
        if resume
        else "See list_members for a replacement. "
    )
    return (
        f"{target} recently reported {health['code']}; cooling down {duration}. "
        f"{next_step}The user can clear it in App, or ask you to call clear_provider_cooldown."
    )


def model_option(options: dict[str, Any] | Any) -> str | None:
    model = options.get("model")
    return model if isinstance(model, str) and model else None


def check(
    root: Path, targets: Iterable[tuple[str, str | None, str | None]], *, resume: bool = False
) -> None:
    """Refuse a dispatch to a cooling target: (provider, account, model) each."""
    health = active(root)
    for provider, account, model in targets:
        if match := matching(health, provider, model, account):
            raise ValueError(reason(provider, match[1], match[0], resume=resume, account=account))


def record(
    root: Path | None,
    provider: str,
    code: Code,
    model: str | None = None,
    reset: datetime | None = None,
    *,
    account: str | None = None,
) -> None:
    # A named account is one login: when it fails to authenticate, no model of it can run.
    # The default account may route models to different providers, so it keeps its per-model
    # records (ADR-0081).
    if code == "auth" and account is not None:
        model = None
    _update(_root(root), provider, account, code, model, reset)


def clear(
    root: Path | None,
    provider: str,
    model: str | None = None,
    *,
    all_models: bool = True,
    account: str | None = None,
) -> bool:
    """Clear records of one account (the default one unless named); with no model, all of them."""
    return _update(
        _root(root), provider, account, None, model, None, all_models=all_models and model is None
    )


def _update(
    root: Path,
    provider: str,
    account: str | None,
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
        return _update_locked(path, provider, account, code, model, reset, all_models)
    except OSError:
        return False
    finally:
        with suppress(OSError):
            release_and_remove(handle, lock_path)


def _update_locked(
    path: Path,
    provider: str,
    account: str | None,
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
    container = cast(dict[str, Any], raw) if isinstance(raw, dict) else {}
    # Keep old CLI-wide records as the default account's default when adding other records.
    if "code" in container:
        legacy = {key: container[key] for key in ("code", "until", "reset") if key in container}
        container = {
            "default": legacy,
            **({"accounts": container["accounts"]} if "accounts" in container else {}),
        }
    named = container.get("accounts")
    named = cast(dict[str, Any], named) if isinstance(named, dict) else {}
    # The default account's records are the provider object itself, a named account's the object
    # under accounts.<id>; each is edited alone and the other is kept as it was.
    if account is None:
        item = {key: value for key, value in container.items() if key != "accounts"}
    else:
        level = named.get(account)
        item = cast(dict[str, Any], level) if isinstance(level, dict) else {}
    if code is None and all_models:
        if not item:
            return True
        item.clear()
    elif not _edit_level(item, provider, code, model, reset):
        return True
    if account is None:
        container = {**item, **({"accounts": named} if named else {})}
    else:
        if item:
            named[account] = item
        else:
            named.pop(account, None)
        container = {key: value for key, value in container.items() if key != "accounts"}
        if named:
            container["accounts"] = named
    if container:
        providers[provider] = container
    else:
        providers.pop(provider, None)
    write_file(path, json.dumps(document).encode())
    return True


def _edit_level(
    item: dict[str, Any],
    provider: str,
    code: Code | None,
    model: str | None,
    reset: datetime | None,
) -> bool:
    """Add or remove one record of an account's level; False when nothing needs writing."""
    if code is None and not item:
        return False
    models = item.get("models")
    if not isinstance(models, dict):
        models = {}
    models = cast(dict[str, Any], models)
    groups = item.get("groups")
    if not isinstance(groups, dict):
        groups = {}
    groups = cast(dict[str, Any], groups)
    group = quota_group(provider, model) if model else None
    target = groups if code == "quota" and group else models
    key = group if code == "quota" and group else model
    previous = _entry(target.get(key) if key else item.get("default"), datetime.now(UTC))
    if code is None:
        if model:
            removed = models.pop(model, None) is not None
            removed_group = group is not None and groups.pop(group, None) is not None
            removed_default = item.pop("default", None) is not None
            if not removed and not removed_group and not removed_default:
                return False
        else:
            if "default" not in item:
                return False
            del item["default"]
    else:
        now = datetime.now(UTC)
        if code == "quota":
            if reset is None:
                if previous is not None:
                    return False  # an active record stands; a repeat does not extend it
                until = now + _QUOTA_GUESS
            elif reset.tzinfo is None or reset <= now:
                return False
            else:
                until = min(reset.astimezone(UTC), now + _MAX_TTL)
        else:
            until = now + timedelta(minutes=30) if code == "billing" else None
        entry = _merged_health(previous, code, until, reset)
        if entry == previous:
            return False
        if key:
            target[key] = dict(entry)
        else:
            item["default"] = dict(entry)
    if models:
        item["models"] = models
    else:
        item.pop("models", None)
    if groups:
        item["groups"] = groups
    else:
        item.pop("groups", None)
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
        elif code == "quota" and reset is not None and "reset" not in previous:
            pass  # a reset time replaces the guess recorded before it, even when that is earlier
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


def blocker(
    result: InvocationResult, error_message: str | None, reported_errors: Iterable[str]
) -> Code | None:
    """The account blocker a failed result reports, if any."""
    if result.status is InvocationStatus.SUCCEEDED:
        return None
    return classify(
        result.status,
        result.error.code if result.error else None,
        error_message or (result.error.message if result.error else None),
        reported_errors,
    )


def observe_result(
    root: Path | None,
    result: InvocationResult,
    error_message: str | None,
    reported_errors: Iterable[str],
    model: str | None = None,
    reset: datetime | None = None,
    *,
    account: str | None = None,
) -> InvocationResult:
    if root is not None:
        if result.status is InvocationStatus.SUCCEEDED:
            clear(root, result.provider, model, all_models=False, account=account)
        elif category := blocker(result, error_message, reported_errors):
            record(root, result.provider, category, model, reset, account=account)
    return result

"""Provider adapter protocol and shared helpers."""

from __future__ import annotations

import json
import math
import os
import shutil
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Protocol, cast

from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity

# Common install locations for provider CLIs when the host passes a minimal PATH.
PROVIDER_BIN_DIRS = (
    "~/.local/bin",
    "~/.grok/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.bun/bin",
    "~/.npm-global/bin",
    "~/.volta/bin",
    "~/.cargo/bin",
)


def provider_bin_dirs() -> list[str]:
    """Existing common install directories, resolved on every call."""
    found: list[str] = []
    for entry in PROVIDER_BIN_DIRS:
        path = os.path.expanduser(entry)
        if os.path.isdir(path) and path not in found:
            found.append(path)
    return found


def resolve_executable(name: str, preferred: Iterable[str] = ()) -> str:
    """Absolute provider path, or the bare name so launch reports it as missing.

    Resolution runs on every prepare, so installing or removing a CLI takes effect
    on the next call. Windows keeps resolving the bare name inside its supervisor.
    """
    for candidate in preferred:
        executable = shutil.which(os.path.expanduser(candidate))
        if executable is not None:
            return executable
    if os.name == "nt":
        return name
    executable = shutil.which(name)
    if executable is None:
        executable = shutil.which(name, path=os.pathsep.join(provider_bin_dirs()))
    return os.path.abspath(executable) if executable is not None else name


@dataclass(frozen=True, slots=True)
class PreparedCommand:
    argv: tuple[str, ...]
    cwd: Path
    stdin: bytes | None = None
    cleanup_paths: tuple[Path, ...] = ()
    capture_line: Callable[[bytes], bytes] | None = None
    # Given the head of a line above the capture limit, True drops it instead of failing.
    skip_oversized_line: Callable[[bytes], bool] | None = None
    # A new native session the provider was told to create, so a run stopped before the
    # provider reports its id can still be continued.
    session_id: str | None = None


@dataclass(frozen=True, slots=True)
class ParsedProviderResult:
    status: InvocationStatus
    output: str
    session_id: str | None = None
    usage: dict[str, int | float] = field(default_factory=lambda: {})
    error_message: str | None = None
    warnings: tuple[str, ...] = ()


class ProviderAdapter(Protocol):
    name: str

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]: ...

    def prepare(self, request: InvocationRequest) -> PreparedCommand: ...

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult: ...

    def activity(self, event: dict[str, object]) -> ProviderActivity | None: ...


def brief(value: object, limit: int = 512) -> str | None:
    """Bound only explicitly selected public fields; never stringify arbitrary payloads."""
    return value[-limit:] if isinstance(value, str) and value else None


def parse_json_object(text: str) -> dict[str, object] | None:
    try:
        raw = cast(object, json.loads(text, parse_int=_parse_json_int))
    except (ValueError, RecursionError):
        return None
    if not isinstance(raw, dict):
        return None
    mapping = cast(dict[object, object], raw)
    return {str(key): value for key, value in mapping.items()}


def _parse_json_int(value: str) -> int | str:
    try:
        return int(value)
    except ValueError:
        return value


def parse_json_lines(text: str) -> list[dict[str, object]]:
    return [payload for line in text.splitlines() if (payload := parse_json_object(line))]


def object_dict(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        return {}
    mapping = cast(dict[object, object], value)
    return {str(key): item for key, item in mapping.items()}


def model_from_event(provider: str, event: dict[str, object]) -> str | None:
    """Read native model identity only from known top-level protocol fields."""
    value: object = None
    kind = event.get("type")
    if provider in ("claude", "codebuddy") and event.get("parent_tool_use_id") is None:
        if kind == "assistant":
            value = object_dict(event.get("message")).get("model")
        elif kind == "stream_event":
            nested = object_dict(event.get("event"))
            if nested.get("type") == "message_start":
                value = object_dict(nested.get("message")).get("model")
    elif provider == "grok" and kind == "end":
        # Grok reports no model per message; its final usage lists every model it called.
        # Only a single entry is unambiguous: subagents may add other models.
        usage = object_dict(event.get("modelUsage"))
        if len(usage) == 1:
            value = next(iter(usage))
    elif provider == "pi" and kind == "message_end":
        message = object_dict(event.get("message"))
        if message.get("role") == "assistant":
            value = message.get("responseModel") or message.get("model")
    elif provider == "zcode" and kind == "model":
        value = event.get("model")
    if (
        isinstance(value, str)
        and value.strip()
        and value.strip() not in {"<synthetic>", "unknown"}
        and len(value) <= 200
        and all(c.isprintable() for c in value)
    ):
        return value.strip()
    return None


def normalized_usage(payload: dict[str, object]) -> dict[str, int | float]:
    """Keep only usage summary values supported by the stable MCP result contract."""
    nested = object_dict(payload.get("usage"))
    usage: dict[str, int | float] = {}
    num_turns = payload.get("num_turns", nested.get("num_turns"))
    if isinstance(num_turns, int) and not isinstance(num_turns, bool):
        usage["num_turns"] = num_turns
    total_cost = payload.get("total_cost_usd", nested.get("total_cost_usd"))
    if isinstance(total_cost, (int, float)) and not isinstance(total_cost, bool):
        try:
            normalized_cost = float(total_cost)
        except OverflowError:
            pass
        else:
            if math.isfinite(normalized_cost):
                usage["total_cost_usd"] = normalized_cost
    return usage


def failure_status(text: str) -> InvocationStatus:
    lowered = text.lower()
    blocked_markers = (
        "permission denied",
        "permission required",
        "approval required",
        "authentication failed",
        "authentication required",
        "not authenticated",
        "/login command",
        "login required",
    )
    return (
        InvocationStatus.BLOCKED
        if any(marker in lowered for marker in blocked_markers)
        else InvocationStatus.FAILED
    )


def error_summary(text: str, fallback: str, limit: int = 512) -> str:
    normalized = " ".join(text.split())
    return (normalized or fallback)[:limit]


def option_args(request: InvocationRequest, allowlist: dict[str, str]) -> tuple[str, ...]:
    unknown = sorted(set(request.provider_options) - set(allowlist))
    if unknown:
        raise ValueError(f"unsupported {request.provider} options: {', '.join(unknown)}")
    args: list[str] = []
    for key, value in request.provider_options.items():
        args.extend((allowlist[key], str(value).lower() if isinstance(value, bool) else str(value)))
    return tuple(args)


# Options that grant no tools: Pi's and DeepSeek's provider only pick the model backend,
# and DeepSeek's stored_credentials only restores the app's saved sign-in.
RESTRICTED_EXTRA_OPTIONS: dict[str, set[str]] = {
    "pi": {"provider"},
    "deepseek": {"provider", "stored_credentials"},
}


def restricted_options(request: InvocationRequest) -> tuple[InvocationRequest, bool]:
    """Explicit opt-in only; restricted profiles cannot enable other tools."""
    options = dict(request.provider_options)
    enabled = options.pop("restricted_tools", False)
    if not isinstance(enabled, bool):
        raise ValueError("restricted_tools must be a boolean")
    if enabled:
        if request.provider not in (
            "antigravity",
            "claude",
            "codebuddy",
            "codex",
            "deepseek",
            "grok",
            "pi",
        ):
            raise ValueError("restricted_tools requires a registered provider")
        allowed = {"model", "effort"} | RESTRICTED_EXTRA_OPTIONS.get(request.provider, set())
        if set(options) - allowed:
            raise ValueError("restricted_tools accepts only model and effort")
    return replace(request, provider_options=options), enabled

"""Provider adapter protocol and shared helpers."""

from __future__ import annotations

import json
import math
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol, cast

from agentnave.models import InvocationRequest, InvocationStatus


@dataclass(frozen=True, slots=True)
class PreparedCommand:
    argv: tuple[str, ...]
    cwd: Path
    stdin: bytes | None = None
    cleanup_paths: tuple[Path, ...] = ()


@dataclass(frozen=True, slots=True)
class ParsedProviderResult:
    status: InvocationStatus
    output: str
    session_id: str | None = None
    usage: dict[str, int | float] = field(default_factory=lambda: {})
    error_message: str | None = None


class ProviderAdapter(Protocol):
    name: str

    def prepare(self, request: InvocationRequest) -> PreparedCommand: ...

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult: ...


def parse_json_object(text: str) -> dict[str, object] | None:
    try:
        raw = cast(object, json.loads(text, parse_int=_parse_json_int))
    except json.JSONDecodeError:
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

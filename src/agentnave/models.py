"""Stable request and result contracts for one provider invocation."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path
from types import MappingProxyType

type ProviderOption = str | int | float | bool


class InvocationStatus(StrEnum):
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    BLOCKED = "blocked"
    CANCELLED = "cancelled"
    TIMED_OUT = "timed_out"


class InvocationPhase(StrEnum):
    PREPARING = "preparing"
    RUNNING = "running"
    STOPPING = "stopping"


@dataclass(frozen=True, slots=True)
class InvocationRequest:
    provider: str
    prompt: str
    cwd: Path
    session_id: str | None = None
    timeout_seconds: float | None = None
    provider_options: Mapping[str, ProviderOption] = field(default_factory=lambda: {})

    def __post_init__(self) -> None:
        provider = self.provider.strip().lower()
        if not self.cwd.is_absolute():
            raise ValueError("cwd must be an absolute path")
        cwd = self.cwd.resolve()
        if not provider:
            raise ValueError("provider must not be empty")
        if not self.prompt.strip():
            raise ValueError("prompt must not be empty")
        if not cwd.is_dir():
            raise ValueError(f"cwd is not a directory: {cwd}")
        if self.session_id is not None and not self.session_id.strip():
            raise ValueError("session_id must not be empty")
        if self.session_id is not None and (
            self.session_id.startswith("-")
            or any(ord(character) < 32 or ord(character) == 127 for character in self.session_id)
        ):
            raise ValueError("session_id contains unsafe characters")
        if self.timeout_seconds is not None and not 0 < self.timeout_seconds <= 86_400:
            raise ValueError("timeout_seconds must be between 0 and 86400")
        object.__setattr__(self, "provider", provider)
        object.__setattr__(self, "cwd", cwd)
        object.__setattr__(self, "provider_options", MappingProxyType(dict(self.provider_options)))


@dataclass(frozen=True, slots=True)
class InvocationError:
    code: str
    message: str
    details: str | None = None

    def to_dict(self) -> dict[str, object]:
        return {"code": self.code, "message": self.message, "details": self.details}


@dataclass(frozen=True, slots=True)
class ProviderActivity:
    """One observed provider event, not an inferred view of all active work."""

    kind: str
    event_type: str
    state: str | None = None
    tool_name: str | None = None
    message: str | None = None
    tool_call_id: str | None = None
    message_delta: bool = False

    def to_dict(self) -> dict[str, object]:
        return {
            "kind": self.kind,
            "event_type": self.event_type,
            "state": self.state,
            "tool_name": self.tool_name,
            "message": self.message,
            "tool_call_id": self.tool_call_id,
        }


@dataclass(frozen=True, slots=True)
class InvocationSnapshot:
    phase: InvocationPhase
    elapsed_ms: int
    last_event_age_ms: int | None
    last_activity: ProviderActivity | None = None
    last_activity_age_ms: int | None = None
    remaining_ms: int | None = None

    def to_dict(self) -> dict[str, object]:
        return {
            "phase": self.phase.value,
            "elapsed_ms": self.elapsed_ms,
            "last_event_age_ms": self.last_event_age_ms,
            "last_activity": None if self.last_activity is None else self.last_activity.to_dict(),
            "last_activity_age_ms": self.last_activity_age_ms,
            "remaining_ms": self.remaining_ms,
        }


@dataclass(frozen=True, slots=True)
class InvocationResult:
    status: InvocationStatus
    provider: str
    output: str
    session_id: str | None
    provider_usage: dict[str, int | float]
    duration_ms: int
    error: InvocationError | None = None

    def to_dict(self) -> dict[str, object]:
        return {
            "status": self.status.value,
            "provider": self.provider,
            "output": self.output,
            "session_id": self.session_id,
            "provider_usage": self.provider_usage,
            "duration_ms": self.duration_ms,
            "error": None if self.error is None else self.error.to_dict(),
        }

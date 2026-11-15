"""Stable request and result contracts: one provider invocation, and the round orders."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path
from types import MappingProxyType
from typing import Literal

type HostName = Literal[
    "codex",
    "claude",
    "codebuddy",
    "workbuddy",
    "grok",
    "antigravity",
    "pi",
    "deepseek",
    "devin",
    "hermes",
    "zcode",
]
# Hosts no longer connected or detected; saved history may still name them.
type RetiredHostName = Literal["gemini", "opencode"]
type HostSource = Literal["env", "parent", "unknown"]


@dataclass(frozen=True)
class Host:
    name: HostName | None = None
    source: HostSource = "unknown"
    app_id: str | None = None

    @property
    def provider(self) -> str | None:
        if self.name == "workbuddy":
            return "codebuddy"
        if self.name == "hermes":
            return None
        return self.name


type ProviderOption = bool | int | float | str
type RoundOrder = Literal["parallel", "serial"]


class InvocationStatus(StrEnum):
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    BLOCKED = "blocked"
    CANCELLED = "cancelled"


class InvocationPhase(StrEnum):
    PREPARING = "preparing"
    RUNNING = "running"
    STOPPING = "stopping"


def validate_invocation_inputs(
    provider: str,
    prompt: str,
    cwd: Path,
    session_id: str | None = None,
    *,
    require_cwd_dir: bool = True,
) -> tuple[str, Path]:
    """Validate invocation inputs; callers that rebuild missing worktrees defer require_cwd_dir."""
    normalized_provider = provider.strip().lower()
    if not cwd.is_absolute():
        raise ValueError("cwd must be an absolute path")
    resolved_cwd = cwd.resolve()
    if not normalized_provider:
        raise ValueError("provider must not be empty")
    if not prompt.strip():
        raise ValueError("prompt must not be empty")
    if require_cwd_dir and not resolved_cwd.is_dir():
        raise ValueError(f"cwd is not a directory: {resolved_cwd}")
    if session_id is not None and not session_id.strip():
        raise ValueError("session_id must not be empty")
    if session_id is not None and (
        session_id.startswith("-")
        or any(ord(character) < 32 or ord(character) == 127 for character in session_id)
    ):
        raise ValueError("session_id contains unsafe characters")
    return normalized_provider, resolved_cwd


@dataclass(frozen=True, slots=True)
class InvocationRequest:
    provider: str
    prompt: str
    cwd: Path
    session_id: str | None = None
    provider_options: Mapping[str, ProviderOption] = field(default_factory=lambda: {})
    read_only: bool = False  # Adds the read-only instruction; native permissions stay as they are.
    extra_instructions: str | None = None  # Appended after the shared instructions.

    def __post_init__(self) -> None:
        provider, cwd = validate_invocation_inputs(
            self.provider, self.prompt, self.cwd, self.session_id
        )
        object.__setattr__(self, "provider", provider)
        object.__setattr__(self, "cwd", cwd)
        object.__setattr__(self, "provider_options", MappingProxyType(dict(self.provider_options)))


@dataclass(frozen=True, slots=True)
class InvocationError:
    code: str
    message: str
    details: str | None = None


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
    blocking_error: str | None = None
    public_output: str | None = None


@dataclass(frozen=True, slots=True)
class InvocationSnapshot:
    phase: InvocationPhase
    elapsed_ms: int
    last_event_age_ms: int | None
    last_activity: ProviderActivity | None = None
    last_activity_age_ms: int | None = None


@dataclass(frozen=True, slots=True)
class InvocationListing:
    """Metadata of one retained invocation; never its prompt or output."""

    invocation_id: str
    provider: str
    cwd: Path
    elapsed_ms: int
    events_seen: bool
    phase: InvocationPhase | None = None
    status: InvocationStatus | None = None
    session_id: str | None = None
    result_read: bool = False
    error: InvocationError | None = None
    read_only: bool = False


@dataclass(frozen=True, slots=True)
class InvocationResult:
    status: InvocationStatus
    provider: str
    output: str
    session_id: str | None
    duration_ms: int
    error: InvocationError | None = None
    reported_model: str | None = None
    # Problems in a run the CLI still reported as finished, such as denied tool calls.
    warnings: tuple[str, ...] = ()

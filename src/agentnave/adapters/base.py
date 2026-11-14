"""Provider adapter protocol and shared helpers."""

from __future__ import annotations

import os
import shutil
from collections.abc import Callable, Iterable
from dataclasses import dataclass, replace
from datetime import datetime
from pathlib import Path
from typing import Protocol

from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity
from agentnave.native_prompts import NativePrompt
from agentnave.native_sessions import clean_model as clean_model
from agentnave.native_sessions import object_dict as object_dict
from agentnave.native_sessions import parse_json_object as parse_json_object

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
    # Windows: npm's global prefix, the per-user Node install (its `node` runs ZCode's
    # bundle), and the installers that add their own bin directory.
    "~/AppData/Roaming/npm",
    "~/AppData/Local/Programs/nodejs",
    "~/AppData/Local/ZCode/bin",
    "~/AppData/Local/devin/cli/bin",
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
    on the next call. Windows resolves `PATHEXT` suffixes itself, so the same fallback
    finds `codex.cmd` and `codex.exe` there too.
    """
    for candidate in preferred:
        executable = shutil.which(os.path.expanduser(candidate))
        if executable is not None:
            return executable
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
    keep_stdin_open: bool = False
    unset_env: tuple[str, ...] = ()
    # Create this private directory only when the invocation starts; its name is exposed to the
    # helper through this environment variable and removed after the process tree stops.
    temp_dir_env: str | None = None


@dataclass(frozen=True, slots=True)
class UsageQuery:
    """A CLI's own command that reports its quota windows, run by the service (ADR-0081)."""

    argv: tuple[str, ...]
    # Reads the command's output into the time the quota that ran out resets, if it can.
    parse: Callable[[bytes, datetime], datetime | None]
    # Written to the command's stdin; with none, stdin is closed at once.
    stdin: bytes | None = None
    # With a command that keeps running, whether the output so far holds the answer.
    complete: Callable[[bytes], bool] | None = None


@dataclass(frozen=True, slots=True)
class ParsedProviderResult:
    status: InvocationStatus
    output: str
    session_id: str | None = None
    error_message: str | None = None
    warnings: tuple[str, ...] = ()


class ProviderAdapter(Protocol):
    name: str
    options_description: str
    supports_restricted_tools: bool
    restricted_extra_options: frozenset[str]

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]: ...

    def is_installed(self) -> bool: ...

    def prepare(self, request: InvocationRequest) -> PreparedCommand: ...

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult: ...

    def parse_prepared(
        self, prepared: PreparedCommand, returncode: int, stdout: bytes, stderr: bytes
    ) -> ParsedProviderResult: ...

    def quota_reset(self, stdout: bytes, stderr: bytes, now: datetime) -> datetime | None: ...

    def usage_query(self, model: str | None) -> UsageQuery | None: ...

    def quota_group(self, model: str) -> str | None: ...

    def activity(self, event: dict[str, object]) -> ProviderActivity | None: ...

    def event_model(self, event: dict[str, object]) -> str | None: ...

    def session_model(self, session_id: str | None, *, since: datetime) -> str | None: ...

    def prompt_event(self, event: dict[str, object]) -> NativePrompt | None: ...

    def encode_prompt_answer(self, answer: dict[str, object]) -> bytes: ...

    def prepare_seat(self, directory: Path) -> None: ...


class AdapterHooks:
    """What most CLIs do not need: the defaults of the optional ProviderAdapter hooks."""

    name: str
    executable_name: str | None = None
    options_description = ""
    supports_restricted_tools = True
    restricted_extra_options: frozenset[str] = frozenset()

    def quota_group(self, model: str) -> str | None:
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        raise NotImplementedError

    def quota_reset(self, stdout: bytes, stderr: bytes, now: datetime) -> datetime | None:
        return None

    def usage_query(self, model: str | None) -> UsageQuery | None:
        """The CLI's own usage command for the quota this model ran out of, if it has one."""
        return None

    def parse_prepared(
        self, prepared: PreparedCommand, returncode: int, stdout: bytes, stderr: bytes
    ) -> ParsedProviderResult:
        return self.parse(returncode, stdout, stderr)

    def executable_path(self) -> str:
        return resolve_executable(self.executable_name or self.name)

    def is_installed(self) -> bool:
        """Resolve the launch executable without preparing a command or starting a process."""
        return os.path.isabs(self.executable_path())

    def event_model(self, event: dict[str, object]) -> str | None:
        """The main assistant's model named by one native stream event."""
        return None

    def session_model(self, session_id: str | None, *, since: datetime) -> str | None:
        """The model a finished run used, read from the CLI's own files when it streams none."""
        return None

    def prompt_event(self, event: dict[str, object]) -> NativePrompt | None:
        return None

    def encode_prompt_answer(self, answer: dict[str, object]) -> bytes:
        raise ValueError("this adapter does not support native prompts")

    def prepare_seat(self, directory: Path) -> None:
        """Prepare a dedicated, empty seat directory before restricted runs use it."""


def brief(value: object, limit: int = 512) -> str | None:
    """Bound only explicitly selected public fields; never stringify arbitrary payloads."""
    return value[-limit:] if isinstance(value, str) and value else None


def parse_json_lines(text: str) -> list[dict[str, object]]:
    return [payload for line in text.splitlines() if (payload := parse_json_object(line))]


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


def restricted_options(
    request: InvocationRequest, adapter: ProviderAdapter
) -> tuple[InvocationRequest, bool]:
    """Explicit opt-in only; restricted profiles cannot enable other tools."""
    options = dict(request.provider_options)
    enabled = options.pop("restricted_tools", False)
    if not isinstance(enabled, bool):
        raise ValueError("restricted_tools must be a boolean")
    if enabled:
        if not adapter.supports_restricted_tools:
            raise ValueError("restricted_tools requires a registered provider")
        allowed = {"model", "effort"} | adapter.restricted_extra_options
        if set(options) - allowed:
            raise ValueError("restricted_tools accepts only model and effort")
    return replace(request, provider_options=options), enabled

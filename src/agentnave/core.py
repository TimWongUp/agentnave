"""In-memory lifecycle manager for provider invocations."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import random
import re
import shutil
import tempfile
import time
import uuid
from collections import deque
from collections.abc import Callable, Container, Sequence
from contextlib import suppress
from dataclasses import dataclass, field, replace
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import IO

from agentnave import accounts, provider_health
from agentnave.adapters import ProviderAdapter, get_adapter
from agentnave.adapters.base import AdapterHooks, PreparedCommand, parse_json_object
from agentnave.background_tasks import (
    BACKGROUND_TASKS_PENDING,
    BACKGROUND_WAIT_REMINDER_SECONDS,
    BackgroundTasksState,
    BackgroundWaitTracker,
    background_cancel_warning,
    background_tasks_pending_error,
)
from agentnave.locks import lock_file, release_and_remove
from agentnave.models import (
    InvocationError,
    InvocationListing,
    InvocationPhase,
    InvocationRequest,
    InvocationResult,
    InvocationSnapshot,
    InvocationStatus,
    ProviderActivity,
)
from agentnave.native_prompts import NativePrompt, PromptChannel
from agentnave.processes import (
    ProviderCompletion,
    SupervisedProcess,
    spawn_process,
    terminate_process_tree,
)
from agentnave.progress_stall import (
    PROGRESS_STALL_SECONDS,
    ProgressTracker,
    progress_stall_guard,
    progress_stalled_error,
)
from agentnave.push_capability import TrayIdentity
from agentnave.terminal_execution import TerminalLaunch, TerminalRun, execute_run
from agentnave.usage_probe import run_usage_query
from agentnave.warm_pool import (
    KEEP_SECONDS,
    MAX_KEPT,
    KeptProcess,
    WarmPool,
    can_keep,
    launch_signature,
)

_MAX_STDOUT_BYTES = 1024 * 1024
_MAX_EVENT_BYTES = 8 * 1024 * 1024
_MAX_STDERR_BYTES = 64 * 1024
_MAX_ERROR_DETAILS = 16_384
_DIAGNOSTIC_TAIL_BYTES = 4096
_SENSITIVE_DIAGNOSTIC = re.compile(
    r"authorization|bearer|cookie|password|passwd|secret|credential|private[ _-]?key|"
    r"api[ _-]?key|access[ _-]?token|refresh[ _-]?token|"
    r"(?:token|key)\b[\"']?\s*[:=]|"
    r"[a-z][a-z0-9+.-]*://[^/\s]*@|[A-Za-z0-9+/_=-]{40,}",
    re.IGNORECASE,
)
_OUTPUT_LIMIT_ERROR = InvocationError(
    "output_limit_exceeded", "provider output exceeded the AgentNave capture limit"
)
# A provider that keeps reporting retries without producing output is stuck (a network fault
# Codex retries forever); stop it instead of letting the invocation run with no end.
_RETRY_STALL_SECONDS = 300.0
# A single retry followed by silence is a recovered provider, not a stuck one: the streak only
# counts while retries keep arriving.
_RETRY_STALL_QUIET_SECONDS = 60.0
_RETRY_STALL_ERROR = InvocationError(
    "provider_error",
    "the provider kept retrying without producing output for 5 minutes; the invocation was stopped",
)
_FOREIGN_SESSION_ERROR = InvocationError(
    "foreign_session",
    "CLI returned a native session belonging to another concurrent task; "
    "output cannot be trusted, retry the task.",
)
_PIPE_DRAIN_SECONDS = 1.0
# After an Adapter's terminal event, the provider gets this long to exit on its own (ADR-0095).
_TERMINAL_GRACE_SECONDS = 0.5
_FINISHED_RETENTION_SECONDS = 7 * 24 * 60 * 60
_ENDED_TOOL_STATES = frozenset({"completed", "done", "failed", "error", "errored"})
_RECENT_TOOLS = 5


def _doing(activity: ProviderActivity | None) -> str | None:
    if activity is None:
        return None
    if activity.kind == "tool":
        return (
            "thinking"
            if (activity.state or "").lower() in _ENDED_TOOL_STATES
            else f"tool:{activity.tool_name or ''}"
        )
    if activity.kind == "lifecycle" and activity.state == "reasoning":
        return "thinking"
    if activity.kind == "message":
        return "writing"
    if activity.kind == "retry":
        return "retrying"
    return None


@dataclass(slots=True)
class _InvocationRecord:
    request: InvocationRequest
    health_root: Path | None = None
    reported_model: str | None = None
    started_wall_at: datetime = field(default_factory=lambda: datetime.now(UTC))
    started_at: float = field(default_factory=time.monotonic)
    phase: InvocationPhase = InvocationPhase.PREPARING
    cancel_event: asyncio.Event = field(default_factory=asyncio.Event)
    task: asyncio.Task[InvocationResult] | None = None
    cleanup_handle: asyncio.TimerHandle | None = None
    process: asyncio.subprocess.Process | None = None
    last_event_at: float | None = None
    last_activity: ProviderActivity | None = None
    last_activity_at: float | None = None
    doing: str | None = None
    doing_since: float | None = None
    # When the current run of "retrying" activity began, so a stuck provider can be timed out.
    retry_since: float | None = None
    retry_at: float | None = None
    tool_names: dict[str, str] = field(default_factory=lambda: {})
    # Names of the latest tool calls, oldest first, one entry per call.
    recent_tools: deque[str] = field(default_factory=lambda: deque[str](maxlen=_RECENT_TOOLS))
    terminal_event: asyncio.Event = field(default_factory=asyncio.Event)
    attention_event: asyncio.Event = field(default_factory=asyncio.Event)
    pending_attention: deque[InvocationError] = field(
        default_factory=lambda: deque[InvocationError]()
    )
    reported_errors: set[str] = field(default_factory=lambda: set[str]())
    output_tail: str = ""
    output_at: float | None = None
    output_event_type: str | None = None
    # Whether a caller has received the final result; lets the relay restart only when no
    # unread result would be lost.
    result_read: bool = False
    prompts: PromptChannel = field(default_factory=PromptChannel)
    start_seq: int = 0
    end_seq: int | None = None
    session_id: str | None = None
    on_inbox_change: Callable[[], None] | None = None
    background_wait: BackgroundWaitTracker = field(default_factory=BackgroundWaitTracker)
    background_wait_changed: asyncio.Event = field(default_factory=asyncio.Event)
    background_only: bool = False
    background_reply: str | None = None
    background_reminder_sent: bool = False
    progress: ProgressTracker = field(default_factory=ProgressTracker)
    # A process whose turn ended with it still running, until the run's outcome decides
    # whether it is kept for the next resume (ADR-0120).
    kept: KeptProcess | None = None
    # Runs in an isolated dir, which is released after the run: its process is never kept.
    isolated: bool = False

    def raise_attention(self, error: InvocationError) -> None:
        """Queue a running-time blocker for the next wait and wake the inbox."""
        self.pending_attention.append(error)
        self.attention_event.set()
        if self.on_inbox_change is not None:
            self.on_inbox_change()

    def accepts_reminders(self) -> bool:
        """Whether a one-shot reminder still makes sense: the run is live and unsettled."""
        return (
            self.phase != InvocationPhase.STOPPING
            and not self.terminal_event.is_set()
            and (self.task is None or not self.task.done())
        )

    def counts_silence(self) -> bool:
        """Whether quiet time counts toward the no-progress reminder (ADR-0116)."""
        return not self.prompts.pending() and not self.background_wait.waiting

    def observe_background_tasks(self, state: BackgroundTasksState) -> None:
        if state.session_id:
            self.session_id = state.session_id
        if state.model:
            self.reported_model = state.model
        self.background_only = state.only_background and bool(state.reply and state.reply.strip())
        self.background_reply = state.reply if self.background_only else None
        was_waiting = self.background_wait.waiting
        if self.background_wait.update(state):
            self.background_wait_changed.set()
        if was_waiting != self.background_wait.waiting:
            self.progress.hold()

    def observe_event(self, line: bytes, adapter: ProviderAdapter) -> None:
        event = parse_json_object(line.decode(errors="replace"))
        if event is None:
            return
        self.last_event_at = time.monotonic()
        if adapter.terminal_event(event):
            self.terminal_event.set()
        # Once the final outcome is known, the task's end is reported once by its settlement;
        # a blocker in that outcome is not announced as a running blocker first.
        finished = self.terminal_event.is_set()
        self.reported_model = adapter.event_model(event) or self.reported_model
        if (bg_state := adapter.background_tasks_event(event)) is not None:
            self.observe_background_tasks(bg_state)
        if self.prompts.encode is not None:
            prompt = adapter.prompt_event(event)
            if prompt is not None and self.prompts.receive(prompt):
                self.retry_since = self.retry_at = None
                self.attention_event.set()
        activity = adapter.activity(event)
        if adapter.is_progress(event, activity):
            self.progress.note(self.last_event_at)
        if activity is not None:
            code = activity.blocking_error
            if code is not None and code not in self.reported_errors:
                self.reported_errors.add(code)
                category = provider_health.classify(
                    InvocationStatus.BLOCKED, code, activity.message, self.reported_errors
                )
                if self.health_root is not None and category:
                    provider_health.record(
                        self.health_root,
                        self.request.provider,
                        category,
                        provider_health.model_option(self.request.provider_options),
                        adapter.quota_reset(line, b"", datetime.now(UTC)),
                        account=self.request.account,
                    )
                message = "CLI reported an execution blocker; inspect or cancel the invocation."
                if category == "quota":
                    message += " " + provider_health.QUOTA_RESUME_HINT
                if not finished:
                    self.raise_attention(InvocationError(code, message))
            if activity.kind == "message" and activity.public_output:
                # Delta events append; cumulative/final message events replace the current reply.
                if activity.message_delta and self.output_event_type == activity.event_type:
                    self.output_tail = (self.output_tail + activity.public_output)[-1000:]
                else:
                    self.output_tail = activity.public_output[-1000:]
                self.output_at = self.last_event_at
                self.output_event_type = activity.event_type
            previous = self.last_activity
            self._note_tool(activity, previous)
            if activity.kind == "tool" and activity.tool_call_id is not None:
                if activity.tool_name is not None:
                    self.tool_names[activity.tool_call_id] = activity.tool_name
                else:
                    activity = replace(
                        activity, tool_name=self.tool_names.get(activity.tool_call_id)
                    )
            if (
                activity.message_delta
                and previous is not None
                and previous.kind == "message"
                and previous.event_type == activity.event_type
            ):
                activity = replace(
                    activity, message=((previous.message or "") + (activity.message or ""))[-512:]
                )
            self.last_activity = activity
            self.last_activity_at = self.last_event_at
            self.update_doing(self.last_event_at)
            if activity.kind == "retry":
                self.retry_since = _retry_streak_start(
                    self.retry_since, self.retry_at, self.last_event_at
                )
                self.retry_at = self.last_event_at
            else:
                self.retry_since = None
                self.retry_at = None

    def _note_tool(self, activity: ProviderActivity, previous: ProviderActivity | None) -> None:
        """Remember the name of each tool call once, however many events report it."""
        if activity.kind != "tool" or not activity.tool_name:
            return
        if activity.tool_call_id is not None:
            if activity.tool_call_id in self.tool_names:
                return
        elif (activity.state or "").lower() in _ENDED_TOOL_STATES or (
            # Without an id, repeated updates of one call arrive back to back.
            previous is not None
            and previous.kind == "tool"
            and previous.tool_call_id is None
            and previous.tool_name == activity.tool_name
            and (previous.state or "").lower() not in _ENDED_TOOL_STATES
        ):
            return
        self.recent_tools.append(activity.tool_name)

    def update_doing(self, now: float) -> None:
        doing = None if self.prompts.pending() else _doing(self.last_activity)
        if doing != self.doing:
            self.doing, self.doing_since = doing, now if doing is not None else None

    def take_update(self) -> InvocationResult | InvocationError | NativePrompt | None:
        """Return the final result, else consume the oldest unread blocker."""
        if self.task is not None and self.task.done():
            return self.task.result()
        prompt = self.prompts.take()
        if prompt is not None:
            return prompt
        if not self.pending_attention:
            self.attention_event.clear()
            return None
        attention = self.pending_attention.popleft()
        if not self.pending_attention:
            self.attention_event.clear()
        return attention


@dataclass(frozen=True, slots=True)
class _CapturedStream:
    data: bytes
    exceeded: bool
    # Bytes read past the end of a kept process's turn; the process broke its contract.
    trailing: bytes = b""


def _retry_streak_start(since: float | None, last: float | None, now: float) -> float:
    """When the current run of retries began.

    A run that already went quiet is over: a retry arriving after more than the quiet window
    starts a new one, so a second unrelated hiccup is not counted as the first one continuing.
    """
    if since is None or (last is not None and now - last > _RETRY_STALL_QUIET_SECONDS):
        return now
    return since


async def _stall_guard(record: _InvocationRecord, interval: float = 15.0) -> None:
    """Complete once the provider has been retrying without output for too long."""
    while True:
        await asyncio.sleep(interval)
        if record.prompts.pending():
            record.retry_since = record.retry_at = None
            continue
        since, last = record.retry_since, record.retry_at
        if since is None or last is None:
            continue
        now = time.monotonic()
        if now - since >= _RETRY_STALL_SECONDS and now - last <= _RETRY_STALL_QUIET_SECONDS:
            return


@dataclass(slots=True)
class _StreamDiagnostics:
    """Bounded evidence that survives cancellation of a pipe reader during cleanup."""

    tail: bytes = b""
    leading_partial: bool = False
    total_bytes: int = 0
    exceeded_reason: str | None = None

    def observe(self, chunk: bytes) -> None:
        self.total_bytes += len(chunk)
        combined = self.tail + chunk
        if len(combined) > _DIAGNOSTIC_TAIL_BYTES:
            self.leading_partial = combined[-_DIAGNOSTIC_TAIL_BYTES - 1] != ord("\n")
        self.tail = combined[-_DIAGNOSTIC_TAIL_BYTES:]

    def safe_tail(self) -> str:
        tail = self.tail
        if self.leading_partial:
            # Never expose a credential's continuation without its identifying prefix.
            _, _, tail = tail.partition(b"\n")
        text = tail.decode(errors="replace")
        secrets = [
            value
            for key, value in os.environ.items()
            if value and re.search(r"TOKEN|SECRET|PASSWORD|PASSWD|KEY|CREDENTIAL|AUTH", key, re.I)
        ]
        lines: list[str] = []
        for line in text.splitlines():
            if _SENSITIVE_DIAGNOSTIC.search(line) or any(value in line for value in secrets):
                lines.append("[REDACTED]")
            else:
                lines.append("".join(char for char in line if char.isprintable()))
        return "\n".join(lines) or "[no complete diagnostic line retained]"


async def _background_reminder_guard(
    record: _InvocationRecord,
    threshold_seconds: float,
    notify: Callable[[BackgroundTasksState], bool],
) -> None:
    """Emit one background_tasks_pending blocker once waiting exceeds threshold_seconds."""
    while not record.background_wait.reminded and not record.background_reminder_sent:
        record.background_wait_changed.clear()
        remaining = record.background_wait.remaining_seconds(threshold_seconds)
        if remaining is None:
            await record.background_wait_changed.wait()
            continue
        if remaining <= 0:
            if state := record.background_wait.fire():
                notify(state)
            return
        try:
            await asyncio.wait_for(record.background_wait_changed.wait(), timeout=remaining)
        except TimeoutError:
            if state := record.background_wait.fire():
                notify(state)
            return


@dataclass
class _RunIO:
    stdout_task: asyncio.Task[_CapturedStream]
    # None for a keep-alive run, whose stderr is drained by its process for its whole life.
    stderr_task: asyncio.Task[_CapturedStream] | None
    stdin_task: asyncio.Task[None]
    completion_task: asyncio.Task[ProviderCompletion]
    cancel_task: asyncio.Task[bool]
    exceeded_task: asyncio.Task[bool]
    stall_task: asyncio.Task[None]
    terminal_task: asyncio.Task[None]
    bg_reminder_task: asyncio.Task[None]
    progress_task: asyncio.Task[None]
    output_exceeded: asyncio.Event
    stdout_diagnostics: _StreamDiagnostics
    stderr_diagnostics: _StreamDiagnostics
    kept: KeptProcess | None = None

    @property
    def tasks(self) -> tuple[asyncio.Task[object], ...]:
        """The tasks this run ends; a keep-alive process's exit watch outlives the run."""
        tasks: list[asyncio.Task[object] | None] = [
            self.stdout_task,
            self.stderr_task,
            self.stdin_task,
            None if self.kept is not None else self.completion_task,
            self.cancel_task,
            self.exceeded_task,
            self.stall_task,
            self.terminal_task,
            self.bg_reminder_task,
            self.progress_task,
        ]
        return tuple(task for task in tasks if task is not None)


async def _terminal_grace(event: asyncio.Event) -> None:
    await event.wait()
    await asyncio.sleep(_TERMINAL_GRACE_SECONDS)


async def _turn_end(event: asyncio.Event, stdout_task: asyncio.Task[_CapturedStream]) -> None:
    """A keep-alive turn ends once its terminal event is read: the reader stops right there."""
    await event.wait()
    await asyncio.wait({stdout_task})


def _retain_line(line: bytes) -> bytes:
    return line + b"\n"


def _drain_turn_stderr(
    kept: KeptProcess, diagnostics: _StreamDiagnostics, exceeded: asyncio.Event
) -> None:
    """Retain what a kept process writes to stderr during this turn, with the usual limit."""

    def over_limit() -> None:
        diagnostics.exceeded_reason = "capture_limit"
        exceeded.set()

    kept.stderr.begin(_MAX_STDERR_BYTES, diagnostics.observe, over_limit)


def _start_run_io(
    record: _InvocationRecord,
    adapter: ProviderAdapter,
    prepared: PreparedCommand,
    supervised: SupervisedProcess,
    bg_reminder_task: asyncio.Task[None],
    progress_task: asyncio.Task[None],
    kept: KeptProcess | None = None,
) -> _RunIO:
    """Start one run's readers, stdin writer and guards. A kept process runs one turn: its
    stdout is read up to the turn's terminal event and its stdin stays open (ADR-0120)."""
    process = supervised.process
    assert process.stdout is not None and process.stderr is not None and process.stdin is not None
    output_exceeded = asyncio.Event()
    stdout_diagnostics = _StreamDiagnostics()
    stderr_diagnostics = _StreamDiagnostics()
    stdout_task = asyncio.create_task(
        _read_limited(
            process.stdout,
            _MAX_STDOUT_BYTES,
            output_exceeded,
            lambda line: record.observe_event(line, adapter),
            prepared.capture_line or (_retain_line if kept is not None else None),
            prepared.skip_oversized_line,
            event_limit=_MAX_EVENT_BYTES,
            diagnostics=stdout_diagnostics,
            until=record.terminal_event if kept is not None else None,
        )
    )
    stderr_task = None
    if kept is not None:
        _drain_turn_stderr(kept, stderr_diagnostics, output_exceeded)
    else:
        stderr_task = asyncio.create_task(
            _read_limited(
                process.stderr,
                _MAX_STDERR_BYTES,
                output_exceeded,
                diagnostics=stderr_diagnostics,
            )
        )
    stdin_task = asyncio.create_task(
        _write_stdin(
            process.stdin,
            prepared.stdin,
            keep_open=prepared.keep_stdin_open or kept is not None,
        )
    )
    completion_task = supervised.completion
    cancel_task = asyncio.create_task(record.cancel_event.wait())
    exceeded_task = asyncio.create_task(output_exceeded.wait())
    stall_task = asyncio.create_task(_stall_guard(record))
    return _RunIO(
        stdout_task=stdout_task,
        stderr_task=stderr_task,
        stdin_task=stdin_task,
        completion_task=completion_task,
        cancel_task=cancel_task,
        exceeded_task=exceeded_task,
        stall_task=stall_task,
        terminal_task=asyncio.create_task(
            _terminal_grace(record.terminal_event)
            if kept is None
            else _turn_end(record.terminal_event, stdout_task)
        ),
        bg_reminder_task=bg_reminder_task,
        progress_task=progress_task,
        output_exceeded=output_exceeded,
        stdout_diagnostics=stdout_diagnostics,
        stderr_diagnostics=stderr_diagnostics,
        kept=kept,
    )


def _run_outcome(
    record: _InvocationRecord,
    io: _RunIO,
    done: Container[asyncio.Task[object]],
) -> tuple[InvocationStatus | None, InvocationError | None, int | None]:
    provider_returncode: int | None = None
    # A guard that fires in the same tick as a real signal must not mask it: the guard
    # only ends a run that would otherwise keep going. Cancellation and a provider that
    # already exited are outcomes, so they are settled before the stall error.
    if io.exceeded_task in done and io.output_exceeded.is_set():
        terminal = InvocationStatus.FAILED
        error = _OUTPUT_LIMIT_ERROR
    elif io.cancel_task in done and record.cancel_event.is_set():
        terminal = InvocationStatus.CANCELLED
        error = InvocationError("cancelled", "invocation was cancelled")
    elif io.completion_task in done:
        completion = io.completion_task.result()
        if completion.kind == "exit" and completion.returncode is not None:
            provider_returncode = completion.returncode
            terminal = None
            error = None
        else:
            terminal = InvocationStatus.FAILED
            error = InvocationError(
                ("launch_error" if completion.kind == "launch_error" else "supervision_lost"),
                completion.message or "provider supervisor failed",
            )
    elif io.terminal_task in done and io.stall_task not in done:
        terminal = error = None
        provider_returncode = 0
    else:
        # The stall guard is the only trigger left, and the provider had not exited:
        # stop it as the retry policy did before the guard existed.
        terminal = InvocationStatus.FAILED
        error = _RETRY_STALL_ERROR
    return terminal, error, provider_returncode


def _output_limit_error(
    stdout: _CapturedStream,
    stderr: _CapturedStream,
    stdout_diagnostics: _StreamDiagnostics,
    stderr_diagnostics: _StreamDiagnostics,
) -> InvocationError:
    details: list[str] = []
    for name, captured, diagnostic, limit in (
        ("stdout", stdout, stdout_diagnostics, _MAX_STDOUT_BYTES),
        ("stderr", stderr, stderr_diagnostics, _MAX_STDERR_BYTES),
    ):
        if not captured.exceeded and diagnostic.exceeded_reason is None:
            continue
        reason = diagnostic.exceeded_reason or "capture_limit"
        if reason == "event_limit":
            limit = _MAX_EVENT_BYTES
        details.append(
            f"stream={name} reason={reason} limit_bytes={limit} "
            f"observed_raw_bytes={diagnostic.total_bytes}\n"
            f"tail (credential lines redacted):\n{diagnostic.safe_tail()}"
        )
    return replace(_OUTPUT_LIMIT_ERROR, details="\n\n".join(details)[:_MAX_ERROR_DETAILS])


SUBAGENT_INSTRUCTIONS = (
    "Please handle this within your current permissions without bypassing them. Work on it "
    "yourself, without calling AgentNave tools or passing it on to someone else through "
    "AgentNave. In your final reply, let me know about any unmet requirements, unchecked scope, "
    "blockers, or additional permissions needed."
)
QUESTIONS_ALLOWED_INSTRUCTIONS = "If anything is unclear or needs my decision, feel free to ask me."
NO_QUESTIONS_INSTRUCTIONS = (
    "I won't be able to answer questions while you work, so don't use question tools or stop to "
    "ask—note anything uncertain or needing a decision at the end of your reply."
)
DERIVED_INSTRUCTIONS = (
    "Please handle this within your current permissions without bypassing them. You can use "
    "AgentNave to run background tasks or hold meetings if needed, but don't open another "
    "terminal session through it. If anything is unclear, feel free to ask me here. In your "
    "final reply, let me know about any unmet requirements, unchecked scope, blockers, "
    "decisions, or additional permissions needed."
)
READ_ONLY_INSTRUCTIONS = (
    "This is a read-only task, so please leave files unchanged and don't commit, push, send "
    "messages, or post comments. Feel free to read files and run read-only commands and tests, "
    "including any caches and temporary files they create."
)
TASK_SEPARATOR = "\n\nHere's the task:\n"


class InvocationManager:
    """Own active child processes only for this Python process lifetime."""

    spawn_interval: tuple[float, float] = (1.0, 3.0)
    background_wait_reminder_seconds: float = BACKGROUND_WAIT_REMINDER_SECONDS
    progress_stall_seconds: float = PROGRESS_STALL_SECONDS
    keep_seconds: float = KEEP_SECONDS
    max_kept: int = MAX_KEPT

    def __init__(self) -> None:
        self._records: dict[str, _InvocationRecord] = {}
        # Processes kept between turns; they are not runs, so no running limit counts them.
        self._pool = WarmPool(self.keep_seconds, self.max_kept)
        self._seq = 0
        self._closed = False
        self.on_forget: Callable[[str], None] | None = None
        self.on_prompt_change: Callable[[str], None] | None = None
        self.on_inbox_change: Callable[[], None] | None = None
        # Lock directory shared by every server on one data root; None claims in-process only.
        self.session_locks: Path | None = None
        self.health_root: Path | None = None
        self._spawn_locks: dict[str, asyncio.Lock] = {}
        self._last_spawn: dict[str, float] = {}
        self.terminal_runs: dict[str, TerminalRun] = {}
        self.terminal_reconcilers: set[asyncio.Task[None]] = set()
        self.settled_requests: list[dict[str, object]] = []

    def _next_seq(self) -> int:
        self._seq += 1
        return self._seq

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        """Apply the shared task instructions before provider-specific encoding."""
        adapter = get_adapter(request.provider)
        question_guidance = (
            QUESTIONS_ALLOWED_INSTRUCTIONS
            if adapter.can_answer_questions(request)
            else NO_QUESTIONS_INSTRUCTIONS
        )
        instructions = f"{SUBAGENT_INSTRUCTIONS} {question_guidance}"
        if request.read_only:
            instructions += f" {READ_ONLY_INSTRUCTIONS}"
        if request.extra_instructions:
            instructions += f" {request.extra_instructions}"
        prepared = adapter.prepare(
            replace(request, prompt=f"{instructions}{TASK_SEPARATOR}{request.prompt}")
        )
        # A named account's directory goes to the child alone, through the whole command chain
        # (the permission-mode fallback and the interactive runtime run inside it); this
        # process's own environment is never touched.
        if account_env := accounts.env_for(adapter, request.account):
            return replace(prepared, extra_env={**prepared.extra_env, **account_env})
        return prepared

    def start(
        self,
        request: InvocationRequest,
        *,
        prepared: PreparedCommand | None = None,
        claim: IO[bytes] | None = None,
        on_settled: Callable[[str, InvocationStatus], None] | None = None,
        terminal: TerminalLaunch | None = None,
        isolated: bool = False,
    ) -> str:
        """Start one run; it owns claim (a reserve() result, taken here when omitted) from this
        call on and releases it once its process tree has ended. An isolated run (in an
        isolated dir) never keeps its process (ADR-0120)."""
        if claim is None:
            claim = self.reserve(request)
        try:
            adapter = get_adapter(request.provider)
            prepared = prepared if prepared is not None else self.prepare(request)
            invocation_id = str(uuid.uuid4())
            record = _InvocationRecord(
                request,
                health_root=self.health_root,
                start_seq=self._next_seq(),
                session_id=request.session_id or prepared.session_id,
                on_inbox_change=self.on_inbox_change,
                isolated=isolated,
            )
            if prepared.keep_stdin_open:
                record.prompts.encode = adapter.encode_prompt_answer
                record.prompts.changed = lambda: self._prompts_changed(invocation_id)
            self._records[invocation_id] = record
            if terminal is None:
                record.task = asyncio.create_task(
                    self._execute(record, adapter, prepared, invocation_id, on_settled)
                )
            else:

                def started(item: dict[str, object]) -> None:
                    record.phase = InvocationPhase.RUNNING
                    session_id = item.get("session_id")
                    if isinstance(session_id, str) and session_id:
                        record.session_id = session_id

                def blocked(code: str, message: str) -> None:
                    if code == BACKGROUND_TASKS_PENDING:
                        # Same once-per-dispatch guard as helpers; never in reported_errors.
                        if record.background_reminder_sent:
                            return
                        record.background_reminder_sent = True
                    elif code in record.reported_errors:
                        return
                    else:
                        record.reported_errors.add(code)
                    record.raise_attention(InvocationError(code, message))

                instructions = DERIVED_INSTRUCTIONS
                if request.read_only:
                    instructions += f" {READ_ONLY_INSTRUCTIONS}"
                if request.extra_instructions:
                    instructions += f" {request.extra_instructions}"
                run = TerminalRun(
                    terminal,
                    request,
                    invocation_id,
                    f"{instructions}{TASK_SEPARATOR}{request.prompt}",
                    lambda _item: None,
                    started,
                    self.ack_terminal_request,
                    on_blocked=blocked,
                    session_id=request.session_id,
                )
                self.terminal_runs[invocation_id] = run
                record.task = asyncio.create_task(
                    execute_run(
                        run,
                        lambda result: self._settle_session(record, result),
                        self.terminal_reconcilers,
                        on_settled,
                    )
                )
        except BaseException:
            if claim is not None:
                release_and_remove(claim, Path(claim.name))
            raise
        if claim is not None:
            held = claim
            record.task.add_done_callback(lambda _: release_and_remove(held, Path(held.name)))
        record.task.add_done_callback(lambda _: self._schedule_cleanup(invocation_id))
        if self.on_inbox_change is not None:
            record.task.add_done_callback(lambda _: self.on_inbox_change and self.on_inbox_change())
        return invocation_id

    def reserve(self, request: InvocationRequest) -> IO[bytes] | None:
        """Claim the request's native session for one run, across every server sharing the
        lock directory; release_and_remove removes it once the run ends."""
        self.check_start(request)
        if request.session_id is None or self.session_locks is None:
            return None
        identity = json.dumps([request.provider, request.session_id]).encode()
        self.session_locks.mkdir(parents=True, mode=0o700, exist_ok=True)
        return lock_file(
            self.session_locks / (hashlib.sha256(identity).hexdigest() + ".lock"),
            "This native session already has an active invocation in another AgentNave "
            "server; wait for it or cancel it there",
        )

    def active_ids(self) -> set[str]:
        """CLI runs of this server still in progress: private tasks and seats alike."""
        return {
            invocation_id
            for invocation_id, record in self._records.items()
            if record.task is not None and not record.task.done()
        }

    def check_start(self, request: InvocationRequest) -> None:
        if self._closed:
            raise RuntimeError("invocation manager is closed")
        if request.session_id is not None and any(
            record.request.provider == request.provider
            and (
                record.request.session_id == request.session_id
                or record.session_id == request.session_id
            )
            and record.task is not None
            and not record.task.done()
            for record in self._records.values()
        ):
            raise ValueError(
                "This native session already has an active invocation; wait or cancel it"
            )

    def _schedule_cleanup(self, invocation_id: str) -> None:
        record = self._records.get(invocation_id)
        if self._closed or record is None:
            return
        record.cleanup_handle = asyncio.get_running_loop().call_later(
            _FINISHED_RETENTION_SECONDS, self.forget_finished, {invocation_id}
        )

    async def wait(
        self, invocation_id: str, timeout_seconds: float | None = None
    ) -> InvocationResult | None:
        record = self._record(invocation_id)
        if record.task is None:
            raise RuntimeError("invocation task was not initialized")
        try:
            async with asyncio.timeout(timeout_seconds):
                return await asyncio.shield(record.task)
        except TimeoutError:
            return None

    async def wait_for_update(
        self, invocation_id: str, timeout_seconds: float
    ) -> InvocationResult | InvocationError | NativePrompt | None:
        updates = await self.wait_for_any((invocation_id,), timeout_seconds)
        return updates.get(invocation_id)

    async def wait_for_any(
        self, invocation_ids: Sequence[str], timeout_seconds: float
    ) -> dict[str, InvocationResult | InvocationError | NativePrompt]:
        """Return every listed invocation that has finished or has an unread blocker.

        Each scan runs without awaiting, so only blockers placed in the result are consumed.
        An empty result means the wait elapsed.
        """
        if not invocation_ids:
            raise ValueError("invocation_ids must not be empty")
        if len(set(invocation_ids)) != len(invocation_ids):
            raise ValueError("invocation_ids must not repeat")
        records = {invocation_id: self._record(invocation_id) for invocation_id in invocation_ids}
        tasks: list[asyncio.Task[InvocationResult]] = []
        for record in records.values():
            if record.task is None:
                raise RuntimeError("invocation task was not initialized")
            tasks.append(record.task)
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout_seconds
        while True:
            updates: dict[str, InvocationResult | InvocationError | NativePrompt] = {}
            for invocation_id, record in records.items():
                update = record.take_update()
                if update is not None:
                    updates[invocation_id] = update
                    record.result_read |= isinstance(update, InvocationResult)
            remaining = deadline - loop.time()
            if updates or remaining <= 0:
                return updates
            waiters = [
                asyncio.create_task(record.attention_event.wait()) for record in records.values()
            ]
            try:
                done, _ = await asyncio.wait(
                    (*tasks, *waiters), timeout=remaining, return_when=asyncio.FIRST_COMPLETED
                )
            finally:
                for waiter in waiters:
                    waiter.cancel()
                await asyncio.gather(*waiters, return_exceptions=True)
            # An expired window returns snapshots: a blocker landing now stays unread.
            if not done or loop.time() >= deadline:
                return {}

    def list_invocations(self) -> list[InvocationListing]:
        listings: list[InvocationListing] = []
        for invocation_id, record in self._records.items():
            result = (
                record.task.result() if record.task is not None and record.task.done() else None
            )
            listings.append(
                InvocationListing(
                    invocation_id,
                    record.request.provider,
                    record.request.cwd,
                    _duration_ms(record.started_at) if result is None else result.duration_ms,
                    record.last_event_at is not None,
                    phase=record.phase if result is None else None,
                    status=None if result is None else result.status,
                    session_id=None if result is None else result.session_id,
                    result_read=record.result_read,
                    error=None if result is None else result.error,
                    read_only=record.request.read_only,
                )
            )
        return listings

    def snapshot(self, invocation_id: str) -> InvocationSnapshot:
        record = self._record(invocation_id)
        now = time.monotonic()
        last_event_age_ms = (
            None
            if record.last_event_at is None
            else max(0, round((now - record.last_event_at) * 1000))
        )
        return InvocationSnapshot(
            record.phase,
            round((now - record.started_at) * 1000),
            last_event_age_ms,
            record.last_activity,
            None
            if record.last_activity_at is None
            else max(0, round((now - record.last_activity_at) * 1000)),
        )

    def unread_blockers(self, invocation_id: str) -> list[str]:
        """Codes of blockers no wait has returned yet, oldest first; reading them consumes none."""
        return [attention.code for attention in self._record(invocation_id).pending_attention]

    def notify_background_tasks_pending(
        self,
        invocation_id: str,
        *,
        task_ids: Sequence[str] = (),
        count: int | None = None,
    ) -> bool:
        """Queue the one-shot background_tasks_pending execution_blocked alert (ADR-0114).

        Shared by interactive helpers and derived-terminal push receipt bridges. Never feeds
        account health classification or cooldown, never includes reply body, and leaves the
        invocation running so a terminal outcome delivers directly if it arrives first.
        """
        record = self._records.get(invocation_id)
        if record is None or record.background_reminder_sent or not record.accepts_reminders():
            return False
        record.background_reminder_sent = True
        record.background_wait.reminded = True
        record.raise_attention(background_tasks_pending_error(task_ids, count=count))
        return True

    def notify_progress_stalled(self, invocation_id: str, quiet_seconds: float) -> bool:
        """Queue the one-shot progress_stalled execution_blocked alert (ADR-0116).

        Like the background reminder it never feeds account health or cooldown, is never
        recorded in reported_errors, and leaves the invocation running.
        """
        record = self._records.get(invocation_id)
        if record is None or not record.accepts_reminders():
            return False
        record.update_doing(time.monotonic())
        last_tool = record.recent_tools[-1] if record.recent_tools else None
        record.raise_attention(progress_stalled_error(quiet_seconds, record.doing, last_tool))
        return True

    def progress(self, invocation_id: str) -> tuple[str | None, str | None]:
        """What the CLI is doing now and when its reply last grew, as labels that change only
        with the work itself: thinking, tool:<name>, writing or retrying, and a wall time."""
        record = self._record(invocation_id)
        record.update_doing(time.monotonic())
        output_at: str | None = None
        if record.output_at is not None:
            wall = record.started_wall_at + timedelta(seconds=record.output_at - record.started_at)
            output_at = wall.replace(microsecond=0).isoformat()
        return record.doing, output_at

    def recent_tools(self, invocation_id: str) -> list[str]:
        return list(self._record(invocation_id).recent_tools)

    def progress_since(self, invocation_id: str) -> str | None:
        record = self._record(invocation_id)
        if record.doing_since is None:
            return None
        wall = record.started_wall_at + timedelta(seconds=record.doing_since - record.started_at)
        return wall.isoformat()

    def recent_output(self, invocation_id: str) -> tuple[str, int | None]:
        record = self._record(invocation_id)
        age = (
            None
            if record.output_at is None
            else max(0, round((time.monotonic() - record.output_at) * 1000))
        )
        return record.output_tail, age

    def reported_model(self, invocation_id: str) -> str | None:
        record = self._records.get(invocation_id)
        return record.reported_model if record is not None else None

    def _prompts_changed(self, invocation_id: str) -> None:
        if self.on_inbox_change is not None:
            self.on_inbox_change()
        record = self._record(invocation_id)
        # A prompt opening, closing or being answered pauses the silence clock; the guard only
        # polls, so a pause shorter than its interval must restart the clock here.
        record.progress.hold()
        record.update_doing(time.monotonic())
        if self.on_prompt_change is not None:
            self.on_prompt_change(invocation_id)

    def pending_prompts(self, invocation_id: str) -> list[NativePrompt]:
        return self._record(invocation_id).prompts.pending()

    def prompt_history(self, invocation_id: str) -> list[dict[str, object]]:
        return [
            {"invocation_id": invocation_id, **prompt.history()}
            for prompt in self._record(invocation_id).prompts.prompts.values()
        ]

    async def answer_prompt(
        self,
        invocation_id: str,
        prompt_id: str,
        decision: str | None = None,
        reason: str | None = None,
        answers: dict[str, str] | None = None,
    ) -> NativePrompt:
        record = self._record(invocation_id)
        if record.task is None or record.task.done() or record.phase == InvocationPhase.STOPPING:
            raise ValueError("invocation has ended or is stopping; its prompts are closed")
        return await record.prompts.answer(prompt_id, decision, reason, answers)

    async def cancel(self, invocation_id: str) -> InvocationResult:
        record = self._record(invocation_id)
        if (
            (run := self.terminal_runs.get(invocation_id)) is not None
            and record.task is not None
            and not record.task.done()
        ):
            outcome = await run.cancel()
            if outcome == "not_safe_to_interrupt":
                raise ValueError(
                    "Terminal could not safely identify the requested turn; it was not interrupted"
                )
            result = await self.wait(invocation_id)
            if result is None:
                raise RuntimeError("terminal invocation did not terminate")
            record.result_read = True
            return result
        record.phase = InvocationPhase.STOPPING
        for prompt in record.prompts.pending():
            try:
                await record.prompts.answer(
                    prompt.prompt_id, "deny", "Invocation cancelled", None, cancel=True
                )
            except (OSError, ValueError):
                logging.getLogger(__name__).exception(
                    "Could not deliver native prompt cancellation"
                )
        record.cancel_event.set()
        result = await self.wait(invocation_id)
        if result is None:
            raise RuntimeError("cancelled invocation did not terminate")
        record.result_read = True
        return result

    async def shutdown(self) -> None:
        if self._closed:
            return
        self._closed = True
        for record in self._records.values():
            if record.cleanup_handle is not None:
                record.cleanup_handle.cancel()
        active = [
            invocation_id
            for invocation_id, record in self._records.items()
            if record.task is not None and not record.task.done()
        ]
        for run in self.terminal_runs.values():
            run.stop.set()
        await asyncio.gather(
            *(
                self.wait(item) if item in self.terminal_runs else self.cancel(item)
                for item in active
            ),
            return_exceptions=True,
        )
        await asyncio.gather(*self.terminal_reconcilers, return_exceptions=True)
        await self._pool.close()

    def ack_terminal_request(self, request_id: str, tray: TrayIdentity) -> None:
        ack: dict[str, object] = {
            "tray_pid": tray.pid,
            "tray_started_at": tray.started_at,
            "request_id": request_id,
        }
        if ack not in self.settled_requests:
            self.settled_requests.append(ack)
            if self.on_inbox_change:
                self.on_inbox_change()

    def forget_finished(self, invocation_ids: set[str]) -> int:
        removed = 0
        for invocation_id in invocation_ids:
            record = self._records.get(invocation_id)
            if record is not None and record.task is not None and record.task.done():
                if record.cleanup_handle is not None:
                    record.cleanup_handle.cancel()
                del self._records[invocation_id]
                if terminal_run := self.terminal_runs.pop(invocation_id, None):
                    terminal_run.stop.set()
                if self.on_forget is not None:
                    self.on_forget(invocation_id)
                removed += 1
        return removed

    def _record(self, invocation_id: str) -> _InvocationRecord:
        try:
            return self._records[invocation_id]
        except KeyError as exc:
            raise KeyError(f"unknown invocation_id: {invocation_id}") from exc

    def _has_overlapping_session(self, record: _InvocationRecord, session_id: str) -> bool:
        provider = record.request.provider
        for other in self._records.values():
            if other is record or other.request.provider != provider:
                continue
            if other.session_id != session_id:
                continue
            if other.end_seq is None or other.end_seq > record.start_seq:
                return True
        return False

    def _settle_session(
        self, record: _InvocationRecord, result: InvocationResult
    ) -> InvocationResult:
        """Reject a task whose CLI returned another concurrent task's native session."""
        record.end_seq = self._next_seq()
        session_id = result.session_id
        if (
            session_id
            and session_id != record.request.session_id
            and self._has_overlapping_session(record, session_id)
        ):
            record.session_id = None
            if result.status in (InvocationStatus.FAILED, InvocationStatus.CANCELLED):
                return replace(result, session_id=None)
            return replace(
                result,
                status=InvocationStatus.FAILED,
                session_id=None,
                error=_FOREIGN_SESSION_ERROR,
            )
        if result.status is not InvocationStatus.CANCELLED:
            record.session_id = session_id or record.request.session_id
        return result

    async def _execute(
        self,
        record: _InvocationRecord,
        adapter: ProviderAdapter,
        prepared: PreparedCommand,
        invocation_id: str,
        on_settled: Callable[[str, InvocationStatus], None] | None = None,
    ) -> InvocationResult:
        outcome = InvocationStatus.FAILED
        try:
            result = await self._run(record, adapter, prepared, invocation_id)
            result = self._settle_session(record, result)
            outcome = result.status
            await self._keep_or_end(record, result)
            reported = record.reported_model
            if (
                not reported
                and not prepared.model_from_result_only
                and getattr(adapter.session_model, "__func__", None)
                is not AdapterHooks.session_model
            ):
                reported = await asyncio.to_thread(
                    adapter.session_model, result.session_id, since=record.started_wall_at
                )
            record.reported_model = reported
            return replace(result, reported_model=reported)
        except Exception:
            record.end_seq = record.end_seq or self._next_seq()
            logging.getLogger(__name__).exception("Unexpected invocation failure")
            outcome = InvocationStatus.FAILED
            return InvocationResult(
                InvocationStatus.FAILED,
                record.request.provider,
                "",
                record.request.session_id,
                _duration_ms(record.started_at),
                InvocationError(
                    "internal_error", "Unexpected invocation failure; inspect server logs."
                ),
            )
        finally:
            await self._keep_or_end(record, None)
            if on_settled is not None:
                try:
                    # Off the event loop: settlement runs git and takes repository locks.
                    await asyncio.shield(asyncio.to_thread(on_settled, invocation_id, outcome))
                except Exception:
                    logging.getLogger(__name__).exception("Post-invocation settlement failed")

    async def _keep_or_end(
        self, record: _InvocationRecord, result: InvocationResult | None
    ) -> None:
        """Keep the process of a turn that succeeded for its session's next resume; a cancelled,
        failed or blocked run's process ends now, as every other run's does (ADR-0120)."""
        kept, record.kept = record.kept, None
        if kept is None:
            return
        if (
            result is not None
            and result.status is InvocationStatus.SUCCEEDED
            and result.session_id
            and not record.cancel_event.is_set()
            and not self._closed
            and kept.alive()
        ):
            self._pool.keep(record.request.provider, result.session_id, kept)
        else:
            await kept.end()

    async def _observe(
        self,
        record: _InvocationRecord,
        adapter: ProviderAdapter,
        result: InvocationResult,
        error_message: str | None,
        stdout: bytes,
        stderr: bytes,
    ) -> InvocationResult:
        """Record the result's account blocker; a quota one with no reset time asks the CLI."""
        model = provider_health.model_option(record.request.provider_options)
        now = datetime.now(UTC)
        reset = adapter.quota_reset(stdout, stderr, now)
        if (
            reset is None
            and self.health_root is not None
            and provider_health.blocker(result, error_message, record.reported_errors) == "quota"
        ):
            query = adapter.usage_query(model)
            if query is not None:
                try:
                    # The query asks the same account that ran out of quota.
                    env = accounts.env_for(adapter, record.request.account)
                except ValueError:
                    env = None  # the account was unregistered meanwhile: no answer to ask for
                if env is not None:
                    reset = await run_usage_query(query, now, env)
        return provider_health.observe_result(
            self.health_root,
            result,
            error_message,
            record.reported_errors,
            model,
            reset,
            account=record.request.account,
        )

    async def _run(
        self,
        record: _InvocationRecord,
        adapter: ProviderAdapter,
        prepared: PreparedCommand,
        invocation_id: str,
    ) -> InvocationResult:
        started = record.started_at
        supervised = None
        kept = None
        tasks: list[asyncio.Task[object]] = []
        scratch = None
        try:
            if prepared.temp_dir_env:
                scratch = tempfile.mkdtemp(prefix="agentnave-invocation-")
            launched = await self._launch(record, prepared, scratch)
            if launched is None:
                return _cancelled_before_spawn(record, prepared)
            supervised, kept = launched
            process = supervised.process
            record.process = process
            if prepared.keep_stdin_open:
                record.prompts.writer = process.stdin
            record.phase = InvocationPhase.RUNNING
            if process.stdout is None or process.stderr is None or process.stdin is None:
                raise RuntimeError("provider subprocess pipes were not created")

            bg_reminder_task = asyncio.create_task(
                _background_reminder_guard(
                    record,
                    self.background_wait_reminder_seconds,
                    lambda state: self.notify_background_tasks_pending(
                        invocation_id, task_ids=state.task_ids
                    ),
                )
            )
            progress_task = asyncio.create_task(
                progress_stall_guard(
                    record.progress,
                    self.progress_stall_seconds,
                    record.counts_silence,
                    lambda quiet: self.notify_progress_stalled(invocation_id, quiet),
                )
            )
            io = _start_run_io(
                record, adapter, prepared, supervised, bg_reminder_task, progress_task, kept
            )
            tasks.extend(io.tasks)

            return await self._collect_run(record, adapter, prepared, process, io)
        except (OSError, ValueError) as exc:
            return InvocationResult(
                InvocationStatus.FAILED,
                record.request.provider,
                "",
                record.request.session_id,
                _duration_ms(started),
                InvocationError("launch_error", str(exc)),
            )
        finally:
            try:
                try:
                    if record.kept is not None:
                        pass  # the run's outcome decides whether the process is kept
                    elif kept is not None:
                        await kept.end()
                    elif supervised is not None and supervised.process.returncode is None:
                        await terminate_process_tree(
                            supervised.process, completion=supervised.completion
                        )
                finally:
                    try:
                        for task in tasks:
                            if not task.done():
                                task.cancel()
                        if tasks:
                            await asyncio.gather(*tasks, return_exceptions=True)
                    finally:
                        for path in prepared.cleanup_paths:
                            Path(path).unlink(missing_ok=True)
            finally:
                try:
                    if scratch is not None:
                        # Best effort: a file still held open on Windows must not fail the call.
                        shutil.rmtree(scratch, ignore_errors=True)
                finally:
                    record.process = None
                    record.tool_names.clear()
                    record.prompts.close()

    async def _launch(
        self, record: _InvocationRecord, prepared: PreparedCommand, scratch: str | None
    ) -> tuple[SupervisedProcess, KeptProcess | None] | None:
        """The session's kept process when it was started the same way, else a new process,
        held as kept when the Adapter declared it can serve later turns. None: cancelled first."""
        request = record.request
        signature = launch_signature(prepared, request.account)
        keepable = can_keep(prepared) and not record.isolated
        if record.cancel_event.is_set():
            return None
        if request.session_id is not None:
            kept = self._pool.take(request.provider, request.session_id)
            if kept is not None:
                if keepable and kept.signature == signature and kept.alive():
                    return kept.supervised, kept
                # Started differently: it must not share the native session with a new process.
                await kept.end()
            await self._pool.ended(request.provider, request.session_id)
        if not await self._wait_to_spawn(record):
            return None
        try:
            if record.cancel_event.is_set():
                return None
            supervised = await spawn_process(
                prepared.argv,
                prepared.cwd,
                unset_env=prepared.unset_env,
                extra_env={
                    **prepared.extra_env,
                    **(
                        {prepared.temp_dir_env: scratch}
                        if prepared.temp_dir_env and scratch
                        else {}
                    ),
                },
            )
            self._last_spawn[request.provider] = time.monotonic()
        finally:
            self._spawn_locks[request.provider].release()
        if not keepable:
            return supervised, None
        return supervised, KeptProcess.adopt(supervised, signature)

    async def _collect_run(
        self,
        record: _InvocationRecord,
        adapter: ProviderAdapter,
        prepared: PreparedCommand,
        process: asyncio.subprocess.Process,
        io: _RunIO,
    ) -> InvocationResult:
        started = record.started_at
        done, _ = await asyncio.wait(
            (
                io.completion_task,
                io.cancel_task,
                io.exceeded_task,
                io.stall_task,
                io.terminal_task,
            ),
            return_when=asyncio.FIRST_COMPLETED,
        )
        terminal, error, provider_returncode = _run_outcome(record, io, done)
        bg_cancel_reply = record.background_reply if record.background_only else None
        bg_cancel_tasks = record.background_wait.task_ids

        record.phase = InvocationPhase.STOPPING
        for guard in (io.bg_reminder_task, io.progress_task):
            if not guard.done():
                guard.cancel()
        stdout, stderr = await _end_streams(record, process, io, done, turn_ended=terminal is None)
        terminal, error = _drained_limit(terminal, error, stdout, stderr, io)
        parse_returncode = provider_returncode
        if parse_returncode is None:
            parse_returncode = process.returncode if process.returncode is not None else 1
        parsed = adapter.parse_prepared(prepared, parse_returncode, stdout.data, stderr.data)

        # Without a final answer, an interrupted run keeps the reply text seen so far.
        partial = parsed.output or record.output_tail
        if terminal is InvocationStatus.CANCELLED and bg_cancel_reply is not None:
            # Only background tasks remained: settle directly with the reply already written,
            # noting that the helper process group was terminated (ADR-0114 section 4).
            result = InvocationResult(
                InvocationStatus.SUCCEEDED,
                record.request.provider,
                bg_cancel_reply,
                parsed.session_id
                or record.session_id
                or prepared.session_id
                or record.request.session_id,
                _duration_ms(started),
                warnings=(*parsed.warnings, background_cancel_warning(bg_cancel_tasks)),
            )
            return await self._observe(record, adapter, result, None, stdout.data, stderr.data)
        if terminal is not None:
            result = InvocationResult(
                terminal,
                record.request.provider,
                partial,
                parsed.session_id
                or record.session_id
                or prepared.session_id
                or record.request.session_id,
                _duration_ms(started),
                error,
            )
            return await self._observe(
                record, adapter, result, parsed.error_message, stdout.data, stderr.data
            )

        if provider_returncode is None:
            raise RuntimeError("provider completion did not include an exit status")
        parsed_error = None
        if parsed.error_message is not None:
            details = stderr.data.decode(errors="replace")[-_MAX_ERROR_DETAILS:] or None
            parsed_error = InvocationError("provider_error", parsed.error_message, details)
        result = InvocationResult(
            parsed.status,
            record.request.provider,
            parsed.output if parsed.status is InvocationStatus.SUCCEEDED else partial,
            parsed.session_id or prepared.session_id,
            _duration_ms(started),
            parsed_error,
            warnings=parsed.warnings,
        )
        return await self._observe(
            record, adapter, result, parsed.error_message, stdout.data, stderr.data
        )

    async def _wait_to_spawn(self, record: _InvocationRecord) -> bool:
        """Hold the provider lock through spawn; cancellation never reserves a slot."""
        provider = record.request.provider
        lock = self._spawn_locks.setdefault(provider, asyncio.Lock())
        acquire = asyncio.create_task(lock.acquire())
        cancelled = asyncio.create_task(record.cancel_event.wait())
        handed_off = False
        try:
            await asyncio.wait((acquire, cancelled), return_when=asyncio.FIRST_COMPLETED)
            if record.cancel_event.is_set():
                return False
            last = self._last_spawn.get(provider)
            if last is not None:
                delay = max(0.0, last + random.uniform(*self.spawn_interval) - time.monotonic())
                if delay:
                    with suppress(TimeoutError):
                        await asyncio.wait_for(record.cancel_event.wait(), delay)
            if record.cancel_event.is_set():
                return False
            handed_off = True  # The caller owns the lock through spawn.
            return True
        finally:
            if not acquire.done():
                acquire.cancel()
            cancelled.cancel()
            await asyncio.gather(acquire, cancelled, return_exceptions=True)
            if not handed_off and not acquire.cancelled() and acquire.result():
                lock.release()


def _cancelled_before_spawn(
    record: _InvocationRecord, prepared: PreparedCommand
) -> InvocationResult:
    return InvocationResult(
        InvocationStatus.CANCELLED,
        record.request.provider,
        "",
        prepared.session_id or record.request.session_id,
        _duration_ms(record.started_at),
        InvocationError("cancelled", "invocation was cancelled"),
    )


def _drained_limit(
    terminal: InvocationStatus | None,
    error: InvocationError | None,
    stdout: _CapturedStream,
    stderr: _CapturedStream,
    io: _RunIO,
) -> tuple[InvocationStatus | None, InvocationError | None]:
    # The provider may exit before the readers notice the limit while draining.
    if terminal is None and (
        stdout.exceeded
        or stderr.exceeded
        or io.stdout_diagnostics.exceeded_reason is not None
        or io.stderr_diagnostics.exceeded_reason is not None
    ):
        terminal, error = InvocationStatus.FAILED, _OUTPUT_LIMIT_ERROR
    if error is _OUTPUT_LIMIT_ERROR:
        error = _output_limit_error(stdout, stderr, io.stdout_diagnostics, io.stderr_diagnostics)
    return terminal, error


async def _read_limited(
    stream: asyncio.StreamReader,
    limit: int,
    exceeded_event: asyncio.Event,
    observe_event: Callable[[bytes], None] | None = None,
    capture_line: Callable[[bytes], bytes] | None = None,
    skip_oversized_line: Callable[[bytes], bool] | None = None,
    *,
    event_limit: int | None = None,
    diagnostics: _StreamDiagnostics | None = None,
    until: asyncio.Event | None = None,
) -> _CapturedStream:
    """Read a stream to its end, or, with until, to the line that sets it (a kept process's
    turn end, which needs capture_line)."""
    diagnostics = diagnostics if diagnostics is not None else _StreamDiagnostics()
    if capture_line is not None:
        return await _read_filtered(
            stream,
            limit,
            exceeded_event,
            observe_event,
            capture_line,
            skip_oversized_line,
            limit if event_limit is None else event_limit,
            diagnostics,
            until,
        )
    data = bytearray()
    pending = bytearray()
    exceeded = False
    while chunk := await stream.read(64 * 1024):
        diagnostics.observe(chunk)
        if observe_event is not None:
            pending.extend(chunk[: max(0, limit - len(data))])
            while b"\n" in pending:
                line, _, remainder = pending.partition(b"\n")
                pending = bytearray(remainder)
                if line.strip():
                    observe_event(bytes(line))
        remaining = limit - len(data)
        if remaining > 0:
            data.extend(chunk[:remaining])
        if len(chunk) > remaining:
            exceeded = True
            diagnostics.exceeded_reason = "capture_limit"
            exceeded_event.set()
    if observe_event is not None and pending.strip():
        observe_event(bytes(pending))
    return _CapturedStream(bytes(data), exceeded)


async def _read_filtered(
    stream: asyncio.StreamReader,
    limit: int,
    exceeded_event: asyncio.Event,
    observe_event: Callable[[bytes], None] | None,
    capture_line: Callable[[bytes], bytes],
    skip_oversized_line: Callable[[bytes], bool] | None,
    event_limit: int,
    diagnostics: _StreamDiagnostics,
    until: asyncio.Event | None = None,
) -> _CapturedStream:
    data = bytearray()
    pending = bytearray()
    exceeded = False
    discarding = False

    def skippable(line: bytes | bytearray) -> bool:
        return skip_oversized_line is not None and skip_oversized_line(bytes(line[:256]))

    def capture(line: bytes) -> None:
        nonlocal exceeded
        if observe_event is not None and line.strip():
            observe_event(line)
        retained = capture_line(line)
        remaining = limit - len(data)
        data.extend(retained[:remaining])
        if len(retained) > remaining:
            exceeded = True
            diagnostics.exceeded_reason = "capture_limit"

    while chunk := await stream.read(64 * 1024):
        diagnostics.observe(chunk)
        if exceeded:
            continue
        pending.extend(chunk)
        while b"\n" in pending:
            line, _, remainder = pending.partition(b"\n")
            pending = bytearray(remainder)
            if discarding:
                discarding = False
            elif len(line) > event_limit:
                exceeded = not skippable(line)
                if exceeded:
                    diagnostics.exceeded_reason = "event_limit"
            else:
                capture(bytes(line))
            if exceeded:
                break
            if until is not None and until.is_set():
                return _CapturedStream(bytes(data), False, bytes(pending))
        # Bound a single unterminated event independently of retained result bytes.
        if len(pending) > event_limit:
            # Once discarding, pending holds only the tail of the line being dropped.
            if discarding or skippable(pending):
                discarding = True
                pending.clear()
            else:
                exceeded = True
                diagnostics.exceeded_reason = "event_limit"
        if exceeded:
            pending.clear()
            exceeded_event.set()
    if pending.strip() and not exceeded and not discarding:
        capture(bytes(pending))
        if exceeded:
            exceeded_event.set()
    return _CapturedStream(bytes(data), exceeded)


async def _write_stdin(
    writer: asyncio.StreamWriter, data: bytes | None, *, keep_open: bool = False
) -> None:
    try:
        if data is not None:
            writer.write(data)
            await writer.drain()
    except (BrokenPipeError, ConnectionResetError):
        pass
    finally:
        if not keep_open:
            writer.close()
            with suppress(BrokenPipeError, ConnectionResetError):
                await writer.wait_closed()


async def _end_streams(
    record: _InvocationRecord,
    process: asyncio.subprocess.Process,
    io: _RunIO,
    done: Container[asyncio.Task[object]],
    *,
    turn_ended: bool,
) -> tuple[_CapturedStream, _CapturedStream]:
    """A kept process whose turn ended is left running with this turn's output taken (its
    outcome decides whether it is kept, ADR-0120); any other run's process tree is stopped first
    and its pipes drained."""
    kept = io.kept
    if kept is not None and turn_ended and io.completion_task not in done:
        stdout = io.stdout_task.result()
        data, exceeded = kept.stderr.end()
        if not stdout.trailing:
            record.kept = kept
        return stdout, _CapturedStream(data, exceeded)
    await terminate_process_tree(
        process,
        0.2 if io.completion_task in done else 2.0,
        completion=io.completion_task,
    )
    if kept is None:
        assert io.stderr_task is not None
        return await _finish_streams(io.stdout_task, io.stderr_task)
    return await _finish_streams(io.stdout_task, asyncio.create_task(_kept_stderr(kept)))


async def _kept_stderr(kept: KeptProcess) -> _CapturedStream:
    data, exceeded = await kept.stderr.finish()
    return _CapturedStream(data, exceeded)


async def _finish_streams(
    stdout_task: asyncio.Task[_CapturedStream],
    stderr_task: asyncio.Task[_CapturedStream],
) -> tuple[_CapturedStream, _CapturedStream]:
    try:
        async with asyncio.timeout(_PIPE_DRAIN_SECONDS):
            return await asyncio.gather(stdout_task, stderr_task)
    except TimeoutError:
        for task in (stdout_task, stderr_task):
            if not task.done():
                task.cancel()
        await asyncio.gather(stdout_task, stderr_task, return_exceptions=True)
        empty = _CapturedStream(b"", False)
        return (
            stdout_task.result() if stdout_task.done() and not stdout_task.cancelled() else empty,
            stderr_task.result() if stderr_task.done() and not stderr_task.cancelled() else empty,
        )


def _duration_ms(started: float) -> int:
    return round((time.monotonic() - started) * 1000)

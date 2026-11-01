"""In-memory lifecycle manager for provider invocations."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import re
import time
import uuid
from collections import deque
from collections.abc import Callable, Sequence
from contextlib import suppress
from dataclasses import dataclass, field, replace
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import IO

from agentnave.adapters import ProviderAdapter, get_adapter
from agentnave.adapters.base import PreparedCommand, parse_json_object
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
from agentnave.processes import spawn_process, terminate_process_tree

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
_PIPE_DRAIN_SECONDS = 1.0
_FINISHED_RETENTION_SECONDS = 7 * 24 * 60 * 60
_ENDED_TOOL_STATES = frozenset({"completed", "failed", "error", "errored"})


@dataclass(slots=True)
class _InvocationRecord:
    request: InvocationRequest
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
    tool_names: dict[str, str] = field(default_factory=lambda: {})
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

    def observe_event(self, line: bytes, adapter: ProviderAdapter) -> None:
        event = parse_json_object(line.decode(errors="replace"))
        if event is None:
            return
        self.last_event_at = time.monotonic()
        self.reported_model = adapter.event_model(event) or self.reported_model
        activity = adapter.activity(event)
        if activity is not None:
            code = activity.blocking_error
            if code is not None and code not in self.reported_errors:
                self.reported_errors.add(code)
                self.pending_attention.append(
                    InvocationError(
                        code, "CLI reported an execution blocker; inspect or cancel the invocation."
                    )
                )
                self.attention_event.set()
            if activity.kind == "message" and activity.public_output:
                # Delta events append; cumulative/final message events replace the current reply.
                if activity.message_delta and self.output_event_type == activity.event_type:
                    self.output_tail = (self.output_tail + activity.public_output)[-1000:]
                else:
                    self.output_tail = activity.public_output[-1000:]
                self.output_at = self.last_event_at
                self.output_event_type = activity.event_type
            previous = self.last_activity
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

    def take_update(self) -> InvocationResult | InvocationError | None:
        """Return the final result, else consume the oldest unread blocker."""
        if self.task is not None and self.task.done():
            return self.task.result()
        if not self.pending_attention:
            return None
        attention = self.pending_attention.popleft()
        if not self.pending_attention:
            self.attention_event.clear()
        return attention


@dataclass(frozen=True, slots=True)
class _CapturedStream:
    data: bytes
    exceeded: bool


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
    "You are a subagent dispatched by AgentNave. Complete the assigned task and report back "
    "to the calling manager. Do not call AgentNave tools or use AgentNave to delegate further "
    "work. If blocked, report the blocker to the manager."
)


def lock_file(path: Path, busy_message: str) -> IO[bytes]:
    """Acquire a nonblocking OS lock; close the returned handle to release it."""
    lock = path.open("a+b")
    try:
        if os.name == "nt":
            import msvcrt

            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        lock.close()
        raise ValueError(busy_message) from None
    return lock


class InvocationManager:
    """Own active child processes only for this Python process lifetime."""

    def __init__(self) -> None:
        self._records: dict[str, _InvocationRecord] = {}
        self._closed = False
        self.on_forget: Callable[[str], None] | None = None
        # Lock directory shared by every server on one data root; None claims in-process only.
        self.session_locks: Path | None = None

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        """Apply the shared subagent instruction before provider-specific encoding."""
        return get_adapter(request.provider).prepare(
            replace(request, prompt=f"{SUBAGENT_INSTRUCTIONS}\n\nAssigned task:\n{request.prompt}")
        )

    def start(
        self,
        request: InvocationRequest,
        *,
        prepared: PreparedCommand | None = None,
        claim: IO[bytes] | None = None,
    ) -> str:
        """Start one run; it owns claim (a reserve() result, taken here when omitted) from this
        call on and releases it once its process tree has ended."""
        if claim is None:
            claim = self.reserve(request)
        try:
            adapter = get_adapter(request.provider)
            prepared = prepared if prepared is not None else self.prepare(request)
            invocation_id = str(uuid.uuid4())
            record = _InvocationRecord(request)
            self._records[invocation_id] = record
            record.task = asyncio.create_task(self._execute(record, adapter, prepared))
        except BaseException:
            if claim is not None:
                claim.close()
            raise
        if claim is not None:
            held = claim
            record.task.add_done_callback(lambda _: held.close())
        record.task.add_done_callback(lambda _: self._schedule_cleanup(invocation_id))
        return invocation_id

    def reserve(self, request: InvocationRequest) -> IO[bytes] | None:
        """Claim the request's native session for one run, across every server sharing the
        lock directory; closing the handle, or this process ending, releases it."""
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
            and record.request.session_id == request.session_id
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
    ) -> InvocationResult | InvocationError | None:
        updates = await self.wait_for_any((invocation_id,), timeout_seconds)
        return updates.get(invocation_id)

    async def wait_for_any(
        self, invocation_ids: Sequence[str], timeout_seconds: float
    ) -> dict[str, InvocationResult | InvocationError]:
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
            updates: dict[str, InvocationResult | InvocationError] = {}
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

    def progress(self, invocation_id: str) -> tuple[str | None, str | None]:
        """What the CLI is doing now and when its reply last grew, as labels that change only
        with the work itself: thinking, tool:<name>, writing or retrying, and a wall time."""
        record = self._record(invocation_id)
        activity = record.last_activity
        doing: str | None = None
        if activity is not None:
            if activity.kind == "tool" and activity.state not in _ENDED_TOOL_STATES:
                doing = f"tool:{activity.tool_name or ''}"
            elif activity.kind == "lifecycle" and activity.state == "reasoning":
                doing = "thinking"
            elif activity.kind == "message":
                doing = "writing"
            elif activity.kind == "retry":
                doing = "retrying"
        output_at: str | None = None
        if record.output_at is not None:
            wall = record.started_wall_at + timedelta(seconds=record.output_at - record.started_at)
            output_at = wall.replace(microsecond=0).isoformat()
        return doing, output_at

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

    async def cancel(self, invocation_id: str) -> InvocationResult:
        record = self._record(invocation_id)
        record.phase = InvocationPhase.STOPPING
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
        await asyncio.gather(*(self.cancel(item) for item in active), return_exceptions=True)

    def forget_finished(self, invocation_ids: set[str]) -> int:
        removed = 0
        for invocation_id in invocation_ids:
            record = self._records.get(invocation_id)
            if record is not None and record.task is not None and record.task.done():
                if record.cleanup_handle is not None:
                    record.cleanup_handle.cancel()
                del self._records[invocation_id]
                if self.on_forget is not None:
                    self.on_forget(invocation_id)
                removed += 1
        return removed

    def _record(self, invocation_id: str) -> _InvocationRecord:
        try:
            return self._records[invocation_id]
        except KeyError as exc:
            raise KeyError(f"unknown invocation_id: {invocation_id}") from exc

    async def _execute(
        self, record: _InvocationRecord, adapter: ProviderAdapter, prepared: PreparedCommand
    ) -> InvocationResult:
        try:
            result = await self._run(record, adapter, prepared)
            reported = record.reported_model or adapter.session_model(
                result.session_id, since=record.started_wall_at
            )
            record.reported_model = reported
            return replace(result, reported_model=reported)
        except Exception:
            logging.getLogger(__name__).exception("Unexpected invocation failure")
            return InvocationResult(
                InvocationStatus.FAILED,
                record.request.provider,
                "",
                record.request.session_id,
                {},
                _duration_ms(record.started_at),
                InvocationError(
                    "internal_error", "Unexpected invocation failure; inspect server logs."
                ),
            )

    async def _run(
        self, record: _InvocationRecord, adapter: ProviderAdapter, prepared: PreparedCommand
    ) -> InvocationResult:
        started = record.started_at
        supervised = None
        provider_returncode: int | None = None
        tasks: list[asyncio.Task[object]] = []
        try:
            supervised = await spawn_process(prepared.argv, prepared.cwd)
            process = supervised.process
            record.process = process
            record.phase = InvocationPhase.RUNNING
            if process.stdout is None or process.stderr is None or process.stdin is None:
                raise RuntimeError("provider subprocess pipes were not created")

            output_exceeded = asyncio.Event()
            stdout_diagnostics = _StreamDiagnostics()
            stderr_diagnostics = _StreamDiagnostics()
            stdout_task = asyncio.create_task(
                _read_limited(
                    process.stdout,
                    _MAX_STDOUT_BYTES,
                    output_exceeded,
                    lambda line: record.observe_event(line, adapter),
                    prepared.capture_line,
                    prepared.skip_oversized_line,
                    event_limit=_MAX_EVENT_BYTES,
                    diagnostics=stdout_diagnostics,
                )
            )
            stderr_task = asyncio.create_task(
                _read_limited(
                    process.stderr,
                    _MAX_STDERR_BYTES,
                    output_exceeded,
                    diagnostics=stderr_diagnostics,
                )
            )
            stdin_task = asyncio.create_task(_write_stdin(process.stdin, prepared.stdin))
            completion_task = supervised.completion
            cancel_task = asyncio.create_task(record.cancel_event.wait())
            exceeded_task = asyncio.create_task(output_exceeded.wait())
            tasks.extend(
                (
                    stdout_task,
                    stderr_task,
                    stdin_task,
                    completion_task,
                    cancel_task,
                    exceeded_task,
                )
            )

            done, _ = await asyncio.wait(
                (completion_task, cancel_task, exceeded_task),
                return_when=asyncio.FIRST_COMPLETED,
            )
            if exceeded_task in done and output_exceeded.is_set():
                terminal = InvocationStatus.FAILED
                error = _OUTPUT_LIMIT_ERROR
            elif cancel_task in done and record.cancel_event.is_set():
                terminal = InvocationStatus.CANCELLED
                error = InvocationError("cancelled", "invocation was cancelled")
            else:
                completion = completion_task.result()
                if completion.kind == "exit" and completion.returncode is not None:
                    provider_returncode = completion.returncode
                    terminal = None
                    error = None
                else:
                    terminal = InvocationStatus.FAILED
                    error = InvocationError(
                        (
                            "launch_error"
                            if completion.kind == "launch_error"
                            else "supervision_lost"
                        ),
                        completion.message or "provider supervisor failed",
                    )

            record.phase = InvocationPhase.STOPPING
            await terminate_process_tree(process, 0.2 if completion_task in done else 2.0)
            stdout, stderr = await _finish_streams(stdout_task, stderr_task)
            # The provider may exit before the readers notice the limit while draining.
            if terminal is None and (
                stdout.exceeded
                or stderr.exceeded
                or stdout_diagnostics.exceeded_reason is not None
                or stderr_diagnostics.exceeded_reason is not None
            ):
                terminal = InvocationStatus.FAILED
                error = _OUTPUT_LIMIT_ERROR
            if error is _OUTPUT_LIMIT_ERROR:
                error = _output_limit_error(stdout, stderr, stdout_diagnostics, stderr_diagnostics)
            parse_returncode = provider_returncode
            if parse_returncode is None:
                parse_returncode = process.returncode if process.returncode is not None else 1
            parsed = adapter.parse(parse_returncode, stdout.data, stderr.data)

            # Without a final answer, an interrupted run keeps the reply text seen so far.
            partial = parsed.output or record.output_tail
            if terminal is not None:
                return InvocationResult(
                    terminal,
                    record.request.provider,
                    partial,
                    parsed.session_id or prepared.session_id or record.request.session_id,
                    parsed.usage,
                    _duration_ms(started),
                    error,
                )

            if provider_returncode is None:
                raise RuntimeError("provider completion did not include an exit status")
            parsed_error = None
            if parsed.error_message is not None:
                details = stderr.data.decode(errors="replace")[-_MAX_ERROR_DETAILS:] or None
                parsed_error = InvocationError("provider_error", parsed.error_message, details)
            return InvocationResult(
                parsed.status,
                record.request.provider,
                parsed.output if parsed.status is InvocationStatus.SUCCEEDED else partial,
                parsed.session_id or prepared.session_id,
                parsed.usage,
                _duration_ms(started),
                parsed_error,
                warnings=parsed.warnings,
            )
        except (OSError, ValueError) as exc:
            return InvocationResult(
                InvocationStatus.FAILED,
                record.request.provider,
                "",
                record.request.session_id,
                {},
                _duration_ms(started),
                InvocationError("launch_error", str(exc)),
            )
        finally:
            try:
                try:
                    if supervised is not None and supervised.process.returncode is None:
                        await terminate_process_tree(supervised.process)
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
                record.process = None
                record.tool_names.clear()


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
) -> _CapturedStream:
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


async def _write_stdin(writer: asyncio.StreamWriter, data: bytes | None) -> None:
    try:
        if data is not None:
            writer.write(data)
            await writer.drain()
    except (BrokenPipeError, ConnectionResetError):
        pass
    finally:
        writer.close()
        with suppress(BrokenPipeError, ConnectionResetError):
            await writer.wait_closed()


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

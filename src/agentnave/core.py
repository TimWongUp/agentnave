"""In-memory lifecycle manager for provider invocations."""

from __future__ import annotations

import asyncio
import logging
import time
import uuid
from collections import deque
from collections.abc import Callable, Sequence
from contextlib import suppress
from dataclasses import dataclass, field, replace
from pathlib import Path

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
_OUTPUT_LIMIT_ERROR = InvocationError(
    "output_limit_exceeded", "provider output exceeded the AgentNave capture limit"
)
_PIPE_DRAIN_SECONDS = 1.0
_FINISHED_RETENTION_SECONDS = 7 * 24 * 60 * 60


@dataclass(slots=True)
class _InvocationRecord:
    request: InvocationRequest
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

    def observe_event(self, line: bytes, adapter: ProviderAdapter) -> None:
        event = parse_json_object(line.decode(errors="replace"))
        if event is None:
            return
        self.last_event_at = time.monotonic()
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


class InvocationManager:
    """Own active child processes only for this Python process lifetime."""

    def __init__(self) -> None:
        self._records: dict[str, _InvocationRecord] = {}
        self._closed = False

    def start(self, request: InvocationRequest, *, prepared: PreparedCommand | None = None) -> str:
        self.check_start(request)
        adapter = get_adapter(request.provider)
        prepared = prepared if prepared is not None else adapter.prepare(request)
        invocation_id = str(uuid.uuid4())
        record = _InvocationRecord(request)
        self._records[invocation_id] = record
        record.task = asyncio.create_task(self._execute(record, adapter, prepared))
        record.task.add_done_callback(lambda _: self._schedule_cleanup(invocation_id))
        return invocation_id

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

    def recent_output(self, invocation_id: str) -> tuple[str, int | None]:
        record = self._record(invocation_id)
        age = (
            None
            if record.output_at is None
            else max(0, round((time.monotonic() - record.output_at) * 1000))
        )
        return record.output_tail, age

    async def cancel(self, invocation_id: str) -> InvocationResult:
        record = self._record(invocation_id)
        record.phase = InvocationPhase.STOPPING
        record.cancel_event.set()
        result = await self.wait(invocation_id)
        if result is None:
            raise RuntimeError("cancelled invocation did not terminate")
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
            return await self._run(record, adapter, prepared)
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
            stdout_task = asyncio.create_task(
                _read_limited(
                    process.stdout,
                    _MAX_STDOUT_BYTES,
                    output_exceeded,
                    lambda line: record.observe_event(line, adapter),
                    prepared.capture_line,
                    prepared.skip_oversized_line,
                    event_limit=_MAX_EVENT_BYTES,
                )
            )
            stderr_task = asyncio.create_task(
                _read_limited(
                    process.stderr,
                    _MAX_STDERR_BYTES,
                    output_exceeded,
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
            if terminal is None and (stdout.exceeded or stderr.exceeded):
                terminal = InvocationStatus.FAILED
                error = _OUTPUT_LIMIT_ERROR
            parse_returncode = provider_returncode
            if parse_returncode is None:
                parse_returncode = process.returncode if process.returncode is not None else 1
            parsed = adapter.parse(parse_returncode, stdout.data, stderr.data)

            if terminal is not None:
                return InvocationResult(
                    terminal,
                    record.request.provider,
                    parsed.output,
                    parsed.session_id or record.request.session_id,
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
                parsed.output,
                parsed.session_id,
                parsed.usage,
                _duration_ms(started),
                parsed_error,
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
) -> _CapturedStream:
    if capture_line is not None:
        return await _read_filtered(
            stream,
            limit,
            exceeded_event,
            observe_event,
            capture_line,
            skip_oversized_line,
            limit if event_limit is None else event_limit,
        )
    data = bytearray()
    pending = bytearray()
    exceeded = False
    while chunk := await stream.read(64 * 1024):
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

    while chunk := await stream.read(64 * 1024):
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

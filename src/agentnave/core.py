"""In-memory lifecycle manager for provider invocations."""

from __future__ import annotations

import asyncio
import time
import uuid
from collections.abc import Callable
from contextlib import suppress
from dataclasses import dataclass, field
from pathlib import Path

from agentnave.adapters import ProviderAdapter, get_adapter
from agentnave.models import (
    InvocationError,
    InvocationPhase,
    InvocationRequest,
    InvocationResult,
    InvocationSnapshot,
    InvocationStatus,
)
from agentnave.processes import spawn_process, terminate_process_tree

_MAX_STDOUT_BYTES = 8 * 1024 * 1024
_MAX_STDERR_BYTES = 64 * 1024
_MAX_ERROR_DETAILS = 16_384
_PIPE_DRAIN_SECONDS = 1.0


@dataclass(slots=True)
class _InvocationRecord:
    request: InvocationRequest
    started_at: float = field(default_factory=time.monotonic)
    phase: InvocationPhase = InvocationPhase.PREPARING
    cancel_event: asyncio.Event = field(default_factory=asyncio.Event)
    task: asyncio.Task[InvocationResult] | None = None
    process: asyncio.subprocess.Process | None = None
    last_event_at: float | None = None

    def observe_event(self) -> None:
        self.last_event_at = time.monotonic()


@dataclass(frozen=True, slots=True)
class _CapturedStream:
    data: bytes
    exceeded: bool


class InvocationManager:
    """Own active child processes only for this Python process lifetime."""

    def __init__(self) -> None:
        self._records: dict[str, _InvocationRecord] = {}
        self._closed = False

    def start(self, request: InvocationRequest) -> str:
        if self._closed:
            raise RuntimeError("invocation manager is closed")
        adapter = get_adapter(request.provider)
        invocation_id = str(uuid.uuid4())
        record = _InvocationRecord(request)
        self._records[invocation_id] = record
        record.task = asyncio.create_task(self._execute(record, adapter))
        return invocation_id

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
        )

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
        active = [
            invocation_id
            for invocation_id, record in self._records.items()
            if record.task is not None and not record.task.done()
        ]
        await asyncio.gather(*(self.cancel(item) for item in active), return_exceptions=True)

    def _record(self, invocation_id: str) -> _InvocationRecord:
        try:
            return self._records[invocation_id]
        except KeyError as exc:
            raise KeyError(f"unknown invocation_id: {invocation_id}") from exc

    async def _execute(
        self, record: _InvocationRecord, adapter: ProviderAdapter
    ) -> InvocationResult:
        started = record.started_at
        prepared = None
        supervised = None
        provider_returncode: int | None = None
        tasks: list[asyncio.Task[object]] = []
        try:
            prepared = adapter.prepare(record.request)
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
                    record.observe_event,
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
                timeout=record.request.timeout_seconds,
                return_when=asyncio.FIRST_COMPLETED,
            )
            if exceeded_task in done and output_exceeded.is_set():
                terminal = InvocationStatus.FAILED
                error = InvocationError(
                    "output_limit_exceeded",
                    "provider output exceeded the AgentNave capture limit",
                )
            elif cancel_task in done and record.cancel_event.is_set():
                terminal = InvocationStatus.CANCELLED
                error = InvocationError("cancelled", "invocation was cancelled")
            elif completion_task in done:
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
            else:
                terminal = InvocationStatus.TIMED_OUT
                error = InvocationError(
                    "timed_out",
                    f"invocation exceeded {record.request.timeout_seconds:g} seconds",
                )

            record.phase = InvocationPhase.STOPPING
            await terminate_process_tree(process, 0.2 if completion_task in done else 2.0)
            stdout, stderr = await _finish_streams(stdout_task, stderr_task)
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
            if supervised is not None and supervised.process.returncode is None:
                await terminate_process_tree(supervised.process)
            for task in tasks:
                if not task.done():
                    task.cancel()
            if tasks:
                await asyncio.gather(*tasks, return_exceptions=True)
            if prepared is not None:
                for path in prepared.cleanup_paths:
                    Path(path).unlink(missing_ok=True)
            record.process = None


async def _read_limited(
    stream: asyncio.StreamReader,
    limit: int,
    exceeded_event: asyncio.Event,
    observe_event: Callable[[], None] | None = None,
) -> _CapturedStream:
    data = bytearray()
    pending = bytearray()
    exceeded = False
    while chunk := await stream.read(64 * 1024):
        if observe_event is not None:
            pending.extend(chunk[: max(0, limit - len(pending))])
            while b"\n" in pending:
                line, _, remainder = pending.partition(b"\n")
                pending = bytearray(remainder)
                if line.strip():
                    observe_event()
        remaining = limit - len(data)
        if remaining > 0:
            data.extend(chunk[:remaining])
        if len(chunk) > remaining:
            exceeded = True
            exceeded_event.set()
    if observe_event is not None and pending.strip():
        observe_event()
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

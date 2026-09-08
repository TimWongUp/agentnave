from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path

import pytest

from agentnave.adapters.base import ParsedProviderResult, PreparedCommand
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.core import InvocationManager, _read_limited  # pyright: ignore[reportPrivateUsage]
from agentnave.models import InvocationRequest, InvocationStatus


class FakeAdapter(ClaudeAdapter):
    name = "claude"

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        seconds = "30" if request.prompt == "sleep" else "0"
        if request.prompt == "child_holds_pipes":
            marker = request.cwd / "descendant-survived"
            code = (
                "import subprocess,sys; "
                "subprocess.Popen([sys.executable, '-c', "
                f"\"import pathlib,time; time.sleep(0.5); pathlib.Path({str(marker)!r}).write_text('survived')\"] )"
            )
        elif request.prompt == "large_output":
            marker = request.cwd / "provider-survived"
            code = (
                "import pathlib,sys,time; "
                "sys.stdout.buffer.write(b'x' * (9 * 1024 * 1024)); sys.stdout.flush(); "
                f"time.sleep(0.5); pathlib.Path({str(marker)!r}).write_text('survived')"
            )
        elif request.prompt == "kill_supervisor":
            code = "import os,signal,time; os.kill(os.getppid(), signal.SIGKILL); time.sleep(0.1)"
        elif request.prompt == "progress":
            code = (
                "import sys,time; "
                'sys.stdout.write(\'{"type":"system","subtype":"api_retry","error":"authentication_failed"}\\n\'); '
                "sys.stdout.flush(); time.sleep(30)"
            )
        elif request.prompt == "streaming_message":
            events = [
                {"type": "stream_event", "event": {"delta": {"type": "text_delta", "text": text}}}
                for text in ("x" * 600, "中文")
            ]
            wire = (
                "\n".join(json.dumps(event, ensure_ascii=False) for event in events) + "\n"
            ).encode()
            split = wire.index("中文".encode()) + 1
            code = (
                "import sys,time; "
                f"sys.stdout.buffer.write({wire[:split]!r}); sys.stdout.flush(); time.sleep(0.02); "
                f"sys.stdout.buffer.write({wire[split:]!r}); sys.stdout.flush(); time.sleep(30)"
            )
        elif request.prompt == "interleaved_tools":
            events = [
                {
                    "type": "assistant",
                    "message": {"content": [{"type": "tool_use", "id": tool_id, "name": name}]},
                }
                for tool_id, name in (("a", "Bash"), ("b", "Read"))
            ]
            events.append(
                {
                    "type": "user",
                    "message": {"content": [{"type": "tool_result", "tool_use_id": "a"}]},
                }
            )
            wire = "\n".join(json.dumps(event) for event in events) + "\n"
            code = (
                f"import sys,time; sys.stdout.write({wire!r}); sys.stdout.flush(); time.sleep(30)"
            )
        else:
            code = f"import sys,time; time.sleep({seconds}); sys.stdout.write('completed')"
        return PreparedCommand((sys.executable, "-c", code), request.cwd)

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        assert not stderr
        if returncode != 0:
            return ParsedProviderResult(
                InvocationStatus.FAILED,
                "",
                "provider-session",
                {"calls": 1},
                "provider stream ended before completion",
            )
        return ParsedProviderResult(
            InvocationStatus.SUCCEEDED, stdout.decode(), "provider-session", {"calls": 1}
        )


@pytest.fixture
def fake_adapter(monkeypatch: pytest.MonkeyPatch) -> None:
    def adapter_for(_provider: str) -> FakeAdapter:
        return FakeAdapter()

    monkeypatch.setattr("agentnave.core.get_adapter", adapter_for)


@pytest.mark.asyncio
async def test_manager_returns_normalized_terminal_result(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "work", tmp_path))

    result = await manager.wait(invocation_id)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "completed"
    assert result.session_id == "provider-session"
    assert result.provider_usage == {"calls": 1}
    await manager.shutdown()


@pytest.mark.asyncio
async def test_provider_cwd_cannot_shadow_supervisor_module(
    tmp_path: Path, fake_adapter: None
) -> None:
    shadow_package = tmp_path / "agentnave"
    shadow_package.mkdir()
    (shadow_package / "__init__.py").write_text("")
    marker = tmp_path / "shadow-supervisor-ran"
    (shadow_package / "supervisor.py").write_text(
        f"from pathlib import Path\nPath({str(marker)!r}).write_text('hijacked')\n"
    )
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "work", tmp_path))

    result = await manager.wait(invocation_id)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    assert not marker.exists()
    await manager.shutdown()


@pytest.mark.asyncio
async def test_provider_completion_does_not_depend_on_default_thread_pool(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def unavailable_to_thread(*_args: object, **_kwargs: object) -> object:
        await asyncio.Future()

    monkeypatch.setattr(asyncio, "to_thread", unavailable_to_thread)
    manager = InvocationManager()
    invocation_id = manager.start(
        InvocationRequest("claude", "work", tmp_path, timeout_seconds=0.5)
    )

    result = await asyncio.wait_for(manager.wait(invocation_id), 2)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "completed"
    await manager.shutdown()


@pytest.mark.asyncio
async def test_wait_timeout_does_not_cancel_invocation(tmp_path: Path, fake_adapter: None) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "sleep", tmp_path))

    assert await manager.wait(invocation_id, 0.01) is None
    result = await manager.cancel(invocation_id)

    assert result.status is InvocationStatus.CANCELLED
    assert result.output == ""
    assert result.session_id == "provider-session"
    assert result.provider_usage == {"calls": 1}
    await manager.shutdown()


@pytest.mark.asyncio
async def test_running_snapshot_reports_phase_elapsed_and_event_activity(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "progress", tmp_path))
    try:
        snapshot = manager.snapshot(invocation_id)
        for _ in range(50):
            if snapshot.last_event_age_ms is not None:
                break
            await asyncio.sleep(0.01)
            snapshot = manager.snapshot(invocation_id)

        assert snapshot.phase.value == "running"
        assert snapshot.elapsed_ms >= 0
        assert snapshot.last_event_age_ms is not None
        assert snapshot.remaining_ms is None
        assert snapshot.last_activity is not None
        assert snapshot.last_activity.kind == "retry"
        assert snapshot.last_activity.message == "authentication_failed"
        assert snapshot.last_activity_age_ms is not None
    finally:
        await manager.cancel(invocation_id)
        await manager.shutdown()


@pytest.mark.asyncio
async def test_invocation_timeout_terminates_process(tmp_path: Path, fake_adapter: None) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(
        InvocationRequest("claude", "sleep", tmp_path, timeout_seconds=0.01)
    )

    result = await manager.wait(invocation_id)

    assert result is not None
    assert result.status is InvocationStatus.TIMED_OUT
    assert result.output == ""
    assert result.session_id == "provider-session"
    assert result.error is not None
    assert result.error.code == "timed_out"
    await manager.shutdown()


@pytest.mark.asyncio
async def test_snapshot_joins_split_unicode_deltas_and_reports_explicit_budget(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(
        InvocationRequest("claude", "streaming_message", tmp_path, timeout_seconds=10)
    )
    try:
        snapshot = manager.snapshot(invocation_id)
        for _ in range(50):
            await manager.wait(invocation_id, 0.02)
            snapshot = manager.snapshot(invocation_id)
            if snapshot.last_activity and (snapshot.last_activity.message or "").endswith("中文"):
                break
        assert snapshot.last_activity is not None
        assert snapshot.last_activity.message == "x" * 510 + "中文"
        assert snapshot.remaining_ms is not None
        assert 0 < snapshot.remaining_ms <= 10_000
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_snapshot_retains_tool_name_across_interleaved_calls(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "interleaved_tools", tmp_path))
    try:
        snapshot = manager.snapshot(invocation_id)
        for _ in range(50):
            await manager.wait(invocation_id, 0.02)
            snapshot = manager.snapshot(invocation_id)
            if snapshot.last_activity and snapshot.last_activity.state == "completed":
                break
        assert snapshot.last_activity is not None
        assert snapshot.last_activity.tool_call_id == "a"
        assert snapshot.last_activity.tool_name == "Bash"
        assert snapshot.last_activity.state == "completed"
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_completion_cleans_group_when_provider_leaves_descendant(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(
        InvocationRequest("claude", "child_holds_pipes", tmp_path, timeout_seconds=3)
    )

    result = await asyncio.wait_for(manager.wait(invocation_id), 3)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    await asyncio.sleep(0.6)
    assert not (tmp_path / "descendant-survived").exists()
    await manager.shutdown()


@pytest.mark.asyncio
async def test_output_limit_terminates_provider_without_unbounded_capture(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "large_output", tmp_path))

    result = await asyncio.wait_for(manager.wait(invocation_id), 3)

    assert result is not None
    assert result.status is InvocationStatus.FAILED
    assert result.error is not None
    assert result.error.code == "output_limit_exceeded"
    assert result.output == ""
    await asyncio.sleep(0.6)
    assert not (tmp_path / "provider-survived").exists()
    await manager.shutdown()


@pytest.mark.asyncio
async def test_supervisor_still_owns_process_group_when_cleanup_starts(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import core

    observed_returncodes: list[int | None] = []
    original = core.terminate_process_tree

    async def observe_cleanup(
        process: asyncio.subprocess.Process, grace_seconds: float = 2.0
    ) -> None:
        observed_returncodes.append(process.returncode)
        await original(process, grace_seconds)

    monkeypatch.setattr(core, "terminate_process_tree", observe_cleanup)
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "work", tmp_path))

    result = await manager.wait(invocation_id)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    assert observed_returncodes == [None]
    await manager.shutdown()


@pytest.mark.asyncio
async def test_supervisor_loss_is_reported_as_infrastructure_failure(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "kill_supervisor", tmp_path))

    result = await asyncio.wait_for(manager.wait(invocation_id), 3)

    assert result is not None
    assert result.status is InvocationStatus.FAILED
    assert result.error is not None
    assert result.error.code == "supervision_lost"
    await manager.shutdown()


@pytest.mark.asyncio
async def test_shutdown_cancels_every_active_invocation(tmp_path: Path, fake_adapter: None) -> None:
    manager = InvocationManager()
    invocation_ids = [
        manager.start(InvocationRequest("claude", "sleep", tmp_path)) for _ in range(2)
    ]
    await asyncio.sleep(0.01)

    await manager.shutdown()

    results = [await manager.wait(item) for item in invocation_ids]
    assert all(result is not None for result in results)
    assert all(result.status is InvocationStatus.CANCELLED for result in results if result)


@pytest.mark.asyncio
async def test_activity_observer_stops_at_capture_limit() -> None:
    stream = asyncio.StreamReader()
    stream.feed_data(b"{}\n{}\n")
    stream.feed_eof()
    exceeded = asyncio.Event()
    observed: list[bytes] = []
    captured = await _read_limited(stream, 3, exceeded, observed.append)
    assert captured.data == b"{}\n"
    assert captured.exceeded and exceeded.is_set()
    assert observed == [b"{}"]

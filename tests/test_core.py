from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from dataclasses import replace
from pathlib import Path
from typing import Any, cast

import pytest

from agentnave.adapters.base import ParsedProviderResult, PreparedCommand
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codex import CodexAdapter
from agentnave.adapters.grok import GrokAdapter
from agentnave.adapters.pi import PiAdapter
from agentnave.core import (
    _MAX_STDOUT_BYTES,  # pyright: ignore[reportPrivateUsage]
    InvocationManager,
    _read_limited,  # pyright: ignore[reportPrivateUsage]
)
from agentnave.models import (
    InvocationError,
    InvocationPhase,
    InvocationRequest,
    InvocationStatus,
)
from agentnave.processes import (
    _wait_windows_supervisor_ready,  # pyright: ignore[reportPrivateUsage]
)


@pytest.mark.asyncio
async def test_failed_codex_resume_does_not_reuse_old_turn_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("CODEX_HOME", str(tmp_path))
    session = "01a14d5a-d23d-7263-b50d-ca6b5ac429dd"
    folder = tmp_path / "sessions" / "2026" / "01" / "01"
    folder.mkdir(parents=True)
    (folder / f"rollout-2026-01-01T00-00-00-{session}.jsonl").write_text(
        json.dumps(
            {
                "type": "turn_context",
                "timestamp": "2026-01-01T00:00:00Z",
                "payload": {"model": "old-model"},
            }
        )
        + "\n"
    )
    manager = InvocationManager()
    try:
        invocation = manager.start(
            InvocationRequest("codex", "resume", tmp_path, session),
            prepared=PreparedCommand((str(tmp_path / "missing-provider"),), tmp_path),
        )
        result = await manager.wait(invocation)
        assert result is not None and result.status is InvocationStatus.FAILED
        assert result.session_id == session
        assert result.reported_model is None
    finally:
        await manager.shutdown()


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
        elif request.prompt in {"at_output_limit", "large_output"}:
            marker = request.cwd / "provider-survived"
            size = 1024 * 1024 + (request.prompt == "large_output")
            code = (
                "import pathlib,sys,time; "
                f"sys.stdout.buffer.write(b'x' * {size}); sys.stdout.flush(); "
                f"time.sleep(0.5); pathlib.Path({str(marker)!r}).write_text('survived')"
            )
        elif request.prompt == "kill_supervisor":
            if os.name == "nt":
                code = (
                    "import ctypes,os,time; "
                    "handle=ctypes.windll.kernel32.OpenProcess(1,False,os.getppid()); "
                    "ctypes.windll.kernel32.TerminateProcess(handle,1); time.sleep(0.1)"
                )
            else:
                code = (
                    "import os,signal,time; os.kill(os.getppid(), signal.SIGKILL); time.sleep(0.1)"
                )
        elif request.prompt in {"progress", "multiple_blockers", "late_blocker"}:
            delay = 1.0 if request.prompt == "late_blocker" else 0
            errors = ["authentication_failed"]
            if request.prompt == "multiple_blockers":
                errors += ["permission_denied", "authentication_failed"]
            wire = (
                "\n".join(
                    json.dumps({"type": "system", "subtype": "api_retry", "error": error})
                    for error in errors
                )
                + "\n"
            )
            written = request.cwd / "blocker-written"
            code = (
                f"import pathlib,sys,time; time.sleep({delay}); "
                f"sys.stdout.write({wire!r}); sys.stdout.flush(); "
                f"pathlib.Path({str(written)!r}).touch(); time.sleep(30)"
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
async def test_active_native_session_cannot_be_resumed_twice(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    request = InvocationRequest("claude", "sleep", tmp_path, "existing-session")
    first = manager.start(request)
    try:
        with pytest.raises(ValueError, match="active invocation"):
            manager.start(request)
        # A different provider's identical session identifier is independent.
        other = manager.start(InvocationRequest("grok", "complete", tmp_path, request.session_id))
        assert await manager.wait(other) is not None
        await manager.cancel(first)
        resumed = manager.start(
            InvocationRequest("claude", "complete", tmp_path, request.session_id)
        )
        assert await manager.wait(resumed) is not None
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_unexpected_execution_error_is_a_readable_terminal_result(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    def broken_parse(
        _self: FakeAdapter, _returncode: int, _stdout: bytes, _stderr: bytes
    ) -> ParsedProviderResult:
        raise RuntimeError("PRIVATE INTERNAL DETAIL")

    monkeypatch.setattr(FakeAdapter, "parse", broken_parse)
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "complete", tmp_path, "existing"))
    try:
        result = await manager.wait_for_update(invocation_id, 3)
        assert result is not None and not isinstance(result, InvocationError)
        assert result.status is InvocationStatus.FAILED
        assert result.error is not None and result.error.code == "internal_error"
        assert result.session_id == "existing"
        assert "PRIVATE INTERNAL DETAIL" not in str(result.to_dict())
        assert await manager.wait(invocation_id) == result
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_finished_invocations_expire_without_expiring_active_work(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("agentnave.core._FINISHED_RETENTION_SECONDS", 0.1)
    manager = InvocationManager()
    active = manager.start(InvocationRequest("claude", "sleep", tmp_path))
    finished = manager.start(InvocationRequest("claude", "complete", tmp_path))
    try:
        result = await manager.wait(finished)
        assert result is not None and result.output == "completed"
        assert await manager.wait_for_update(finished, 0) == result
        # Expiry runs even when there are no new requests, and reading did not remove it.
        await asyncio.sleep(0.2)
        with pytest.raises(KeyError, match="unknown invocation_id"):
            await manager.wait(finished)
        assert await manager.wait(active, 0) is None
        cancelled = await manager.cancel(active)
        assert cancelled.status is InvocationStatus.CANCELLED
        await asyncio.sleep(0.2)
        with pytest.raises(KeyError, match="unknown invocation_id"):
            await manager.cancel(active)
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_wait_preserves_distinct_blockers_in_one_stdout_burst(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "multiple_blockers", tmp_path))
    try:
        async with asyncio.timeout(3):
            first = await manager.wait_for_update(invocation_id, 600)
            second = await manager.wait_for_update(invocation_id, 600)
        assert isinstance(first, InvocationError)
        assert isinstance(second, InvocationError)
        assert [first.code, second.code] == ["authentication_failed", "permission_denied"]
        assert await manager.wait_for_update(invocation_id, 0.02) is None
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_wait_any_returns_ready_subset_without_consuming_other_blockers(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    fast = manager.start(InvocationRequest("claude", "progress", tmp_path))
    burst = manager.start(InvocationRequest("claude", "multiple_blockers", tmp_path))
    late = manager.start(InvocationRequest("claude", "late_blocker", tmp_path))
    done = manager.start(InvocationRequest("claude", "complete", tmp_path))
    watched = [fast, burst, late]
    try:
        async with asyncio.timeout(5):
            while any(manager.snapshot(item).last_event_age_ms is None for item in (fast, burst)):
                await asyncio.sleep(0.01)
            first = await manager.wait_for_any(watched, 600)
            second = await manager.wait_for_any(watched, 600)
            third = await manager.wait_for_any(watched, 600)
            finished = await manager.wait(done)
            terminal = await manager.wait_for_any([late, done], 600)
        codes = [
            {key: cast(InvocationError, value).code for key, value in updates.items()}
            for updates in (first, second, third)
        ]
        # Every ready invocation is returned together, but each gives up only one blocker.
        assert codes[0] == {fast: "authentication_failed", burst: "authentication_failed"}
        assert codes[1] == {burst: "permission_denied"}
        # Earlier returns did not swallow the later blocker of the slower invocation.
        assert codes[2] == {late: "authentication_failed"}
        assert await manager.wait_for_any(watched, 0.05) == {}
        assert terminal == {done: finished}
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_wait_any_leaves_blocker_unread_when_it_lands_after_the_window(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    late = manager.start(InvocationRequest("claude", "late_blocker", tmp_path))
    try:
        async with asyncio.timeout(5):
            while manager.snapshot(late).phase is not InvocationPhase.RUNNING:
                await asyncio.sleep(0.01)
            waiting = asyncio.create_task(manager.wait_for_any([late], 0.05))
            await asyncio.sleep(0)
            # Stall the loop until both the window and the blocker are due.
            started = time.monotonic()
            while not (tmp_path / "blocker-written").exists() or time.monotonic() - started < 0.1:
                assert time.monotonic() - started < 5
                time.sleep(0.01)
            assert await waiting == {}
            later = await manager.wait_for_any([late], 600)
        assert cast(InvocationError, later[late]).code == "authentication_failed"
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_blocker_wakes_long_wait_once_and_cancellation_still_works(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "progress", tmp_path))
    try:
        async with asyncio.timeout(3):
            notice = await manager.wait_for_update(invocation_id, 600)
        assert isinstance(notice, InvocationError)
        assert notice.code == "authentication_failed"
        # No duplicate notice, and neither wait cancels the CLI.
        assert await manager.wait_for_update(invocation_id, 0.02) is None
        assert manager.snapshot(invocation_id).phase.value == "running"
        result = await manager.cancel(invocation_id)
        assert result.status is InvocationStatus.CANCELLED
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_wait_update_timeout_keeps_streaming_public_tail(
    tmp_path: Path, fake_adapter: None
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "streaming_message", tmp_path))
    try:
        assert await manager.wait_for_update(invocation_id, 0.5) is None
        output, age = manager.recent_output(invocation_id)
        assert output == "x" * 600 + "中文"
        assert age is not None
        waiting = asyncio.create_task(manager.wait_for_update(invocation_id, 600))
        await asyncio.sleep(0)
        waiting.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiting
        assert await manager.wait_for_update(invocation_id, 0.01) is None
    finally:
        await manager.shutdown()


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
    invocation_id = manager.start(InvocationRequest("claude", "work", tmp_path))

    result = await asyncio.wait_for(manager.wait(invocation_id), 2)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "completed"
    await manager.shutdown()


@pytest.mark.asyncio
async def test_windows_supervisor_is_not_exposed_before_job_is_ready(tmp_path: Path) -> None:
    class PendingProcess:
        returncode: int | None = None

    ready_path = tmp_path / "job-ready"
    process = cast(asyncio.subprocess.Process, PendingProcess())
    waiting = asyncio.create_task(_wait_windows_supervisor_ready(process, ready_path))

    await asyncio.sleep(0.02)
    assert not waiting.done()

    ready_path.touch()
    await asyncio.wait_for(waiting, 1)


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
        async with asyncio.timeout(3):
            while snapshot.last_event_age_ms is None:
                await asyncio.sleep(0.01)
                snapshot = manager.snapshot(invocation_id)

        assert snapshot.phase.value == "running"
        assert snapshot.elapsed_ms >= 0
        assert snapshot.last_event_age_ms is not None
        assert snapshot.last_activity is not None
        assert snapshot.last_activity.kind == "retry"
        assert snapshot.last_activity.message == "authentication_failed"
        assert snapshot.last_activity_age_ms is not None
    finally:
        await manager.cancel(invocation_id)
        await manager.shutdown()


@pytest.mark.asyncio
async def test_snapshot_joins_split_unicode_deltas(tmp_path: Path, fake_adapter: None) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "streaming_message", tmp_path))
    try:
        snapshot = manager.snapshot(invocation_id)
        for _ in range(50):
            await manager.wait(invocation_id, 0.02)
            snapshot = manager.snapshot(invocation_id)
            if snapshot.last_activity and (snapshot.last_activity.message or "").endswith("中文"):
                break
        assert snapshot.last_activity is not None
        assert snapshot.last_activity.message == "x" * 510 + "中文"
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
    invocation_id = manager.start(InvocationRequest("claude", "child_holds_pipes", tmp_path))

    result = await asyncio.wait_for(manager.wait(invocation_id), 3)

    assert result is not None
    assert result.status is InvocationStatus.SUCCEEDED
    await asyncio.sleep(0.6)
    assert not (tmp_path / "descendant-survived").exists()
    await manager.shutdown()


@pytest.mark.asyncio
async def test_grok_image_tool_output_does_not_terminate_invocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class ImageGrokAdapter(GrokAdapter):
        def prepare(self, request: InvocationRequest) -> PreparedCommand:
            prepared = super().prepare(request)
            code = (
                "import json,time; "
                "print(json.dumps({'type':'text','data':'检查中'})); "
                "event={'type':'tool_call_update','toolCallId':'image',"
                "'toolName':'read_file','status':'completed','content':[{'type':'content','content':"
                "{'type':'image','data':'x'*(1024*1024),'mimeType':'image/png'}}]}; "
                "[print(json.dumps(event),flush=True) for _ in range(10)]; "
                "time.sleep(0.1); "
                "print(json.dumps({'type':'text','data':'图片已核对'})); "
                "print(json.dumps({'type':'end','stopReason':'end_turn',"
                "'sessionId':'image-session'}))"
            )
            return replace(prepared, argv=(sys.executable, "-c", code))

    def adapter_for(_provider: str) -> ImageGrokAdapter:
        return ImageGrokAdapter()

    monkeypatch.setattr("agentnave.core.get_adapter", adapter_for)
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("grok", "inspect images", tmp_path))
    try:
        result = await asyncio.wait_for(manager.wait(invocation_id), 5)
        assert result is not None
        assert result.status is InvocationStatus.SUCCEEDED
        assert result.output == "图片已核对"
        assert result.session_id == "image-session"
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_claude_long_tool_stream_does_not_terminate_invocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class LongClaudeAdapter(ClaudeAdapter):
        def prepare(self, request: InvocationRequest) -> PreparedCommand:
            prepared = super().prepare(request)
            code = (
                "import json; "
                "s='long-session'; "
                "print(json.dumps({'type':'system','subtype':'init','session_id':s})); "
                "result={'type':'user','session_id':s,'message':{'content':[{'type':'tool_result',"
                "'tool_use_id':'read','content':'x'*(1024*1024)}]}}; "
                "[print(json.dumps(result)) for _ in range(10)]; "
                "delta={'type':'stream_event','session_id':s,'event':{'type':'content_block_delta',"
                "'delta':{'type':'text_delta','text':'y'}}}; "
                "[print(json.dumps(delta)) for _ in range(20000)]; "
                "print(json.dumps({'type':'result','subtype':'success','result':'读完了',"
                "'session_id':s}))"
            )
            return replace(prepared, argv=(sys.executable, "-c", code))

    def adapter_for(_provider: str) -> LongClaudeAdapter:
        return LongClaudeAdapter()

    monkeypatch.setattr("agentnave.core.get_adapter", adapter_for)
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", "read files", tmp_path))
    try:
        result = await asyncio.wait_for(manager.wait(invocation_id), 10)
        assert result is not None
        assert result.status is InvocationStatus.SUCCEEDED
        assert result.output == "读完了"
        assert result.session_id == "long-session"
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("prompt, exceeds", [("at_output_limit", False), ("large_output", True)])
async def test_output_limit_terminates_provider_without_unbounded_capture(
    tmp_path: Path, fake_adapter: None, prompt: str, exceeds: bool
) -> None:
    manager = InvocationManager()
    invocation_id = manager.start(InvocationRequest("claude", prompt, tmp_path))

    result = await asyncio.wait_for(manager.wait(invocation_id), 3)

    assert result is not None
    if exceeds:
        assert result.status is InvocationStatus.FAILED
        assert result.error is not None
        assert result.error.code == "output_limit_exceeded"
        assert result.error.details is not None
        assert "stdout" in result.error.details
        assert result.output == ""
    else:
        assert result.status is InvocationStatus.SUCCEEDED
        assert result.error is None
        assert result.output == "x" * (1024 * 1024)
    await asyncio.sleep(0.6)
    assert (tmp_path / "provider-survived").exists() is not exceeds
    await manager.shutdown()


@pytest.mark.asyncio
async def test_output_limit_found_while_draining_after_exit_still_fails(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import core

    original = core._read_limited  # pyright: ignore[reportPrivateUsage]

    async def exceeded_without_signal(*args: Any, **kwargs: Any) -> Any:
        captured = await original(*args, **kwargs)
        return replace(captured, exceeded=True)

    monkeypatch.setattr(core, "_read_limited", exceeded_without_signal)
    manager = InvocationManager()
    result = await manager.wait(manager.start(InvocationRequest("claude", "work", tmp_path)))

    assert result is not None and result.error is not None
    assert result.status is InvocationStatus.FAILED
    assert result.error.code == "output_limit_exceeded"
    assert result.error.details is not None
    assert "stdout" in result.error.details and "stderr" in result.error.details
    await manager.shutdown()


@pytest.mark.asyncio
@pytest.mark.parametrize("stream", ["stdout", "stderr", "stdout_event"])
async def test_output_limit_diagnostics_preserve_session_and_redact_tail(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, stream: str
) -> None:
    monkeypatch.setattr("agentnave.core._MAX_STDOUT_BYTES", 1024)
    monkeypatch.setattr("agentnave.core._MAX_STDERR_BYTES", 512)
    monkeypatch.setattr("agentnave.core._MAX_EVENT_BYTES", 2048)
    monkeypatch.setenv("TEST_API_KEY", "environment-credential-value")
    events = [
        {"type": "thread.started", "thread_id": "diagnostic-session"},
        {"type": "item.completed", "item": {"type": "agent_message", "text": "已有回复"}},
    ]
    wire = b"".join(json.dumps(event).encode() + b"\n" for event in events)
    if stream == "stderr":
        payload = (
            b"token="
            + b"cut-credential" * 400
            + b"\n"
            + b"warning: retry\n" * 80
            + b"Authorization: Bearer unlabelled-bearer-value\n"
            + b'{"api_key":"json-credential-value"}\n'
            + b"connection environment-credential-value failed\n"
            + b"diagnostic-tail-marker\n"
        )
    else:
        size = 3000 if stream == "stdout_event" else 1100
        payload = (
            json.dumps(
                {"type": "item.completed", "item": {"type": "agent_message", "text": "x" * size}}
            ).encode()
            + b"\ndiagnostic-tail-marker\n"
        )

    class NoisyCodexAdapter(CodexAdapter):
        def prepare(self, request: InvocationRequest) -> PreparedCommand:
            code = (
                f"import sys,time; sys.stdout.buffer.write({wire!r}); sys.stdout.flush(); "
                f"sys.{'stderr' if stream == 'stderr' else 'stdout'}.buffer.write({payload!r}); "
                "sys.stdout.flush(); sys.stderr.flush(); time.sleep(30)"
            )
            return replace(super().prepare(request), argv=(sys.executable, "-c", code))

    def adapter_for(_provider: str) -> NoisyCodexAdapter:
        return NoisyCodexAdapter()

    monkeypatch.setattr("agentnave.core.get_adapter", adapter_for)
    if stream == "stderr":
        from agentnave import core

        original = core._read_limited  # pyright: ignore[reportPrivateUsage]

        async def reader_holds_pipe(*args: Any, **kwargs: Any) -> Any:
            captured = await original(*args, **kwargs)
            if kwargs.get("diagnostics") is not None and captured.exceeded:
                await asyncio.Event().wait()
            return captured

        monkeypatch.setattr(core, "_read_limited", reader_holds_pipe)
        monkeypatch.setattr(core, "_PIPE_DRAIN_SECONDS", 0.01)
    manager = InvocationManager()
    try:
        result = await asyncio.wait_for(
            manager.wait(manager.start(InvocationRequest("codex", "work", tmp_path))), 5
        )
        assert result is not None and result.error is not None
        assert result.error.code == "output_limit_exceeded"
        assert result.session_id == "diagnostic-session"
        assert result.output == "已有回复"
        details = result.error.details
        assert details is not None and len(details) <= 16_384
        assert ("stderr" if stream == "stderr" else "stdout") in details
        assert ("event_limit" if stream == "stdout_event" else "capture_limit") in details
        assert "diagnostic-tail-marker" in details
        for secret in (
            "unlabelled-bearer-value",
            "json-credential-value",
            "environment-credential-value",
            "cut-credential",
        ):
            assert secret not in details
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
@pytest.mark.skipif(os.name == "nt", reason="POSIX supervisor lifecycle assertion")
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
    prompt = "sleep" if os.name == "nt" else "kill_supervisor"
    invocation_id = manager.start(InvocationRequest("claude", prompt, tmp_path))
    if os.name == "nt":
        async with asyncio.timeout(3):
            record = manager._records[invocation_id]  # pyright: ignore[reportPrivateUsage]
            while record.process is None:
                await asyncio.sleep(0.01)
            record.process.terminate()

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


@pytest.mark.asyncio
@pytest.mark.parametrize("oversized_line", [False, True])
async def test_grok_filtered_capture_still_bounds_memory(
    tmp_path: Path, oversized_line: bool
) -> None:
    prepared = GrokAdapter().prepare(InvocationRequest("grok", "work", tmp_path))
    try:
        stream = asyncio.StreamReader()
        line = json.dumps({"type": "text", "data": "x" * 50}).encode()
        stream.feed_data(b"x" * 129 if oversized_line else line + b"\n" + line)
        stream.feed_eof()
        exceeded = asyncio.Event()
        captured = await _read_limited(stream, 128, exceeded, capture_line=prepared.capture_line)
        assert exceeded.is_set() and captured.exceeded
        assert len(captured.data) <= 128
    finally:
        for path in prepared.cleanup_paths:
            path.unlink()


@pytest.mark.asyncio
@pytest.mark.parametrize("final_oversized", [False, True])
async def test_pi_filtered_capture_skips_only_non_result_oversized_lines(
    tmp_path: Path, final_oversized: bool
) -> None:
    prepared = PiAdapter().prepare(InvocationRequest("pi", "work", tmp_path))
    assert prepared.capture_line is not None
    big = "x" * 70_000
    final: dict[str, object] = {
        "role": "assistant",
        "content": [{"type": "text", "text": big}],
        "stopReason": "error",
    }
    events: list[dict[str, object]] = [
        {
            "type": "message_end",
            "message": {"role": "assistant", "content": [], "stopReason": "stop"},
        },
        *([{"type": "message_end", "message": final}] if final_oversized else []),
        {"type": "agent_end", "messages": [big * 3]},
        {"type": "agent_settled", "aborted": False},
    ]
    stream = asyncio.StreamReader()
    stream.feed_data(
        b"".join(json.dumps(event, separators=(",", ":")).encode() + b"\n" for event in events)
    )
    stream.feed_eof()
    exceeded = asyncio.Event()
    captured = await _read_limited(
        stream,
        1024,
        exceeded,
        capture_line=prepared.capture_line,
        skip_oversized_line=prepared.skip_oversized_line,
    )
    assert exceeded.is_set() is final_oversized
    if not final_oversized:
        assert PiAdapter().parse(0, captured.data, b"").status is InvocationStatus.SUCCEEDED


@pytest.mark.asyncio
@pytest.mark.parametrize("reply_oversized", [False, True])
async def test_codex_filtered_capture_drops_tool_output_but_not_replies(
    tmp_path: Path, reply_oversized: bool
) -> None:
    prepared = CodexAdapter().prepare(InvocationRequest("codex", "review", tmp_path))
    event_limit = 256 * 1024

    def command(output: str) -> dict[str, object]:
        item = {"id": "item_1", "type": "command_execution", "aggregated_output": output}
        return {"type": "item.completed", "item": item}

    answer = "x" * (event_limit + 1) if reply_oversized else "审查完成" * 1000
    events: list[dict[str, object]] = [
        {"type": "thread.started", "thread_id": "codex-session"},
        *(command("d" * 200_000) for _ in range(8)),
        command("d" * (event_limit + 1)),
        {
            "type": "item.completed",
            "item": {"id": "item_2", "type": "agent_message", "text": answer},
        },
        {"type": "turn.completed", "usage": {"input_tokens": 9, "output_tokens": 2}},
    ]
    stream = asyncio.StreamReader()
    lines = [
        json.dumps(event, ensure_ascii=False, separators=(",", ":")).encode() for event in events
    ]
    # The skipped tool event may use spaces between keys and values.
    lines[9] = json.dumps(events[9]).encode()
    stream.feed_data(b"".join(line + b"\n" for line in lines))
    stream.feed_eof()
    exceeded = asyncio.Event()
    observed: list[bytes] = []
    captured = await _read_limited(
        stream,
        _MAX_STDOUT_BYTES,
        exceeded,
        observed.append,
        prepared.capture_line,
        prepared.skip_oversized_line,
        event_limit=event_limit,
    )

    assert exceeded.is_set() is reply_oversized
    if not reply_oversized:
        assert len(observed) == len(events) - 1
        result = CodexAdapter().parse(0, captured.data, b"")
        assert result.status is InvocationStatus.SUCCEEDED
        assert (result.output, result.session_id) == (answer, "codex-session")
        # Retained replies are never larger than the provider's own line (no ASCII escaping).
        assert prepared.capture_line is not None
        assert len(prepared.capture_line(lines[10])) <= len(lines[10]) + 1
    # A terminal event is never skipped, even when a nested field names a tool type.
    assert prepared.skip_oversized_line is not None
    assert not prepared.skip_oversized_line(
        b'{"error":{"type":"command_execution","message":"'
        + b"x" * 300
        + b'"},"type":"turn.failed"}'
    )

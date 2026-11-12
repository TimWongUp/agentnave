from __future__ import annotations

import asyncio
import json
import os
import sys
import time
from dataclasses import replace
from datetime import UTC, datetime, timedelta
from pathlib import Path
from types import SimpleNamespace
from typing import Any, cast

import pytest

from agentnave import core, provider_health
from agentnave.adapters.base import ParsedProviderResult, PreparedCommand
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codex import CodexAdapter
from agentnave.adapters.grok import GrokAdapter
from agentnave.adapters.pi import PiAdapter
from agentnave.core import (
    _MAX_STDOUT_BYTES,  # pyright: ignore[reportPrivateUsage]
    _RETRY_STALL_QUIET_SECONDS,  # pyright: ignore[reportPrivateUsage]
    _RETRY_STALL_SECONDS,  # pyright: ignore[reportPrivateUsage]
    READ_ONLY_INSTRUCTIONS,
    SUBAGENT_INSTRUCTIONS,
    InvocationManager,
    _InvocationRecord,  # pyright: ignore[reportPrivateUsage]
    _read_limited,  # pyright: ignore[reportPrivateUsage]
    _retry_streak_start,  # pyright: ignore[reportPrivateUsage]
    _stall_guard,  # pyright: ignore[reportPrivateUsage]
)
from agentnave.locks import lock_file, release_and_remove
from agentnave.models import (
    InvocationError,
    InvocationPhase,
    InvocationRequest,
    InvocationStatus,
    ProviderActivity,
)
from agentnave.native_prompts import NativePrompt, PromptChannel
from agentnave.processes import (
    _wait_windows_supervisor_ready,  # pyright: ignore[reportPrivateUsage]
)


def test_doing_since_changes_only_with_displayed_state(tmp_path: Path) -> None:
    record = _InvocationRecord(  # pyright: ignore[reportPrivateUsage]
        InvocationRequest("antigravity", "work", tmp_path)
    )  # pyright: ignore[reportPrivateUsage]
    record.last_activity = ProviderActivity("lifecycle", "thinking", "reasoning")
    record.update_doing(10)
    assert (record.doing, record.doing_since) == ("thinking", 10)
    record.last_activity = ProviderActivity("tool", "tool", "DONE", "read_file")
    record.update_doing(20)
    assert (record.doing, record.doing_since) == ("thinking", 10)
    record.last_activity = ProviderActivity("tool", "tool", "ACTIVE", "read_file")
    record.update_doing(30)
    assert (record.doing, record.doing_since) == ("tool:read_file", 30)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("provider", "host", "cleared"),
    [
        ("codebuddy", "workbuddy-desktop", True),
        ("codebuddy", "cli", False),
        ("claude", "workbuddy-desktop", False),
    ],
)
async def test_workbuddy_product_config_does_not_change_child_codebuddy_login(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, provider: str, host: str, cleared: bool
) -> None:
    product_keys = (
        "ACC_PRODUCT_CONFIG",
        "ACC_PRODUCT_CONFIG_V2",
        "ACC_PRODUCT_CONFIG_V3",
        "ACC_PRODUCT_CONFIG_PATH",
    )
    inherited: dict[str, str] = dict.fromkeys(product_keys, "host-product-config")
    inherited.update(CODEBUDDY_HOST=host, CODEBUDDY_CONFIG_DIR=str(tmp_path))
    for key, value in inherited.items():
        monkeypatch.setenv(key, value)
    script = (
        "import json,os; "
        f"values={{key:os.environ.get(key) for key in {tuple(inherited)!r}}}; "
        "print(json.dumps({'type':'result','result':json.dumps(values),'session_id':'probe'}))"
    )
    manager = InvocationManager()
    request = InvocationRequest(provider, "Check inherited config", tmp_path)
    prepared = replace(manager.prepare(request), argv=(sys.executable, "-c", script))
    try:
        result = await manager.wait(manager.start(request, prepared=prepared), 10)
        assert result is not None and result.status is InvocationStatus.SUCCEEDED
        expected = {**inherited, **(dict.fromkeys(product_keys) if cleared else {})}
        assert json.loads(result.output) == expected
        assert {key: os.environ[key] for key in inherited} == inherited
    finally:
        await manager.shutdown()


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


@pytest.mark.asyncio
async def test_cancelled_run_keeps_the_session_named_at_launch(tmp_path: Path) -> None:
    manager = InvocationManager()
    try:
        invocation = manager.start(
            InvocationRequest("grok", "work", tmp_path),
            prepared=PreparedCommand(
                (sys.executable, "-c", "import time; time.sleep(30)"),
                tmp_path,
                session_id="named-at-launch",
            ),
        )
        result = await manager.cancel(invocation)
        assert result.status is InvocationStatus.CANCELLED
        assert result.session_id == "named-at-launch"
    finally:
        await manager.shutdown()


class FakeAdapter(ClaudeAdapter):
    name = "claude"

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request = replace(
            request,
            prompt=request.prompt.removeprefix(f"{SUBAGENT_INSTRUCTIONS}\n\nAssigned task:\n"),
        )
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
        elif request.prompt == "retry_then_exit":
            wire = (
                json.dumps({"type": "system", "subtype": "api_retry", "error": "server_error"})
                + "\n"
            )
            code = (
                f"import sys; sys.stdout.write({wire!r}); "
                "sys.stdout.write('completed'); sys.stdout.flush()"
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
                "provider stream ended before completion",
            )
        return ParsedProviderResult(InvocationStatus.SUCCEEDED, stdout.decode(), "provider-session")


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
    manager.session_locks = tmp_path / "native-session-locks"
    request = InvocationRequest("claude", "sleep", tmp_path, "existing-session")
    first = manager.start(request)
    try:
        with pytest.raises(ValueError, match="active invocation"):
            manager.start(request)
        # A different provider's identical session identifier is independent.
        other = manager.start(InvocationRequest("grok", "complete", tmp_path, request.session_id))
        assert await manager.wait(other) is not None
        await manager.cancel(first)
        assert not list(manager.session_locks.glob("*.lock"))
        resumed = manager.start(
            InvocationRequest("claude", "complete", tmp_path, request.session_id)
        )
        assert await manager.wait(resumed) is not None
        assert not list(manager.session_locks.glob("*.lock"))
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
        assert "PRIVATE INTERNAL DETAIL" not in str(result)
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
        deadline = time.monotonic() + 10
        while True:
            assert await manager.wait_for_update(invocation_id, 0.05) is None
            output, age = manager.recent_output(invocation_id)
            if output.endswith("中文") or time.monotonic() > deadline:
                break
        assert output == "x" * 600 + "中文"
        assert age is not None
        waiting = asyncio.create_task(manager.wait_for_update(invocation_id, 600))
        await asyncio.sleep(0)
        waiting.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiting
        assert await manager.wait_for_update(invocation_id, 0.01) is None
        # Cancelled before a final answer, the reply seen so far is kept in the result.
        cancelled = await manager.cancel(invocation_id)
        assert cancelled.status is InvocationStatus.CANCELLED
        assert cancelled.output == "x" * 600 + "中文"
    finally:
        await manager.shutdown()


@pytest.mark.skipif(os.name != "posix", reason="POSIX supervisor")
def test_supervisor_ends_provider_group_when_server_dies(tmp_path: Path) -> None:
    import subprocess

    import agentnave.supervisor

    marker = tmp_path / "provider.pid"
    provider = f"import os,time; open({str(marker)!r},'w').write(str(os.getpid())); time.sleep(60)"
    server = (
        "import os,subprocess,sys,time\n"
        "read, write = os.pipe()\n"
        f"subprocess.Popen([sys.executable, '-I', {agentnave.supervisor.__file__!r}, str(write),"
        f" sys.executable, '-c', {provider!r}], pass_fds=(write,), start_new_session=True)\n"
        "time.sleep(60)\n"
    )
    parent = subprocess.Popen([sys.executable, "-c", server])
    try:
        deadline = time.monotonic() + 10
        while not marker.exists() or not marker.read_text():
            assert time.monotonic() < deadline, "provider did not start"
            time.sleep(0.05)
        pid = int(marker.read_text())
        parent.kill()  # SIGKILL: the server dies without any cleanup
        parent.wait()
        deadline = time.monotonic() + 8
        while True:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            assert time.monotonic() < deadline, "provider outlived its dead server"
            time.sleep(0.1)
    finally:
        if parent.poll() is None:
            parent.kill()


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
        assert manager.progress(invocation_id)[0] == "retrying"
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
        doing, output_at = manager.progress(invocation_id)
        assert doing == "writing" and output_at is not None
        assert output_at == manager.progress(invocation_id)[1]  # stable between reads
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
        # An ended call returns its result to the model, which works on it next.
        assert manager.progress(invocation_id)[0] == "thinking"
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


@pytest.mark.asyncio
async def test_zcode_protocol_model_resume_and_provider_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Exercise the real bridge over a fake app-server, including native bidirectional RPC."""
    script = tmp_path / ("zcode.py" if os.name == "nt" else "zcode")
    script.write_text(
        f"#!{sys.executable}\n"
        + r"""
import json, sys
session = None
resume = False
selected = False
thought = False
for line in sys.stdin:
    m = json.loads(line)
    method, p = m.get('method'), m.get('params', {})
    if method is None:
        assert m['error']['code'] == -32601
        continue
    result = {}
    if method in ('session/create', 'session/resume'):
        session = p['sessionId']
        resume = method.endswith('resume')
        print(json.dumps({'id':'prefs','method':'session/requestRuntimePreferences','params':{}}), flush=True)
        if resume:
            # A log line and the previous prompt's late end must not finish this one.
            print('zcode: resuming session', flush=True)
            print(json.dumps({'method':'state.updated','params':{'sessionId':session,'reason':'prompt_completed'}}),flush=True)
        result = {'settings':{'model':{'available':[{'ref':{'providerId':'route','modelId':'model'}}]}}}
    elif method == 'session/setModel':
        assert p['model'] == {'providerId':'route','modelId':'model','options':{'reasoningLevel':'low'}}
        assert p['persistAsWorkspaceLastUsed'] is False
        selected = True
    elif method == 'session/setThoughtLevel':
        assert p['thoughtLevel'] == 'low'
        thought = True
    elif method == 'session/send':
        assert selected and thought
        assert 'Do not call AgentNave tools' in p['content']
    print(json.dumps({'id':m['id'],'result':result}), flush=True)
    if method == 'session/send':
        failed = 'fail-task' in p['content']
        if failed:
            print(json.dumps({'method':'session/event','params':{'sessionId':session,'type':'model.streaming','payload':{'kind':'text_delta','delta':'answer'}}}),flush=True)
            print(json.dumps({'method':'session/event','params':{'sessionId':session,'type':'turn.failed','payload':{'error':{'type':'auth','message':'authentication required'},'turnPhase':'model'}}}),flush=True)
        else:
            print(json.dumps({'method':'session/event','params':{'sessionId':session,'type':'turn.completed','payload':{'response':'resumed' if resume else 'answer'}}}),flush=True)
        print(json.dumps({'method':'state.updated','params':{'sessionId':session,'reason':'prompt_failed' if failed else 'prompt_completed'}}),flush=True)
""",
        encoding="utf-8",
    )
    script.chmod(0o755)
    if os.name == "nt":
        (tmp_path / "zcode.cmd").write_text(f'@"{sys.executable}" "%~dp0zcode.py" %*\n')
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
    manager = InvocationManager()
    try:
        options = {"model": "route/model", "effort": "low"}
        invocation = manager.start(
            InvocationRequest("zcode", "task", tmp_path, provider_options=options)
        )
        result = await manager.wait(invocation, 10)
        assert result is not None and result.status is InvocationStatus.SUCCEEDED
        assert result.output == "answer" and result.session_id
        resumed = manager.start(
            InvocationRequest(
                "zcode",
                "continue",
                tmp_path,
                session_id=result.session_id,
                provider_options=options,
            )
        )
        continued = await manager.wait(resumed, 10)
        assert continued is not None and continued.status is InvocationStatus.SUCCEEDED
        assert continued.session_id == result.session_id and continued.output == "resumed"
        failed = manager.start(
            InvocationRequest("zcode", "fail-task", tmp_path, provider_options=options)
        )
        failure = await manager.wait(failed, 10)
        assert failure is not None and failure.status is InvocationStatus.BLOCKED
        assert failure.output == "answer" and failure.session_id
        assert failure.error is not None and "authentication required" in failure.error.message
    finally:
        await manager.shutdown()


def _stall_ready_with_every_signal(monkeypatch: pytest.MonkeyPatch) -> None:
    """Arm a stall guard that finishes in the same wait tick as the real trigger.

    The guard is what the retry policy would produce; holding it on a gate until the wait
    has already seen the cancel or the exit reproduces the interleaving where both futures
    are ready when the manager looks at them.
    """
    gate = asyncio.Event()
    real_wait = asyncio.wait

    async def armed_guard(_record: object) -> None:
        await gate.wait()

    async def wait_with_armed_guard(futures: Any, **kwargs: Any) -> Any:
        futures = tuple(futures)
        done, pending = await real_wait(futures, **kwargs)
        if len(futures) != 4:
            return done, pending
        gate.set()
        await real_wait((futures[3],))
        return {*done, futures[3]}, pending - {futures[3]}

    monkeypatch.setattr(core, "_stall_guard", armed_guard)
    monkeypatch.setattr(core.asyncio, "wait", wait_with_armed_guard)


@pytest.mark.asyncio
async def test_completed_provider_wins_when_the_stall_guard_fires_in_the_same_tick(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    _stall_ready_with_every_signal(monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(InvocationRequest("claude", "retry_then_exit", tmp_path))
    try:
        result = await manager.wait(invocation)
        assert result is not None
        assert result.status is InvocationStatus.SUCCEEDED
        assert result.output.endswith("completed")
        assert result.session_id == "provider-session"
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_explicit_cancel_wins_when_the_stall_guard_fires_in_the_same_tick(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    _stall_ready_with_every_signal(monkeypatch)
    manager = InvocationManager()
    invocation = manager.start(InvocationRequest("claude", "sleep", tmp_path))
    try:
        result = await manager.cancel(invocation)
        assert result.status is InvocationStatus.CANCELLED
        assert result.error is not None and result.error.code == "cancelled"
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_a_stall_guard_that_fires_before_the_provider_exits_still_fails(
    tmp_path: Path, fake_adapter: None, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def stalled_guard(_record: object) -> None:
        return

    monkeypatch.setattr(core, "_stall_guard", stalled_guard)
    manager = InvocationManager()
    invocation = manager.start(InvocationRequest("claude", "sleep", tmp_path))
    try:
        result = await manager.wait(invocation)
        assert result is not None and result.status is InvocationStatus.FAILED
        assert result.error is not None and result.error.code == "provider_error"
    finally:
        await manager.shutdown()


def test_a_provider_that_keeps_retrying_is_stopped_after_the_window() -> None:
    stuck = SimpleNamespace(
        retry_since=time.monotonic() - _RETRY_STALL_SECONDS - 1,
        retry_at=time.monotonic(),
        prompts=PromptChannel(),
    )
    asyncio.run(asyncio.wait_for(_stall_guard(cast(Any, stuck), interval=0.01), timeout=2))


def test_a_recovered_provider_is_not_stopped() -> None:
    # One retry long ago, then silence: the provider recovered, so the streak is over.
    once = time.monotonic() - _RETRY_STALL_SECONDS - 1
    recovered = SimpleNamespace(retry_since=once, retry_at=once, prompts=PromptChannel())
    with pytest.raises(TimeoutError):
        asyncio.run(
            asyncio.wait_for(
                _stall_guard(cast(Any, recovered), interval=0.01),
                timeout=0.2,
            )
        )


def test_a_quiet_provider_is_not_stopped() -> None:
    idle = SimpleNamespace(retry_since=None, retry_at=None, prompts=PromptChannel())
    with pytest.raises(TimeoutError):
        asyncio.run(
            asyncio.wait_for(
                _stall_guard(cast(Any, idle), interval=0.01),
                timeout=0.2,
            )
        )


def test_a_retry_after_a_quiet_gap_starts_a_new_streak() -> None:
    quiet = _RETRY_STALL_QUIET_SECONDS
    # First retry ever.
    assert _retry_streak_start(None, None, 100.0) == 100.0
    # Still inside the window: the same streak continues.
    assert _retry_streak_start(10.0, 90.0, 100.0) == 10.0
    # Silent for longer than the window: this is a new run.
    assert _retry_streak_start(10.0, 100.0 - quiet - 1, 100.0) == 100.0


def test_read_only_tasks_get_the_read_only_instruction(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prompts: list[str] = []

    class Capture:
        def prepare(self, request: InvocationRequest) -> InvocationRequest:
            prompts.append(request.prompt)
            return request

    def capture(_: str) -> Capture:
        return Capture()

    monkeypatch.setattr("agentnave.core.get_adapter", capture)
    manager = InvocationManager()
    manager.prepare(InvocationRequest("codex", "task", tmp_path, read_only=True))
    manager.prepare(InvocationRequest("codex", "task", tmp_path))
    assert READ_ONLY_INSTRUCTIONS in prompts[0] and prompts[0].endswith("Assigned task:\ntask")
    assert READ_ONLY_INSTRUCTIONS not in prompts[1] and SUBAGENT_INSTRUCTIONS in prompts[1]


@pytest.mark.asyncio
async def test_pending_prompts_wake_once_and_cancel_denies_before_stopping(tmp_path: Path) -> None:
    script = """
import json, sys, time
json.loads(sys.stdin.readline())
print(json.dumps({'type':'system', 'subtype':'init'}), flush=True)
for prompt_id in ['a' * 32, 'b' * 32]:
    print(json.dumps({'type':'agentnave_prompt', 'prompt_id':prompt_id,
                     'kind':'permission', 'tool_name':'Bash', 'tool_input':'echo safe'}), flush=True)
while line := sys.stdin.readline():
    pass
"""
    manager = InvocationManager()
    sent: list[dict[str, object]] = []
    adapter = ClaudeAdapter()
    original = adapter.encode_prompt_answer

    # Capture what the real stdin writer is asked to deliver, including cancel's deny.
    def encode(answer: dict[str, object]) -> bytes:
        sent.append(answer)
        return original(answer)

    from unittest.mock import patch

    def encode_hook(_self: ClaudeAdapter, answer: dict[str, object]) -> bytes:
        return encode(answer)

    with patch.object(ClaudeAdapter, "encode_prompt_answer", encode_hook):
        invocation = manager.start(
            InvocationRequest("claude", "work", tmp_path),
            prepared=PreparedCommand(
                (sys.executable, "-c", script),
                tmp_path,
                b'{"prompt":"work"}\n',
                keep_stdin_open=True,
            ),
        )
        try:
            notices = [
                await manager.wait_for_update(invocation, 3),
                await manager.wait_for_update(invocation, 3),
            ]
            assert all(isinstance(notice, NativePrompt) for notice in notices)
            assert await manager.wait_for_update(invocation, 0) is None
            assert manager.progress(invocation)[0] is None
            await manager.answer_prompt(invocation, "a" * 32, "allow")
            with pytest.raises(ValueError, match="already answered"):
                await manager.answer_prompt(invocation, "a" * 32, "deny")
            result = await manager.cancel(invocation)
            assert result.status is InvocationStatus.CANCELLED
            assert [answer["decision"] for answer in sent] == ["allow", "deny"]
            assert manager.pending_prompts(invocation) == []
            history = manager.prompt_history(invocation)
            assert history[1]["answer"] is not None
            assert cast(dict[str, object], history[1]["answer"])["actor"] == "service_cancel"
            with pytest.raises(ValueError, match="ended"):
                await manager.answer_prompt(invocation, "b" * 32, "allow")
        finally:
            await manager.shutdown()


@pytest.mark.asyncio
async def test_pending_prompt_resets_retry_stall_clock() -> None:
    channel = PromptChannel()
    channel.receive(NativePrompt("c" * 32, "permission", "Bash"))
    record = SimpleNamespace(
        retry_since=time.monotonic() - _RETRY_STALL_SECONDS - 1,
        retry_at=time.monotonic(),
        prompts=channel,
    )
    guard = asyncio.create_task(_stall_guard(cast(Any, record), interval=0.001))
    async with asyncio.timeout(5):
        while record.retry_since is not None:
            await asyncio.sleep(0.001)
    assert not guard.done() and record.retry_at is None
    guard.cancel()


def test_closed_prompt_is_never_reported_again() -> None:
    channel = PromptChannel()
    prompt = NativePrompt("d" * 32, "permission", "Bash")
    assert channel.receive(prompt)
    assert not channel.receive(NativePrompt("d" * 32, "permission", "", status="closed"))
    assert prompt.status == "closed" and prompt.closed_at
    assert channel.take() is None
    assert not channel.receive(NativePrompt("d" * 32, "permission", "Bash"))


@pytest.mark.parametrize(
    ("tool_input", "masked"),
    [
        ({"command": "curl -H 'Authorization: Bearer abcdefgh12345678' x"}, "Authorization: ****"),
        ({"command": "API_KEY=supersecret123 run"}, "API_KEY=****"),
        ({"env": {"GITHUB_TOKEN": "ghp_" + "a" * 36}}, '"GITHUB_TOKEN": "****"'),
        ({"command": "git clone https://me:pa55word@host/x.git"}, "https://****@host"),
        ({"command": "echo env-secret-value-1"}, "echo ****"),
        ({"file_path": "/repo/token-utils/a.md"}, "/repo/token-utils/a.md"),
        ({"command": 'PASSWORD="two words" run'}, 'PASSWORD=\\"****\\" run'),
        ({"password": "two words"}, '"password": "****"'),
        ({"command": 'PASSWORD="it\'s two" run'}, 'PASSWORD=\\"****\\" run'),
        ({"command": "API_KEY_FOR_A_VERY_LONG_PRIVATE_GATEWAY_SERVICE_NAME=xyz-1234 run"}, "=****"),
    ],
)
def test_prompt_content_hides_credentials_but_stays_json(
    tool_input: dict[str, object], masked: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SERVICE_TOKEN", "env-secret-value-1")
    prompt = NativePrompt("e" * 32, "permission", "Bash", tool_input=json.dumps(tool_input))
    assert masked in prompt.payload()["tool_input"]
    assert json.loads(prompt.history()["tool_input"])  # type: ignore[arg-type]


def test_prompt_content_hides_credentials_before_cutting_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SERVICE_TOKEN", "env-secret-value-1")
    prompt = NativePrompt(
        "e" * 32, "permission", "Bash", tool_input="x" * 3990 + "env-secret-value-1"
    )
    assert prompt.tool_input.endswith("****") and "env-sec" not in prompt.tool_input
    # Quotes and backslashes in a secret are escaped once the input is JSON-encoded.
    monkeypatch.setenv("SERVICE_PASSWORD", 'demo"pass\\word123')
    encoded = json.dumps({"command": "echo 'demo\"pass\\word123'"})
    assert "pass" not in NativePrompt("f" * 32, "permission", "Bash", tool_input=encoded).tool_input
    # A command carrying JSON of its own escapes the secret a second time.
    twice = json.dumps(
        {"command": "curl -d " + json.dumps(json.dumps({"v": 'demo"pass\\word123'}))}
    )
    assert "pass" not in NativePrompt("f" * 32, "permission", "Bash", tool_input=twice).tool_input
    mixed = json.dumps(json.dumps('demo"pass\\word123', ensure_ascii=False), ensure_ascii=True)
    assert "pass" not in NativePrompt("f" * 32, "permission", "Bash", tool_input=mixed).tool_input


def test_provider_health_cooldown_keeps_deadline_and_clears_success(tmp_path: Path) -> None:
    now = datetime.now(UTC)
    provider_health.record(tmp_path, "claude", "quota", "a")
    assert provider_health.active(tmp_path) == {}
    provider_health.record(tmp_path, "claude", "quota", "a", now + timedelta(days=20))
    health = provider_health.active(tmp_path)[("claude", "a")]
    assert health["until"] is not None
    assert (
        timedelta(days=7, hours=23)
        < datetime.fromisoformat(health["until"]) - now
        <= timedelta(days=8, seconds=1)
    )
    provider_health.record(tmp_path, "claude", "quota", "a", now + timedelta(days=20))
    assert provider_health.active(tmp_path)[("claude", "a")] == health
    provider_health.check(tmp_path, [("claude", "b")])
    with pytest.raises(ValueError):
        provider_health.check(tmp_path, [("claude", "a")])
    provider_health.record(tmp_path, "claude", "auth", "a")
    provider_health.record(tmp_path, "claude", "billing", "b")
    assert provider_health.active(tmp_path)[("claude", "a")] == {"code": "auth", "until": None}
    billing = provider_health.active(tmp_path)[("claude", "b")]["until"]
    assert billing is not None and timedelta(minutes=29) < datetime.fromisoformat(
        billing
    ) - now <= timedelta(minutes=30, seconds=1)
    provider_health.clear(tmp_path, "claude", "a")
    assert set(provider_health.active(tmp_path)) == {("claude", "b")}
    provider_health.clear(tmp_path, "claude")
    assert provider_health.active(tmp_path) == {}


def test_provider_quota_extends_only_for_later_explicit_reset(tmp_path: Path) -> None:
    now = datetime.now(UTC)
    provider_health.record(tmp_path, "codex", "quota", "a", now + timedelta(hours=1))
    first = provider_health.active(tmp_path)[("codex", "a")]
    provider_health.record(tmp_path, "codex", "quota", "a", now + timedelta(minutes=30))
    provider_health.record(tmp_path, "codex", "quota", "a")
    assert provider_health.active(tmp_path)[("codex", "a")] == first
    provider_health.record(tmp_path, "codex", "quota", "a", now + timedelta(hours=2))
    assert provider_health.active(tmp_path)[("codex", "a")]["until"] != first["until"]


def test_adapters_parse_explicit_quota_resets() -> None:
    now = datetime.now(UTC)
    epoch = int((now + timedelta(hours=3)).timestamp())
    claude = json.dumps(
        {"type": "result", "is_error": True, "errors": [f"usage limit reached|{epoch}"]}
    ).encode()
    codex = json.dumps(
        {"type": "turn.failed", "error": {"message": "usage limit reached", "resets_at": epoch}}
    ).encode()
    assert ClaudeAdapter().quota_reset(claude, b"", now) == datetime.fromtimestamp(epoch, UTC)
    assert CodexAdapter().quota_reset(codex, b"", now) == datetime.fromtimestamp(epoch, UTC)
    assert CodexAdapter().quota_reset(
        b'{"type":"turn.failed","error":{"message":"quota exceeded; try again in 2 hours"}}',
        b"",
        now,
    ) == now + timedelta(hours=2)
    assert (
        ClaudeAdapter().quota_reset(b'{"type":"result","errors":["quota exceeded"]}', b"", now)
        is None
    )


@pytest.mark.parametrize("first", ["quota", "billing"])
def test_billing_quota_merge_preserves_longer_deadline(tmp_path: Path, first: str) -> None:
    reset = datetime.now(UTC) + timedelta(days=5)
    order = ("quota", "billing") if first == "quota" else ("billing", "quota")
    for category in order:
        provider_health.record(tmp_path, "codex", cast(provider_health.Code, category), "x", reset)
    entry = provider_health.active(tmp_path)[("codex", "x")]
    assert entry["code"] == "billing" and entry["until"] is not None
    assert datetime.fromisoformat(entry["until"]) == reset
    assert "reset" not in entry


@pytest.mark.parametrize("legacy", [True, False])
def test_clear_model_also_clears_cli_default(tmp_path: Path, legacy: bool) -> None:
    path = tmp_path / "provider-health.json"
    default = {"code": "auth", "until": None}
    item = default if legacy else {"default": default, "models": {"other": default}}
    path.write_text(json.dumps({"version": 1, "providers": {"claude": item}}))
    assert provider_health.clear(tmp_path, "claude", "chosen")
    health = provider_health.active(tmp_path)
    assert ("claude", None) not in health
    assert (("claude", "other") in health) is not legacy


def test_success_clears_cli_default_and_matching_model(tmp_path: Path) -> None:
    provider_health.record(tmp_path, "claude", "auth")
    provider_health.record(tmp_path, "claude", "auth", "chosen")
    provider_health.record(tmp_path, "claude", "auth", "other")
    result = core.InvocationResult(InvocationStatus.SUCCEEDED, "claude", "ok", None, 0)
    provider_health.observe_result(tmp_path, result, None, [], "chosen")
    assert set(provider_health.active(tmp_path)) == {("claude", "other")}


def test_stream_blocker_without_model_records_cli_cooldown(tmp_path: Path) -> None:
    record = _InvocationRecord(InvocationRequest("claude", "work", tmp_path), health_root=tmp_path)
    adapter = ClaudeAdapter()
    model_event: dict[str, object] = {
        "type": "assistant",
        "message": {"model": "reported", "content": []},
    }
    record.observe_event(json.dumps(model_event).encode(), adapter)
    assert record.reported_model == "reported"
    event = {"type": "system", "subtype": "api_retry", "error": "authentication_failed"}
    record.observe_event(json.dumps(event).encode(), adapter)
    assert provider_health.active(tmp_path)[("claude", None)]["code"] == "auth"


def test_retry_after_in_json_array_and_before_other_fields() -> None:
    now = datetime.now(UTC)
    claude = {"type": "result", "is_error": True, "errors": ["quota exceeded; Retry-After: 120"]}
    codex = {
        "type": "turn.failed",
        "error": {
            "headers": {"Retry-After": "120", "x-request-id": "1"},
            "message": "quota exceeded",
        },
    }
    for adapter, event in ((ClaudeAdapter(), claude), (CodexAdapter(), codex)):
        assert adapter.quota_reset(json.dumps(event).encode(), b"", now) == now + timedelta(
            seconds=120
        )
    date = "Wed, 21 Oct 2037 07:28:00 GMT"
    codex["error"] = {"headers": {"Retry-After": date, "next": "value"}}
    assert CodexAdapter().quota_reset(json.dumps(codex).encode(), b"", now) == datetime(
        2037, 10, 21, 7, 28, tzinfo=UTC
    )
    codex["error"] = {"headers": {"Retry-After": "120.5"}}
    assert CodexAdapter().quota_reset(json.dumps(codex).encode(), b"", now) is None


def test_provider_health_bad_data_and_busy_lock_fail_open(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "provider-health.json"
    for content in ("{", "[]", '{"version":true,"providers":{}}', '{"version":2,"providers":{}}'):
        path.write_text(content)
        assert provider_health.active(tmp_path) == {}
    provider_health.record(tmp_path, "claude", "quota")
    assert json.loads(path.read_text())["version"] == 2
    now = datetime.now(UTC)
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "providers": {
                    "expired": {"code": "quota", "until": (now - timedelta(seconds=1)).isoformat()},
                    "future": {"code": "auth", "until": (now + timedelta(days=2)).isoformat()},
                    "naive": {"code": "quota", "until": "2026-11-08T08:30:00"},
                    "invalid": {"code": "rate_limit", "until": "invalid"},
                    "claude": {
                        "code": "billing",
                        "until": (now + timedelta(minutes=5)).isoformat(),
                    },
                },
            }
        )
    )
    assert set(provider_health.active(tmp_path)) == {("claude", None), ("future", None)}
    lock = tmp_path / "locks" / "provider-health.lock"
    handle = lock_file(lock, "busy")
    before = path.read_bytes()
    try:
        provider_health.clear(tmp_path, "claude")
        provider_health.record(tmp_path, "grok", "auth")
        assert path.read_bytes() == before
    finally:
        release_and_remove(handle, lock)
    provider_health.clear(tmp_path, "absent")
    assert set(json.loads(path.read_text())["providers"]) == {
        "expired",
        "future",
        "naive",
        "invalid",
        "claude",
    }

    def fail_write(_path: Path, _data: bytes) -> None:
        raise OSError("disk unavailable")

    monkeypatch.setattr(provider_health, "write_file", fail_write)
    provider_health.record(tmp_path, "grok", "auth")
    assert set(provider_health.active(tmp_path)) == {("claude", None), ("future", None)}


@pytest.mark.parametrize(
    ("code", "message", "reported", "expected"),
    [
        ("provider_error", "retrying", {"insufficient_quota"}, "quota"),
        ("invalid_api_key", "", set[str](), "auth"),
        ("billing_error", "", set[str](), "billing"),
        ("permission_denied", "login required", set[str](), None),
        ("provider_failed", "not logged in; login required", set[str](), "auth"),
        ("provider_error", "insufficient balance", set[str](), "billing"),
        ("provider_error", "429 rate limit exceeded", set[str](), None),
        ("provider_error", "429: weekly limit exceeded", set[str](), "quota"),
        ("rate_limit", "too many requests", set[str](), None),
        ("provider_failed", "network timeout; retrying", set[str](), None),
    ],
)
def test_provider_health_classifies_only_account_blockers(
    code: str, message: str, reported: set[str], expected: provider_health.Code | None
) -> None:
    assert provider_health.classify(InvocationStatus.FAILED, code, message, reported) == expected
    assert provider_health.classify(InvocationStatus.SUCCEEDED, code, message, reported) is None


@pytest.mark.asyncio
@pytest.mark.parametrize("reported", [True, False])
async def test_health_survives_retry_stall_and_success_clears_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, reported: bool
) -> None:
    event = (
        {"type": "system", "subtype": "api_retry", "error": "authentication_failed"}
        if reported
        else {
            "type": "result",
            "is_error": True,
            "errors": ["weekly limit reached; try again in 2 hours"],
        }
    )
    manager = InvocationManager()
    manager.health_root = tmp_path

    observed_during_stall: dict[tuple[str, str | None], provider_health.Health] = {}

    async def stalled(record: Any) -> None:
        nonlocal observed_during_stall
        while record.last_event_at is None:
            await asyncio.sleep(0.001)
        observed_during_stall = provider_health.active(tmp_path)

    monkeypatch.setattr(core, "_stall_guard", stalled)
    try:
        script = f"import time; print({json.dumps(event)!r}, flush=True); time.sleep(30)"
        invocation = manager.start(
            InvocationRequest("claude", "work", tmp_path, provider_options={"model": "test"}),
            prepared=PreparedCommand((sys.executable, "-c", script), tmp_path),
        )
        result = await manager.wait(invocation)
        assert result is not None and result.status is InvocationStatus.FAILED
        assert result.error is not None and result.error.code == "provider_error"
        expected = "auth" if reported else "quota"
        assert observed_during_stall[("claude", "test")]["code"] == expected
        assert provider_health.active(tmp_path)[("claude", "test")]["code"] == expected

        # Success on the same model clears its breaker.
        async def no_stall(_record: Any) -> None:
            await asyncio.Event().wait()

        monkeypatch.setattr(core, "_stall_guard", no_stall)
        script = 'print(\'{"type":"result","result":"ok","is_error":false}\')'
        successful = manager.start(
            InvocationRequest("claude", "work", tmp_path, provider_options={"model": "test"}),
            prepared=PreparedCommand((sys.executable, "-c", script), tmp_path),
        )
        result = await manager.wait(successful)
        assert result is not None and result.status is InvocationStatus.SUCCEEDED
        assert provider_health.active(tmp_path) == {}
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_devin_acp_selects_options_asks_boss_and_resumes_without_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Exercise the real ACP bridge over a fake `devin acp`, including a permission request."""
    script = tmp_path / ("devin.py" if os.name == "nt" else "devin")
    script.write_text(
        f"#!{sys.executable}\n"
        + r"""
import json, sys
assert sys.argv[1:] == ['acp']
config = {'model': 'swe-2-flash-medium', 'mode': 'accept-edits'}
choices = {'model': ['swe-2-flash-medium', 'swe-1-6-slow'], 'mode': ['accept-edits', 'bypass']}
def send(value):
    print(json.dumps(value), flush=True)
def options():
    return [{'id': k, 'currentValue': v, 'options': [{'value': c} for c in choices[k]]}
            for k, v in config.items()]
def update(session, value):
    send({'jsonrpc': '2.0', 'method': 'session/update',
          'params': {'sessionId': session, 'update': value}})
for line in sys.stdin:
    m = json.loads(line)
    method, p = m.get('method'), m.get('params', {})
    if method is None:
        continue
    result = {}
    if method == 'initialize':
        result = {'protocolVersion': 1}
    elif method == 'session/new':
        result = {'sessionId': 'fake-session', 'configOptions': options()}
    elif method == 'session/load':
        # History replays before the load reply and must not count as this turn.
        update(p['sessionId'], {'sessionUpdate': 'agent_message_chunk',
                                'content': {'type': 'text', 'text': 'old reply'}})
        result = {'configOptions': options()}
    elif method == 'session/set_config_option':
        if p['value'] not in choices[p['configId']]:
            send({'jsonrpc': '2.0', 'id': m['id'],
                  'error': {'code': -32602, 'message': 'Invalid params', 'data': 'Invalid value'}})
            continue
        config[p['configId']] = p['value']
        result = {'configOptions': options()}
    elif method == 'session/prompt':
        session = p['sessionId']
        update(session, {'sessionUpdate': 'agent_thought_chunk',
                         'content': {'type': 'text', 'text': 'private thought'}})
        if 'continue' in p['prompt'][0]['text']:
            update(session, {'sessionUpdate': 'agent_message_chunk',
                             'content': {'type': 'text', 'text': 'resumed'}})
        else:
            assert config == {'model': 'swe-1-6-slow', 'mode': 'bypass'}
            update(session, {'sessionUpdate': 'agent_message_chunk',
                             'content': {'type': 'text', 'text': 'before the tool'}})
            update(session, {'sessionUpdate': 'tool_call', 'toolCallId': 'c1',
                             'title': 'Ran echo secret-title', 'kind': 'execute',
                             'rawInput': {'command': 'echo draft'},
                             '_meta': {'cognition.ai/inferenceToolName': 'exec'}})
            update(session, {'sessionUpdate': 'tool_call_update', 'toolCallId': 'c1',
                             'rawInput': {'command': 'echo hi'}})
            # A null field means unchanged.
            update(session, {'sessionUpdate': 'tool_call_update', 'toolCallId': 'c1',
                             'rawInput': None})
            send({'jsonrpc': '2.0', 'id': 'perm-1', 'method': 'session/request_permission',
                  'params': {'sessionId': session,
                             'toolCall': {'toolCallId': 'c1', 'rawInput': None},
                             'options': [{'optionId': 'once', 'kind': 'allow_once'},
                                         {'optionId': 'always', 'kind': 'allow_always'},
                                         {'optionId': 'no', 'kind': 'reject_once'}]}})
            reply = json.loads(sys.stdin.readline())
            assert reply['id'] == 'perm-1'
            chosen = reply['result']['outcome']['optionId']
            update(session, {'sessionUpdate': 'tool_call_update', 'toolCallId': 'c1',
                             'status': 'completed'})
            update(session, {'sessionUpdate': 'agent_message_chunk',
                             'content': {'type': 'text', 'text': f'answer {chosen}'}})
        result = {'stopReason': 'end_turn'}
    send({'jsonrpc': '2.0', 'id': m['id'], 'result': result})
""",
        encoding="utf-8",
    )
    script.chmod(0o755)
    if os.name == "nt":
        (tmp_path / "devin.cmd").write_text(f'@"{sys.executable}" "%~dp0devin.py" %*\n')
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
    manager = InvocationManager()
    try:
        options = {"model": "swe-1-6-slow", "permission_mode": "dangerous"}
        invocation = manager.start(
            InvocationRequest("devin", "use a tool", tmp_path, provider_options=options)
        )
        prompt = await manager.wait_for_update(invocation, 10)
        assert isinstance(prompt, NativePrompt)
        assert (prompt.kind, prompt.tool_name) == ("permission", "exec")
        assert "echo hi" in prompt.tool_input and "secret-title" not in prompt.tool_input
        assert "draft" not in prompt.tool_input
        await manager.answer_prompt(invocation, prompt.prompt_id, "allow")
        result = await manager.wait(invocation, 10)
        assert result is not None and result.status is InvocationStatus.SUCCEEDED
        assert result.output == "answer once"
        assert (result.session_id, result.reported_model) == ("fake-session", "swe-1-6-slow")

        resumed = await manager.wait(
            manager.start(
                InvocationRequest("devin", "continue", tmp_path, session_id="fake-session")
            ),
            10,
        )
        assert resumed is not None and resumed.status is InvocationStatus.SUCCEEDED
        assert (resumed.output, resumed.session_id) == ("resumed", "fake-session")

        unavailable = await manager.wait(
            manager.start(
                InvocationRequest(
                    "devin", "hi", tmp_path, provider_options={"model": "claude-sonnet-5-5"}
                )
            ),
            10,
        )
        assert unavailable is not None and unavailable.status is InvocationStatus.FAILED
        assert unavailable.error is not None
        assert "available: swe-2-flash-medium, swe-1-6-slow" in unavailable.error.message
    finally:
        await manager.shutdown()


def test_prepare_appends_extra_instructions_after_read_only_rules(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    exe = bin_dir / ("claude.cmd" if os.name == "nt" else "claude")
    exe.write_text("#!/bin/sh\n", encoding="utf-8")
    exe.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    manager = InvocationManager()
    writable = manager.prepare(
        InvocationRequest("claude", "edit file", tmp_path, extra_instructions="Extra rule.")
    )
    assert writable.stdin is not None
    prefix, body = writable.stdin.decode("utf-8").split("\n\nAssigned task:\n", 1)
    assert body == "edit file"
    assert prefix.endswith("Extra rule.")
    assert "Read-only:" not in prefix

    readonly = manager.prepare(
        InvocationRequest(
            "claude", "inspect file", tmp_path, read_only=True, extra_instructions="Extra rule."
        )
    )
    assert readonly.stdin is not None
    ro_prefix = readonly.stdin.decode("utf-8").split("\n\nAssigned task:\n", 1)[0]
    assert ro_prefix.index("Read-only: do not modify files") < ro_prefix.index("Extra rule.")

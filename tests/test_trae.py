from __future__ import annotations

import asyncio
import json
import sys
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path

import pytest

from agentnave.adapters import get_adapter
from agentnave.adapters.trae import TraeAdapter
from agentnave.core import InvocationManager
from agentnave.host import (
    Host,
    _match,  # pyright: ignore[reportPrivateUsage]
    detect_host,
    host_session_id,
)
from agentnave.members import PermissionFile, ProviderExcludedError, apply_member, check_excluded
from agentnave.models import InvocationRequest, InvocationStatus, ProviderOption
from agentnave.naming import MemberName
from agentnave.native_sessions import session_file, session_model


def _stream(*events: dict[str, object]) -> bytes:
    return b"\n".join(json.dumps(event).encode() for event in events) + b"\n"


START: dict[str, object] = {"type": "thread.started", "thread_id": "native-trae-session"}
REPLY: dict[str, object] = {
    "type": "item.completed",
    "item": {"type": "agent_message", "text": "TRAE reply"},
}
DONE: dict[str, object] = {"type": "turn.completed", "usage": {"output_tokens": 4}}


def test_trae_identity_keeps_cli_and_desktop_in_one_provider_without_inventing_chat(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    for executable in (
        "/Applications/Trae CN.app/Contents/MacOS/Electron",
        "/Applications/TRAE.app/Contents/MacOS/Electron",
        "/tools/bin/traecli",
        "/tools/dist/current/traex",
        "traecli.exe",
    ):
        assert _match(executable) == "trae"
    monkeypatch.setenv("AGENTNAVE_HOST", "trae")
    monkeypatch.setenv("AGENTNAVE_HOST_APP", "trae-desktop")
    host = detect_host()
    assert host == Host("trae", "env", "trae-desktop")
    assert host.provider == "trae"
    with pytest.raises(ProviderExcludedError):
        check_excluded("task", "trae", frozenset({host.provider}))
    monkeypatch.delenv("AGENTNAVE_HOST_SESSION_ID")
    monkeypatch.setenv("TRAE_HOME", str(tmp_path))
    (tmp_path / "traecli.toml").write_text('model = "not-a-reported-model"')
    assert host_session_id(host) is None
    assert session_file(host, "native-trae-session") is None
    assert session_model(host, "native-trae-session") is None
    adapter = get_adapter("trae")
    assert isinstance(adapter, TraeAdapter)
    assert adapter.session_model("native-trae-session", since=datetime.now(UTC)) is None
    assert adapter.event_model({"type": "turn.completed", "model": "unverified"}) is None
    assert MemberName("test", "trae").identity([])["cli"] == "TRAE CLI"


def test_trae_uses_only_documented_native_arguments(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def executable_path(_adapter: TraeAdapter) -> str:
        return "traecli"

    monkeypatch.setattr(TraeAdapter, "executable_path", executable_path)
    request = InvocationRequest("trae", "work", tmp_path)
    adapter = TraeAdapter()
    prepared = adapter.prepare(request)
    assert prepared.argv == (
        "traecli",
        "exec",
        "--json",
        "--skip-git-repo-check",
        "-",
    )
    assert prepared.cwd == tmp_path and prepared.stdin == b"work"
    assert adapter.prepare(replace(request, session_id="native-id")).argv == (
        "traecli",
        "exec",
        "resume",
        "--json",
        "--skip-git-repo-check",
        "native-id",
        "-",
    )
    assert adapter.option_args(
        replace(
            request,
            provider_options={
                "model": "native-model",
                "permission_mode": "auto",
                "sandbox": "workspace-write",
            },
        )
    ) == ("--model", "native-model", "--permission-mode", "auto", "--sandbox", "workspace-write")
    assert not adapter.supports_restricted_tools
    with pytest.raises(ValueError, match="restricted_tools"):
        adapter.prepare(replace(request, provider_options={"restricted_tools": True}))
    with pytest.raises(ValueError, match="resume does not support sandbox"):
        adapter.prepare(
            replace(request, session_id="id", provider_options={"sandbox": "read-only"})
        )
    for options in (
        {"effort": "high"},
        {"approve_for_me": True},
        {"permission_mode": "bypassPermissions"},
        {"model": True},
        {"sandbox": "invalid"},
        {"skip_git_repo_check": "yes"},
    ):
        with pytest.raises(ValueError):
            adapter.prepare(replace(request, provider_options=options))


def test_trae_permissions_are_applied_by_service_entry_and_reported() -> None:
    assert apply_member(None, "task", "trae", {}, frozenset()) == (
        {"permission_mode": "auto"},
        ["permission_mode"],
    )
    tiers = PermissionFile(version=1, clis={"trae": "full"})
    assert apply_member(None, "task", "trae", {}, frozenset(), permissions=tiers) == (
        {"permission_mode": "bypass_permissions"},
        ["permission_mode"],
    )
    explicit: dict[str, ProviderOption] = {"sandbox": "read-only"}
    assert apply_member(None, "task", "trae", explicit, frozenset(), permissions=tiers) == (
        explicit,
        [],
    )
    assert apply_member(None, "discussion", "trae", {}, frozenset(), restricted=True) == ({}, [])


def test_trae_terminal_results_preserve_reply_session_and_fail_closed() -> None:
    adapter = TraeAdapter()
    result = adapter.parse(0, _stream(START, REPLY, DONE), b"")
    assert result.status == InvocationStatus.SUCCEEDED
    assert (result.output, result.session_id) == ("TRAE reply", "native-trae-session")
    empty_turn: dict[str, object] = {"type": "turn.completed", "usage": {}}
    for events in ((START, REPLY), (START, empty_turn)):
        assert adapter.parse(0, _stream(*events), b"").status == InvocationStatus.FAILED
    failed: dict[str, object] = {"type": "turn.failed", "error": {"message": "Not logged in"}}
    result = adapter.parse(1, _stream(START, REPLY, failed), b"")
    assert result.status == InvocationStatus.BLOCKED
    assert (result.output, result.session_id) == ("TRAE reply", "native-trae-session")
    assert adapter.parse(1, _stream(START, REPLY, DONE), b"crash").status == InvocationStatus.FAILED
    assert adapter.parse(0, b'{"type":[]}\nmalformed\n', b"").status == InvocationStatus.FAILED


def test_trae_activity_and_capture_keep_public_output_without_tool_payloads(tmp_path: Path) -> None:
    adapter = TraeAdapter()
    capture = adapter.prepare(InvocationRequest("trae", "work", tmp_path)).capture_line
    assert capture is not None
    tool: dict[str, object] = {
        "type": "item.started",
        "item": {
            "type": "command_execution",
            "id": "tool-1",
            "status": "in_progress",
            "aggregated_output": "private",
        },
    }
    activity = adapter.activity(tool)
    assert activity is not None and activity.kind == "tool" and activity.tool_call_id == "tool-1"
    assert activity.message is None and capture(_stream(tool)) == b""
    activity = adapter.activity(REPLY)
    assert activity is not None and activity.public_output == "TRAE reply"
    assert adapter.activity({"type": []}) is None
    assert adapter.activity({"type": "item.completed", "item": {"type": []}}) is None
    captured = b"".join(capture(_stream(e)) for e in (START, tool, REPLY, DONE))
    assert adapter.parse(0, captured, b"").output == "TRAE reply"


@pytest.mark.asyncio
async def test_trae_fake_process_start_resume_and_cancel_own_descendants(tmp_path: Path) -> None:
    # This script replaces only the executable; the real adapter builds args, captures and
    # parses stdout, and the normal supervisor owns launch, cancellation and descendants.
    script = tmp_path / "fake_trae.py"
    script.write_text(
        "import json,pathlib,subprocess,sys,time\n"
        "prompt=sys.stdin.read()\n"
        "args=sys.argv[1:]\n"
        "assert args[0] == 'exec' and '--no-daemon' not in args\n"
        "assert '--json' in args and '--skip-git-repo-check' in args\n"
        "sid=args[-2] if 'resume' in args else 'native-trae-session'\n"
        "def emit(e): print(json.dumps(e),flush=True)\n"
        "if 'auth-failure' in prompt:\n"
        " emit({'type':'turn.failed','error':{'message':'Not logged in'}}); sys.exit(1)\n"
        "emit({'type':'thread.started','thread_id':sid})\n"
        "if 'pause' in prompt:\n"
        " subprocess.Popen([sys.executable,'-c',\"import pathlib,time; time.sleep(1); pathlib.Path('escaped').write_text('bad')\"])\n"
        " emit({'type':'item.completed','item':{'type':'agent_message','text':'partial reply'}})\n"
        " time.sleep(30)\n"
        "emit({'type':'item.completed','item':{'type':'agent_message','text':json.dumps({'args':args,'prompt':prompt})}})\n"
        "emit({'type':'turn.completed','usage':{'output_tokens':1}})\n"
    )
    manager = InvocationManager()

    def start(prompt: str, session_id: str | None = None) -> str:
        request = InvocationRequest("trae", prompt, tmp_path, session_id, {"model": "native-model"})
        prepared = manager.prepare(request)
        return manager.start(
            request,
            prepared=replace(prepared, argv=(sys.executable, str(script), *prepared.argv[1:])),
        )

    try:
        result = await manager.wait(start("hello"), 10)
        assert result is not None and result.status == InvocationStatus.SUCCEEDED
        assert result.session_id == "native-trae-session" and result.reported_model is None
        body = json.loads(result.output)
        assert "hello" in body["prompt"] and "--model" in body["args"]
        result = await manager.wait(start("continue", result.session_id), 10)
        assert result is not None and result.status == InvocationStatus.SUCCEEDED
        assert result.session_id == "native-trae-session"
        assert "resume" in json.loads(result.output)["args"]
        result = await manager.wait(start("auth-failure", result.session_id), 10)
        assert result is not None and result.status == InvocationStatus.BLOCKED
        assert result.session_id == "native-trae-session" and result.output == ""
        invocation = start("pause")
        async with asyncio.timeout(10):
            while manager.recent_output(invocation)[0] != "partial reply":
                await asyncio.sleep(0.01)
        result = await manager.cancel(invocation)
        assert result.status == InvocationStatus.CANCELLED
        assert (result.output, result.session_id) == ("partial reply", "native-trae-session")
        await asyncio.sleep(1.2)
        assert not (tmp_path / "escaped").exists()
    finally:
        await manager.shutdown()

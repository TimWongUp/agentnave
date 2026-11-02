from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import cast

import pytest
from mcp import Client, StdioServerParameters
from mcp_types import CallToolResult, TextContent

from agentnave import __version__, mcp_server
from agentnave.mcp_server import mcp
from agentnave.processes import spawn_process, terminate_process_tree

EXPECTED_TOOLS = [
    "start_agent",
    "wait",
    "cancel",
    "list",
    "list_members",
    "run_round",
    "decide_answers",
    "handover",
    "read_conversation",
]


@pytest.fixture(autouse=True)
def host_context(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave.host import Host

    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setattr(mcp_server, "HOST", Host())
    monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset[str]())


def _server_args(module: str) -> list[str]:
    return [
        "-c",
        "import os; os.environ.setdefault('AGENTNAVE_HOST', 'hermes'); "
        f"import {module} as server; "
        "server.main()",
    ]


def _install_fake_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    script = tmp_path / ("claude.py" if os.name == "nt" else "claude")
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys, time\n"
        "prompt = sys.stdin.read()\n"
        "assert prompt.startswith('You are a subagent dispatched by AgentNave.')\n"
        "assert 'Do not call AgentNave tools' in prompt\n"
        "prompt = prompt.split('\\n\\nAssigned task:\\n', 1)[1]\n"
        "if prompt == 'finish':\n"
        "    for _ in range(8):\n"
        "        print(json.dumps({'type':'stream_event','event':{'delta':{'type':'text_delta','text':'working '}}}), flush=True)\n"
        "        time.sleep(0.05)\n"
        "if prompt == 'blocker' or prompt.endswith('\\nblocker'):\n"
        "    for text in ['旧' * 1100, '新进展']:\n"
        "        print(json.dumps({'type':'stream_event','event':{'delta':{'type':'text_delta','text':text}}}), flush=True)\n"
        "    print(json.dumps({'type':'assistant','message':{'content':[{'type':'tool_use','id':'t1','name':'Bash','input':{'secret':'DO_NOT_RETURN'}}]}}), flush=True)\n"
        "    for _ in range(5):\n"
        "        print(json.dumps({'type':'system','subtype':'api_retry','error':'authentication_failed'}), flush=True)\n"
        "        time.sleep(0.05)\n"
        "    time.sleep(30)\n"
        "if prompt.endswith('take a nap'):\n"
        "    print(json.dumps({'type':'stream_event','event':{'delta':{'type':'text_delta','text':'still working'}}}), flush=True)\n"
        "    time.sleep(30)\n"
        "if prompt == 'sleep':\n"
        "    time.sleep(30)\n"
        "if prompt == 'malformed usage':\n"
        '    print(\'{"type":"result","result":"MALFORMED USAGE",\' '
        '+ \'"session_id":"session-e2e","num_turns":{"invalid":true},\' '
        "+ '\"total_cost_usd\":' + '1' * 5001 + ',\"is_error\":false}')\n"
        "else:\n"
        "    print(json.dumps({'type': 'result', 'result': prompt.upper(), "
        "'session_id': 'session-e2e', 'num_turns': 1, 'is_error': False}))\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    if os.name == "nt":
        (tmp_path / "claude.cmd").write_text(
            f'@"{sys.executable}" "%~dp0claude.py" %*\n', encoding="utf-8"
        )
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")


def _payload(value: object) -> dict[str, object]:
    return cast(dict[str, object], value)


def _first(result: object, key: str = "updates") -> dict[str, object]:
    """The first task update (or round) of a wait result."""
    content = _payload(cast(CallToolResult, result).structured_content)
    return cast(list[dict[str, object]], content[key])[0]


async def _start_and_wait(client: Client, **arguments: object) -> dict[str, object]:
    result = await client.call_tool("start_agent", {"prompt": "ok", **arguments})
    assert not result.is_error, result.content
    payload = _payload(result.structured_content)
    finished = _first(await client.call_tool("wait", {"ids": [payload["invocation_id"]]}))
    assert finished["status"] == "succeeded", finished
    return payload


@pytest.mark.asyncio
async def test_resume_does_not_inherit_a_declared_access(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    root = tmp_path / "data"
    root.mkdir()
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(root))
    from agentnave.mcp_server import mcp

    async with Client(mcp) as client:
        await _start_and_wait(
            client, provider="claude", cwd=str(tmp_path), role="出计划", access="read"
        )
        resumed = await _start_and_wait(client, session_id="session-e2e")
        assert resumed["access"] is None


@pytest.mark.asyncio
async def test_start_agent_resume_inherits_persisted_latest_task_and_explicit_values_win(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.db import Database
    from agentnave.store import Record

    _install_fake_claude(tmp_path, monkeypatch)
    root = tmp_path / "data"
    (root).mkdir()
    (root / "orchestration.json").write_text(
        json.dumps(
            {
                "version": 1,
                "preset": "split",
                "worker": {"easy": {"provider": "claude", "model": "chosen", "effort": "high"}},
            }
        )
    )
    async with Client(mcp) as client:
        started = await _start_and_wait(
            client, cwd=str(tmp_path), seat="worker-easy", role="实现参数推断"
        )
    # A fresh service reads the seat from disk rather than retaining start arguments.
    async with Client(mcp) as client:
        inherited = await _start_and_wait(client, session_id="session-e2e")
        assert inherited["conversation_id"] == started["conversation_id"]
        assert inherited["provider"] == "claude"
        assert inherited["model"] == "chosen" and inherited["access"] == "write"
        assert set(cast(list[str], inherited["defaults_applied"])) >= {
            *(f"{key} (session_id history)" for key in ("provider", "cwd", "role", "seat")),
            "access (worker-easy seat)",
        }
        alternate = tmp_path / "alternate"
        alternate.mkdir()
        explicit = await _start_and_wait(
            client,
            session_id="session-e2e",
            provider="claude",
            cwd=str(alternate),
            role="只读复核",
            seat="explorer-hard",
            access="read",
        )
        assert not any(
            "session_id history" in key for key in cast(list[str], explicit["defaults_applied"])
        )
        latest = await _start_and_wait(client, session_id="session-e2e")
        assert latest["access"] == "read"
        renamed = await _start_and_wait(client, session_id="session-e2e", role="实现参数推断")
        assert renamed["access"] is None
        assert "seat (session_id history)" not in cast(list[str], renamed["defaults_applied"])
        await _start_and_wait(
            client, session_id="session-e2e", role="只读复核", seat="explorer-hard"
        )
        kept = await _start_and_wait(client, session_id="session-e2e", role=" 只读复核 ")
        assert kept["access"] == "read"
        saved = Record.read(Database(root), root / "locks", str(started["conversation_id"]))
        assert saved.state.tasks[1].cwd == str(tmp_path)
        assert saved.state.tasks[-1].cwd == str(alternate)
        # Inherited seat metadata does not accidentally inherit the previous model or effort.
        assert latest.get("model") != "chosen"
        changed_provider = await client.call_tool(
            "start_agent",
            {"prompt": "ok", "session_id": "session-e2e", "provider": "grok", "cwd": str(tmp_path)},
        )
        assert changed_provider.is_error and "cannot change provider" in str(
            changed_provider.content
        )
        cleared = await _start_and_wait(client, session_id="session-e2e", role="")
        assert "role (session_id history)" not in cast(list[str], cleared["defaults_applied"])
        assert cleared["access"] == "read"
    record = Record.read(Database(root), root / "locks", str(started["conversation_id"]))
    assert record.state.tasks[-2].role == "只读复核"
    assert record.state.tasks[-2].seat == "explorer-hard"
    assert record.state.tasks[-1].role is None
    assert record.state.tasks[-1].prompt == "ok"


@pytest.mark.asyncio
async def test_start_agent_resume_unknown_ambiguous_and_foreign_history(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.db import Database
    from agentnave.host import boss_key
    from agentnave.store import Conversation, Record, TaskRecord

    _install_fake_claude(tmp_path, monkeypatch)
    root = tmp_path / "data"
    async with Client(mcp) as client:
        for arguments in ({}, {"cwd": str(tmp_path)}, {"provider": "claude"}):
            result = await client.call_tool(
                "start_agent", {"prompt": "ok", "session_id": "unknown-native-session", **arguments}
            )
            assert result.is_error and "pass provider and cwd" in str(result.content)
        # Native sessions created outside this service remain resumable with explicit identity.
        await _start_and_wait(
            client, provider="claude", cwd=str(tmp_path), session_id="external-session"
        )
        for provider, owner in (
            ("grok", "test-chat"),
            ("claude", "test-chat"),
            ("claude", "other-boss"),
        ):
            record = Record.create(
                Database(root),
                root / "locks",
                Conversation(
                    id=f"saved-{provider}-{owner}",
                    title="PRIVATE_TITLE",
                    kind="task",
                    manager_session=boss_key(None, owner),
                    tasks=[
                        TaskRecord(
                            id=f"saved-{provider}-{owner}",
                            provider=provider,
                            model="PRIVATE_MODEL",
                            cwd="/PRIVATE_CWD",
                            prompt="PRIVATE_PROMPT",
                            role="PRIVATE_ROLE",
                            session_id="session-e2e",
                            status="succeeded",
                            time="2099-01-01T00:00:00+00:00",
                        )
                    ],
                ),
            )
            record.release()
        ambiguous = await client.call_tool(
            "start_agent", {"prompt": "ok", "session_id": "session-e2e"}
        )
        assert ambiguous.is_error and "multiple providers" in str(ambiguous.content)
        # Explicit provider disambiguates without exposing another boss's saved arguments.
        selected = await _start_and_wait(
            client, provider="claude", cwd=str(tmp_path), session_id="session-e2e"
        )
        assert "PRIVATE_" not in str(selected)
        for arguments in ({}, {"provider": "claude"}):
            foreign = await client.call_tool(
                "start_agent",
                {
                    "prompt": "ok",
                    "session_id": "session-e2e",
                    "manager_session_id": "third-boss",
                    **arguments,
                },
            )
            assert foreign.is_error and "different manager session" in str(foreign.content)
            assert "PRIVATE_" not in str(foreign.content)
        foreign_explicit = await _start_and_wait(
            client,
            session_id="session-e2e",
            manager_session_id="third-boss",
            provider="claude",
            cwd=str(tmp_path),
        )
        assert foreign_explicit["access"] is None
        assert "PRIVATE_" not in str(foreign_explicit)
        assert not any(
            "session_id history" in key
            for key in cast(list[str], foreign_explicit["defaults_applied"])
        )


@pytest.mark.asyncio
async def test_start_agent_provider_inference_reports_unusable_seats_and_requires_cwd(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.host import Host

    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "orchestration.json"
    seat = {"provider": "claude", "model": "chosen"}
    async with Client(mcp) as client:
        for arguments, expected in (
            ({}, "Pass provider"),
            ({"role": "ordinary duty"}, "missing"),
            ({"role": "correctness-reviewer"}, "missing"),
            ({"seat": "worker-easy"}, "missing"),
        ):
            result = await client.call_tool(
                "start_agent", {"prompt": "ok", "cwd": str(tmp_path), **arguments}
            )
            assert result.is_error and expected in str(result.content)
        for content, expected in (
            ("broken", "Invalid orchestration.json"),
            (json.dumps({"version": 1, "preset": "off", "worker": {"easy": seat}}), "not enabled"),
            (json.dumps({"version": 1, "preset": "split"}), "missing"),
        ):
            path.write_text(content)
            result = await client.call_tool(
                "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "worker-easy"}
            )
            assert result.is_error and expected in str(result.content)
        path.write_text(json.dumps({"version": 1, "preset": "split", "worker": {"easy": seat}}))
        missing_cwd = await client.call_tool("start_agent", {"prompt": "ok", "seat": "worker-easy"})
        assert missing_cwd.is_error and "cwd is required" in str(missing_cwd.content)
        monkeypatch.setattr(mcp_server, "HOST", Host("claude", "env"))
        native = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "worker-easy"}
        )
        assert native.is_error and "native" in str(native.content)
        assert "built-in subagent" in str(native.content)


@pytest.mark.asyncio
async def test_mcp_blocker_interrupts_default_wait_with_bounded_public_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent", {"provider": "claude", "prompt": "blocker", "cwd": str(tmp_path)}
        )
        invocation_id = _payload(started.structured_content)["invocation_id"]
        # A later dispatch lists the blocker without taking it from wait.
        blocked = {"id": invocation_id, "kind": "task", "status": "execution_blocked"}
        async with asyncio.timeout(10):
            while True:
                other_result = await client.call_tool(
                    "start_agent",
                    {"provider": "claude", "prompt": "hi", "cwd": str(tmp_path)},
                )
                assert not other_result.is_error, other_result.content
                other = _payload(other_result.structured_content)
                listed = cast(list[dict[str, object]], other.get("unreceived", []))
                if any(blocked.items() <= item.items() for item in listed):
                    break
                await client.call_tool("wait", {"ids": [other["invocation_id"]]})
        async with asyncio.timeout(3):
            response = await client.call_tool("wait", {"ids": [invocation_id]})
        assert _payload(response.structured_content)["reason"] == "ready"
        assert _payload(response.structured_content)["pending_ids"] == [invocation_id]
        assert _payload(response.structured_content)["all_finished"] is False
        payload = _first(response)
        assert payload["status"] == "running"
        assert payload["reason"] == "execution_blocked"
        assert _payload(payload["error"])["code"] == "authentication_failed"
        assert "output" not in payload and "output_age_ms" not in payload
        assert "kind" in _payload(payload["activity"])
        assert "DO_NOT_RETURN" not in str(payload)
        assert "provider_usage" not in payload
        peek = _first(await client.call_tool("wait", {"ids": [invocation_id], "wait_seconds": 0}))
        assert peek["output"] == "旧" * 997 + "新进展"
        assert "DO_NOT_RETURN" not in str(peek)
        hidden = _first(
            await client.call_tool(
                "wait", {"ids": [invocation_id], "wait_seconds": 0, "include_output": False}
            )
        )
        assert "output" not in hidden and "output_age_ms" not in hidden
        monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.4)
        later = _first(await client.call_tool("wait", {"ids": [invocation_id]}))
        assert later["reason"] == "wait_elapsed"
        assert "output" not in later and "output_age_ms" not in later
        later_peek = _first(
            await client.call_tool("wait", {"ids": [invocation_id], "wait_seconds": 0})
        )
        assert later_peek["output"] == peek["output"]
        assert cast(int, later_peek["output_age_ms"]) > cast(int, peek["output_age_ms"])
        cancelled = await client.call_tool("cancel", {"id": invocation_id})
        assert _payload(_payload(cancelled.structured_content)["invocation"])["status"] == (
            "cancelled"
        )


@pytest.mark.asyncio
async def test_mcp_lists_lifecycle_and_discovery_tools_with_structured_contracts() -> None:
    async with Client(mcp) as client:
        result = await client.list_tools()

    assert [tool.name for tool in result.tools] == EXPECTED_TOOLS
    assert all(tool.output_schema is not None for tool in result.tools)
    tools = {tool.name: tool for tool in result.tools}
    start = tools["start_agent"].annotations
    assert start is not None and start.open_world_hint is True and start.destructive_hint is True
    start_properties = _payload(tools["start_agent"].input_schema["properties"])
    assert "timeout_seconds" not in start_properties
    assert tools["start_agent"].input_schema["required"] == ["prompt"]
    wait_properties = _payload(tools["wait"].input_schema["properties"])
    assert set(wait_properties) == {"ids", "include_output", "wait_seconds", "manager_session_id"}
    assert mcp_server.WAIT_SECONDS == 240
    wait_modes = cast(list[dict[str, object]], _payload(wait_properties["wait_seconds"])["anyOf"])
    assert wait_modes[0]["enum"] == [0, 240]
    cancel = tools["cancel"].annotations
    assert cancel is not None and cancel.destructive_hint is True
    for name in ("wait", "list", "list_members", "read_conversation"):
        annotations = tools[name].annotations
        assert annotations is not None and annotations.read_only_hint is True


@pytest.mark.asyncio
async def test_provider_details_are_disclosed_only_on_request(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("PATH", "")
    async with Client(mcp) as client:
        before = await client.list_tools()
        catalog = str(before) + str(client.instructions)
        assert "permission_mode" not in catalog
        assert "list_members" in catalog
        described = await client.call_tool("list_members", {"provider": "codebuddy"})
        assert described.is_error is False
        payload = _payload(_payload(described.structured_content)["provider_details"])
        assert payload["provider"] == "codebuddy"
        assert set(payload) == {"provider", "permitted", "show_permitted", "supported_options"}
        assert "permission_mode" in str(payload["supported_options"])
        assert await client.list_tools() == before
        invalid = await client.call_tool("list_members", {"provider": "unknown"})
        assert invalid.is_error is True


@pytest.mark.asyncio
async def test_mcp_starts_and_waits_for_provider_with_structured_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.05)

    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "finish", "cwd": str(tmp_path)},
        )
        started_payload = _payload(started.structured_content)
        running_payloads: list[dict[str, object]] = []
        async with asyncio.timeout(3):
            while True:
                finished = await client.call_tool(
                    "wait", {"ids": [started_payload["invocation_id"]]}
                )
                payload = _first(finished)
                if payload["status"] != "running":
                    break
                running_payloads.append(payload)

    assert len(running_payloads) >= 2
    assert all(payload["reason"] == "wait_elapsed" for payload in running_payloads)
    assert all(
        "output" not in payload and "output_age_ms" not in payload for payload in running_payloads
    )
    finished_payload = _first(finished)
    invocation_result = finished_payload
    assert started.is_error is False
    assert started_payload["status"] == "running"
    assert started_payload["provider"] == "claude"
    assert "model" not in started_payload
    assert started_payload["effort"] == "继承原生设置"
    assert "prompt" not in started_payload
    assert finished.is_error is False
    assert finished_payload["reason"] == "finished"
    assert invocation_result["status"] == "succeeded"
    assert invocation_result["output"] == "FINISH"
    assert invocation_result["session_id"] == "session-e2e"


@pytest.mark.asyncio
async def test_mcp_group_round_runs_seats_in_parallel_and_returns_final_answers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    async with Client(mcp) as client:
        started = await client.call_tool(
            "run_round",
            {
                "title": "Parallel",
                "cwd": str(tmp_path),
                "members": [{"id": s, "provider": "claude"} for s in "ab"],
                "seats": ["a", "b"],
                "order": "parallel",
                "prompt": "compare notes",
                "instructions": {"b": "argue the other side"},
            },
        )
        group_id = _payload(started.structured_content)["group_id"]
        started_payload = _payload(_payload(started.structured_content)["round"])
        assert started_payload["round"] == 1
        assert started_payload["reason"] == "started"
        seats = cast(list[dict[str, object]], started_payload["seats"])
        assert [seat["status"] for seat in seats] == ["running", "running"]
        async with asyncio.timeout(10):
            while True:
                waited = await client.call_tool(
                    "wait", {"ids": [started_payload["round_id"]], "wait_seconds": 240}
                )
                payload = _first(waited, "rounds")
                if payload["status"] != "running":
                    break
        rejected = await client.call_tool(
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "x"},
        )
        status_only = await client.call_tool(
            "wait", {"ids": [started_payload["round_id"]], "include_output": False}
        )
        compact = _first(status_only, "rounds")
        assert all("output" not in seat for seat in cast(list[dict[str, object]], compact["seats"]))
        reread = await client.call_tool("wait", {"ids": [started_payload["round_id"]]})
        seats = cast(list[dict[str, object]], _first(reread, "rounds")["seats"])
        assert all("output" not in seat and seat["output_returned"] is True for seat in seats)
        # The text content is the structured result, compact and without empty fields.
        text = reread.content[0]
        assert isinstance(text, TextContent) and "null" not in text.text and "\n" not in text.text

    assert payload["status"] == "finished" and payload["reason"] == "finished"
    outputs = [str(seat["output"]) for seat in cast(list[dict[str, object]], payload["seats"])]
    assert all("COMPARE NOTES" in output for output in outputs)
    assert "ARGUE THE OTHER SIDE" not in outputs[0] and "ARGUE THE OTHER SIDE" in outputs[1]
    assert rejected.is_error is True and "unknown seat ids: c" in str(rejected.content)


@pytest.mark.asyncio
async def test_mcp_round_snapshot_holds_back_finished_answers_until_delivery(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.05)
    async with Client(mcp) as client:
        started = await client.call_tool(
            "run_round",
            {
                "title": "Snapshot",
                "cwd": str(tmp_path),
                "members": [{"id": s, "provider": "claude"} for s in "ab"],
            },
        )
        group_id = _payload(started.structured_content)["group_id"]
        started = await client.call_tool(
            "run_round",
            {
                "group_id": group_id,
                "seats": ["a", "b"],
                "order": "parallel",
                "prompt": "hello",
                "instructions": {"b": "take a nap"},
            },
        )
        round_id = _payload(_payload(started.structured_content)["round"])["round_id"]
        async with asyncio.timeout(10):
            while True:
                waited = await client.call_tool("wait", {"ids": [round_id]})
                snapshot = cast(list[dict[str, object]], _first(waited, "rounds")["seats"])
                if snapshot[0]["status"] == "succeeded" and "activity" in snapshot[1]:
                    break
        assert "output" not in snapshot[0] and "output_returned" not in snapshot[0]
        assert "output" not in snapshot[1] and "output_age_ms" not in snapshot[1]
        peek = _first(
            await client.call_tool("wait", {"ids": [round_id], "wait_seconds": 0}), "rounds"
        )
        peek_seats = cast(list[dict[str, object]], peek["seats"])
        assert "output" not in peek_seats[0] and "output_returned" not in peek_seats[0]
        assert peek_seats[1]["output"] == "still working"
        hidden = _first(
            await client.call_tool(
                "wait", {"ids": [round_id], "wait_seconds": 0, "include_output": False}
            ),
            "rounds",
        )
        assert all(
            "output" not in seat and "output_age_ms" not in seat
            for seat in cast(list[dict[str, object]], hidden["seats"])
        )
        cancelled = await client.call_tool("cancel", {"id": round_id})
        delivered = _payload(_payload(cancelled.structured_content)["round"])
        assert "HELLO" in str(cast(list[dict[str, object]], delivered["seats"])[0]["output"])


@pytest.mark.asyncio
async def test_mcp_blocked_round_keeps_finished_answers_and_hides_running_tails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    async with Client(mcp) as client:
        started = await client.call_tool(
            "run_round",
            {
                "title": "Mixed states",
                "cwd": str(tmp_path),
                "members": [{"id": s, "provider": "claude"} for s in "ab"],
                "seats": ["a", "b"],
                "order": "serial",
                "prompt": "hello",
                "instructions": {"b": "blocker"},
            },
        )
        round_id = _payload(_payload(started.structured_content)["round"])["round_id"]
        async with asyncio.timeout(5):
            response = await client.call_tool("wait", {"ids": [round_id]})
        assert _payload(response.structured_content)["reason"] == "ready"
        payload = _first(response, "rounds")
        assert payload["status"] == "running"
        finished, blocked = cast(list[dict[str, object]], payload["seats"])
        assert finished["status"] == "succeeded" and "HELLO" in str(finished["output"])
        assert blocked["status"] == "running"
        assert _payload(blocked["error"])["code"] == "authentication_failed"
        assert "activity" in blocked and "output" not in blocked and "output_age_ms" not in blocked
        peek = _first(
            await client.call_tool("wait", {"ids": [round_id], "wait_seconds": 0}), "rounds"
        )
        finished, blocked = cast(list[dict[str, object]], peek["seats"])
        assert finished["output_returned"] is True and "output" not in finished
        assert blocked["output"] == "旧" * 997 + "新进展"
        await client.call_tool("cancel", {"id": round_id})


@pytest.mark.asyncio
async def test_mcp_rejects_arbitrary_wait_windows() -> None:
    async with Client(mcp) as client:
        for seconds in (1, 45, 300):
            result = await client.call_tool("wait", {"ids": ["missing"], "wait_seconds": seconds})
            assert result.is_error is True
            assert "0" in str(result.content) and "240" in str(result.content)
            assert "Unknown or expired id" not in str(result.content)


@pytest.mark.asyncio
async def test_mcp_preserves_terminal_result_when_provider_usage_is_malformed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)

    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "malformed usage", "cwd": str(tmp_path)},
        )
        invocation_id = _payload(started.structured_content)["invocation_id"]
        finished = await client.call_tool("wait", {"ids": [invocation_id]})

    invocation_result = _first(finished)
    assert finished.is_error is False
    assert invocation_result["status"] == "succeeded"
    assert invocation_result["output"] == "MALFORMED USAGE"
    assert invocation_result["session_id"] == "session-e2e"
    assert "provider_usage" not in invocation_result


@pytest.mark.asyncio
async def test_mcp_wait_any_and_list_invocations_expose_handles_without_task_text(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr("agentnave.core._FINISHED_RETENTION_SECONDS", 1.0)
    async with Client(mcp) as client:
        starts: list[dict[str, object]] = []
        for prompt in ("sleep", "private task text"):
            started = await client.call_tool(
                "start_agent",
                {
                    "provider": "claude",
                    "prompt": prompt,
                    "cwd": str(tmp_path),
                    "manager_model": "test-manager",
                },
            )
            starts.append(_payload(started.structured_content))
        sleeping, finished = (start["invocation_id"] for start in starts)
        # A second run in the same cwd is flagged, never refused.
        assert "warnings" not in starts[0]
        assert str(sleeping) in cast(list[str], starts[1]["warnings"])[0]
        mistaken = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "x", "cwd": str(tmp_path), "session_id": sleeping},
        )
        assert mistaken.is_error is True and "not a native session" in str(mistaken.content)
        async with asyncio.timeout(3):
            for invalid in ([], [finished, finished], [sleeping, "missing"]):
                rejected = await client.call_tool("wait", {"ids": invalid})
                assert rejected.is_error is True
            ready = await client.call_tool("wait", {"ids": [sleeping, finished]})
        ready_payload = _payload(ready.structured_content)
        assert ready_payload["reason"] == "ready"
        (update,) = cast(list[dict[str, object]], ready_payload["updates"])
        assert update["invocation_id"] == finished
        assert (update["status"], update["reason"]) == ("succeeded", "finished")

        listed = _payload((await client.call_tool("list", {})).structured_content)
        active, done = cast(list[dict[str, object]], listed["invocations"])
        assert active["invocation_id"] == sleeping
        assert (active["status"], active["phase"], active["events_seen"]) == (
            "running",
            "running",
            False,
        )
        assert "session_id" not in active
        assert done == {
            "invocation_id": finished,
            "provider": "claude",
            "cwd": str(tmp_path),
            "status": "succeeded",
            "elapsed_ms": update["elapsed_ms"],
            "events_seen": True,
            "session_id": "session-e2e",
            "result_read": True,
        }
        assert "private task text" not in str(listed).lower()

        async with asyncio.timeout(1):
            peek = await client.call_tool("wait", {"ids": [sleeping], "wait_seconds": 0})
        assert _payload(peek.structured_content)["reason"] == "wait_elapsed"
        monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.2)
        elapsed = _payload((await client.call_tool("wait", {"ids": [sleeping]})).structured_content)
        assert elapsed["reason"] == "wait_elapsed"
        (running,) = cast(list[dict[str, object]], elapsed["updates"])
        assert (running["invocation_id"], running["status"]) == (sleeping, "running")
        await asyncio.sleep(1.0)
        # The relay's unlisted probe reports the same handles and whether a restart is safe.
        listed = _payload((await client.call_tool("list_invocations", {})).structured_content)
        assert [
            item["invocation_id"] for item in cast(list[dict[str, object]], listed["invocations"])
        ] == [sleeping]
        assert listed["restart_safe"] is False  # the sleeping task still holds its history
        await client.call_tool("cancel", {"id": sleeping})


@pytest.mark.asyncio
async def test_mcp_running_result_can_be_cancelled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.01)

    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "sleep", "cwd": str(tmp_path)},
        )
        invocation_id = _payload(started.structured_content)["invocation_id"]
        running = await client.call_tool("wait", {"ids": [invocation_id]})
        cancelled = await client.call_tool("cancel", {"id": invocation_id})

    running_payload = _first(running)
    cancelled_payload = _payload(_payload(cancelled.structured_content)["invocation"])
    cancelled_result = cancelled_payload
    assert running_payload["status"] == "running"
    assert isinstance(running_payload["elapsed_ms"], int)
    assert running_payload["reason"] == "wait_elapsed"
    assert "snapshot" not in running_payload
    assert cancelled_payload["reason"] == "finished"
    assert cancelled_result["status"] == "cancelled"


@pytest.mark.asyncio
@pytest.mark.parametrize("title", ["汉" * 25, "🚀" * 25, "a" * 49])
async def test_mcp_rejects_titles_wider_than_one_short_line(tmp_path: Path, title: str) -> None:
    async with Client(mcp) as client:
        task = await client.call_tool(
            "start_agent",
            {"provider": "codex", "prompt": "x", "cwd": str(tmp_path), "title": title},
        )
        show = await client.call_tool("run_round", {"prompt": "x", "title": title})

    for result in (task, show):
        assert result.is_error is True
        assert "within 48" in str(result.content)


@pytest.mark.asyncio
async def test_mcp_schema_rejects_invalid_provider_with_actionable_error(tmp_path: Path) -> None:
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agent",
            {"provider": "unknown", "prompt": "finish", "cwd": str(tmp_path)},
        )

    assert result.is_error is True
    assert result.content
    assert isinstance(result.content[0], TextContent)
    assert "antigravity" in result.content[0].text
    assert "claude" in result.content[0].text
    assert "codebuddy" in result.content[0].text
    assert "codex" in result.content[0].text
    assert "grok" in result.content[0].text
    assert "pi" in result.content[0].text


@pytest.mark.asyncio
@pytest.mark.parametrize("cwd", ["relative", "~", "~missing-user", "option"])
async def test_mcp_rejects_non_absolute_cwd_with_retry_guidance(cwd: str) -> None:
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "finish", "cwd": cwd},
        )

    assert result.is_error is True
    assert result.content
    assert isinstance(result.content[0], TextContent)
    assert "absolute" in result.content[0].text
    assert "retry" in result.content[0].text
    assert "Supported " not in result.content[0].text


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("provider", "options", "message"),
    [
        ("grok", {"unsupported_probe": True}, "unsupported grok options"),
        ("codex", {"skip_git_repo_check": "true"}, "must be a boolean"),
        ("antigravity", {"sandbox": "true"}, "must be boolean"),
    ],
)
async def test_mcp_rejects_invalid_options_before_starting(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    provider: str,
    options: dict[str, str | bool],
    message: str,
) -> None:
    def unexpected_temp_file(*_args: object, **_kwargs: object) -> tuple[int, str]:
        pytest.fail("invalid options must not create a prompt file")

    monkeypatch.setattr("agentnave.adapters.grok.tempfile.mkstemp", unexpected_temp_file)
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agent",
            {
                "provider": provider,
                "prompt": "finish",
                "cwd": str(tmp_path),
                "provider_options": options,
            },
        )

    assert result.is_error is True
    assert message in str(result.content)
    assert f"Supported {provider} options: model, effort," in str(result.content)
    assert "Correct the arguments and retry" in str(result.content)
    assert "invocation_id" not in (result.structured_content or {})


@pytest.mark.asyncio
async def test_mcp_rejects_arguments_outside_the_schema(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("AGENTNAVE_HOST_SESSION_ID")
    async with Client(mcp) as client:
        tools = {tool.name: tool for tool in (await client.list_tools()).tools}
        result = await client.call_tool(
            "start_agent",
            {
                "provider": "pi",
                "prompt": "finish",
                "cwd": str(tmp_path),
                "model": "m",
                "effort": "max",
            },
        )
        listed = _payload((await client.call_tool("list_invocations", {})).structured_content)

    assert tools["start_agent"].input_schema["additionalProperties"] is False
    assert result.is_error is True
    assert "Unknown argument(s) for start_agent: effort, model" in str(result.content)
    assert "go inside provider_options" in str(result.content)
    # Both mistakes come back in one reply, so a single retry fixes them.
    assert "needs manager_session_id" in str(result.content)
    assert listed["invocations"] == []


@pytest.mark.asyncio
async def test_mcp_reports_preparation_io_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def denied_temp_file(*_args: object, **_kwargs: object) -> tuple[int, str]:
        raise PermissionError("temporary directory is not writable")

    monkeypatch.setattr("agentnave.adapters.grok.tempfile.mkstemp", denied_temp_file)
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agent", {"provider": "grok", "prompt": "finish", "cwd": str(tmp_path)}
        )

    assert result.is_error is True
    assert "Unable to prepare or save the task" in str(result.content)
    assert "Invalid invocation request" not in str(result.content)


@pytest.mark.asyncio
async def test_mcp_expired_invocation_returns_tool_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr("agentnave.core._FINISHED_RETENTION_SECONDS", 0.5)
    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent", {"provider": "claude", "prompt": "complete", "cwd": str(tmp_path)}
        )
        invocation_id = _payload(started.structured_content)["invocation_id"]
        finished = await client.call_tool("wait", {"ids": [invocation_id]})
        assert _first(finished)["status"] == "succeeded"
        await asyncio.sleep(0.6)
        for tool, arguments in (
            ("wait", {"ids": [invocation_id]}),
            ("cancel", {"id": invocation_id}),
        ):
            expired = await client.call_tool(tool, arguments)
            assert expired.is_error
            assert "expired" in str(expired.content)
            assert "seven days" in str(expired.content)


@pytest.mark.asyncio
async def test_mcp_unknown_invocation_error_tells_agent_how_to_recover() -> None:
    async with Client(mcp) as client:
        result = await client.call_tool("wait", {"ids": ["missing"]})

    assert result.is_error is True
    assert result.content
    assert isinstance(result.content[0], TextContent)
    assert "start_agent" in result.content[0].text
    assert "MCP server session" in result.content[0].text


@pytest.mark.asyncio
async def test_mcp_provider_launch_failure_is_structured_and_hides_traceback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("PATH", str(tmp_path))

    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "finish", "cwd": str(tmp_path)},
        )
        invocation_id = _payload(started.structured_content)["invocation_id"]
        finished = await client.call_tool("wait", {"ids": [invocation_id]})

    invocation_result = _first(finished)
    error = _payload(invocation_result["error"])
    assert finished.is_error is False
    assert invocation_result["status"] == "failed"
    assert error["code"] == "launch_error"
    assert "Traceback" not in str(finished.content)
    assert str(tmp_path) not in str(finished.content)


@pytest.mark.asyncio
async def test_stdio_entrypoint_exposes_mcp_tools(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    parameters = StdioServerParameters(
        command=sys.executable,
        args=_server_args("agentnave.mcp_server"),
        env={"HOME": str(home), "USERPROFILE": str(home)},
        cwd=tmp_path,
    )
    async with Client(parameters) as client:
        assert client.server_info is not None
        assert client.server_info.name == "AgentNave"
        assert client.server_info.version == __version__
        result = await client.list_tools()
    # Discovery alone does not initialize history or write to the working directory.
    assert sorted(tmp_path.iterdir()) == [home] and not any(home.iterdir())

    assert [tool.name for tool in result.tools] == EXPECTED_TOOLS


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "excluded",
    [" CODEX, codex, ", "antigravity,claude,codebuddy,codex,deepseek,grok,pi,zcode"],
)
async def test_stdio_enforces_host_exclusions_before_launch(
    excluded: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    marker = tmp_path / "codex-started"
    fake_codex = tmp_path / "codex"
    fake_codex.write_text(f"#!/bin/sh\ntouch '{marker}'\n")
    fake_codex.chmod(0o755)
    parameters = StdioServerParameters(
        command=sys.executable,
        args=_server_args("agentnave.mcp_server"),
        env={
            "PATH": os.environ["PATH"],
            "AGENTNAVE_EXCLUDED_PROVIDERS": excluded,
            "AGENTNAVE_DATA_DIR": str(tmp_path / "data"),
            "AGENTNAVE_HOST_SESSION_ID": "test-chat",
        },
    )
    async with Client(parameters) as client:
        tools = (await client.list_tools()).tools
        description = str(tools[0].input_schema)
        assert client.instructions is not None
        assert "list_members" in client.instructions
        assert "Excluded providers:" in client.instructions
        assert "Excluded providers:" in description
        described = await client.call_tool("list_members", {"provider": "codex"})
        details = _payload(_payload(described.structured_content)["provider_details"])
        assert details["permitted"] is False
        rejected = await client.call_tool(
            "start_agent", {"provider": "codex", "prompt": "finish", "cwd": str(tmp_path)}
        )
        assert rejected.is_error is True
        assert "excluded by this host" in str(rejected.content)
        grouped = await client.call_tool(
            "run_round",
            {
                "title": "Excluded",
                "cwd": str(tmp_path),
                "members": [{"id": p, "provider": p} for p in ("codex", "grok")],
            },
        )
        assert grouped.is_error is False  # exclusions limit start_agent, not group seats
        assert not marker.exists()
        if "claude" in excluded:
            assert "Providers permitted by this host configuration: none" in description
        else:
            started = await client.call_tool(
                "start_agent", {"provider": "claude", "prompt": "finish", "cwd": str(tmp_path)}
            )
            assert started.is_error is False
            finished = await client.call_tool(
                "wait", {"ids": [_payload(started.structured_content)["invocation_id"]]}
            )
            assert _first(finished)["output"] == "FINISH"


def test_stdio_rejects_unknown_exclusion_configuration() -> None:
    completed = subprocess.run(
        [sys.executable, "-m", "agentnave.mcp_server"],
        env={**os.environ, "AGENTNAVE_EXCLUDED_PROVIDERS": "codxe"},
        input="",
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert completed.returncode != 0
    assert completed.stdout == ""
    assert "Unknown AGENTNAVE_EXCLUDED_PROVIDERS: codxe" in completed.stderr


@pytest.mark.asyncio
async def test_dispatched_entrypoint_keeps_tools_with_unreadable_host_ancestry(
    tmp_path: Path,
) -> None:
    bootstrap = """
import os
from agentnave import host
os.environ.pop('AGENTNAVE_HOST', None)
def denied(_pid):
    raise PermissionError('ancestor identity denied')
host.read_process_parent = denied
host.read_process_argv = denied
from agentnave.mcp_server import main
main()
"""
    probe = f"""
import asyncio, json, os, sys
from mcp import Client, StdioServerParameters
async def main():
    parameters = StdioServerParameters(
        command=sys.executable,
        args=['-c', {bootstrap!r}],
        env={{'AGENTNAVE_SUPERVISOR': str(os.getppid()),
              'AGENTNAVE_DATA_DIR': {str(tmp_path / "data")!r}}},
    )
    async with Client(parameters) as client:
        print(json.dumps([tool.name for tool in (await client.list_tools()).tools]))
asyncio.run(main())
"""
    supervised = await spawn_process((sys.executable, "-c", probe), tmp_path)
    try:
        async with asyncio.timeout(15):
            completion = await supervised.completion
            assert completion.returncode == 0
            assert supervised.process.stdout is not None
            assert json.loads(await supervised.process.stdout.readline()) == EXPECTED_TOOLS
    finally:
        await terminate_process_tree(supervised.process, grace_seconds=0.01)
    assert not list(tmp_path.iterdir())


@pytest.mark.asyncio
async def test_members_read_missing_and_fresh_files_without_preparing_commands(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json

    from agentnave.adapters.antigravity import AntigravityAdapter
    from agentnave.adapters.grok import GrokAdapter
    from agentnave.host import Host

    root = tmp_path / "data"
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(root))
    monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset({"codex"}))
    monkeypatch.setattr(mcp_server, "HOST", Host(name="claude", source="env"))

    def no_prepare(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("list_members must not prepare CLI commands or discover executables")

    monkeypatch.setattr(GrokAdapter, "prepare", no_prepare)
    monkeypatch.setattr(AntigravityAdapter, "prepare", no_prepare)
    monkeypatch.setattr("agentnave.adapters.codex._codex_executable", no_prepare)
    async with Client(mcp) as client:
        missing = await client.call_tool("list_members", {})
        assert not missing.is_error
        missing_payload = _payload(missing.structured_content)
        assert missing_payload["configured"] is False
        assert missing_payload["modes"] == {
            "task": {"restricted": False, "members": []},
            "discussion": {"restricted": False, "members": []},
        }
        assert not root.exists()
        root.mkdir()
        clis: dict[str, dict[str, dict[str, dict[str, object]]]] = {
            provider: {"models": {"chosen": {"effort": "high", "notes": "核验"}}}
            for provider in ("codex", "claude", "codebuddy", "grok", "antigravity", "pi")
        }
        document: dict[str, object] = {
            "version": 1,
            "clis": clis,
            "modes": {},
            "defaults": {"discussion": {"codex": "chosen"}},
        }
        clis["antigravity"]["models"]["chosen"] = {"options": {"restricted_tools": True}}
        clis["codex"]["models"]["bad-option"] = {"options": {"unknown": True}}
        clis["codex"]["models"]["bad-type"] = {"options": {"skip_git_repo_check": "true"}}
        clis["antigravity"]["models"]["bad-restricted"] = {
            "options": {"restricted_tools": True, "sandbox": True}
        }
        seats = {
            "correctness-reviewer": {"provider": "grok", "model": "chosen", "effort": "high"},
            "security-reviewer": {"provider": "codex", "model": "chosen"},
            "performance-reviewer": {"provider": "grok", "model": "gone"},
            "simplicity-reviewer": {"provider": "claude", "model": "chosen"},
        }
        roles = {
            "version": 1,
            "preset": "split",
            "explorer": {"easy": {"provider": "claude", "model": "chosen"}},
            "worker": {"hard": {"provider": "codex", "model": "chosen", "effort": "high"}},
        }
        (root / "orchestration.json").write_text(json.dumps(roles), encoding="utf-8")
        (root / "review.json").write_text("{", encoding="utf-8")
        content = json.dumps(document, ensure_ascii=False)
        path = root / "members.json"
        path.write_text(content, encoding="utf-8")
        listed = await client.call_tool("list_members", {})
        assert not listed.is_error
        broken = _payload(listed.structured_content)
        assert broken["review"] == {} and "review.json" in cast(str, broken["review_error"])
        (root / "review.json").write_text(json.dumps({"version": 1, "seats": seats}))
        listed = await client.call_tool("list_members", {})
        review = cast(dict[str, dict[str, object]], _payload(listed.structured_content)["review"])
        assert review["correctness-reviewer"]["available"] is True
        assert review["correctness-reviewer"]["effort"] == "high"
        assert review["security-reviewer"]["reason"] == "CLI is excluded by this host"
        # The host's own CLI goes to its built-in subagent rather than being unavailable.
        assert review["simplicity-reviewer"]["available"] is True
        assert review["simplicity-reviewer"]["native"] is True
        orchestration = cast(
            dict[str, dict[str, dict[str, object]]],
            _payload(listed.structured_content)["orchestration"],
        )
        assert orchestration["explorer"]["easy"]["native"] is True
        assert orchestration["worker"]["hard"]["available"] is False
        assert orchestration["worker"]["hard"]["effort"] == "high"
        assert review["security-reviewer"]["alternatives"] == [
            "claude",
            "codebuddy",
            "grok",
            "antigravity",
            "pi",
        ]
        # An unrestricted task mode accepts any model.
        assert review["performance-reviewer"]["available"] is True
        assert review["performance-reviewer"]["alternatives"] == []
        modes = cast(dict[str, dict[str, object]], _payload(listed.structured_content)["modes"])
        items = cast(list[dict[str, object]], modes["task"]["members"])
        valid = [item for item in items if item["model"] == "chosen"]
        assert len(valid) == 6
        assert all(item["valid"] is True and item["error"] is None for item in valid)
        assert valid[0]["permitted"] is False and valid[1]["permitted"] is True
        invalid = [item for item in items if str(item["model"]).startswith("bad-")]
        assert len(invalid) == 3 and all(
            item["valid"] is False and item["error"] for item in invalid
        )
        discussion = cast(list[dict[str, object]], modes["discussion"]["members"])
        assert all(item["permitted"] for item in discussion)
        assert [(item["provider"], item["model"]) for item in discussion if item["default"]] == [
            ("codex", "chosen")
        ]
        assert path.read_text(encoding="utf-8") == content
        assert sorted(root.iterdir()) == [path, root / "orchestration.json", root / "review.json"]
        path.write_text('{"version": 1, "clis": {}, "modes": {"discussion": {}}}', encoding="utf-8")
        fresh = _payload((await client.call_tool("list_members", {})).structured_content)
        assert fresh["configured"] is True
        assert fresh["modes"] == {
            "task": {"restricted": False, "members": []},
            "discussion": {"restricted": True, "members": []},
        }


@pytest.mark.asyncio
async def test_members_report_corrupt_files_without_overwriting_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "members.json"
    async with Client(mcp) as client:
        for content in (
            "{",
            '{"version": 2, "clis": {}, "modes": {}}',
            '{"version": true, "clis": {}, "modes": {}}',
            '{"version": 1, "clis": {"claude": {"models": {"chosen": {"options": {"model": "bad"}}}}}, "modes": {}}',
            '{"version": 1, "clis": {}, "modes": {"task": {"unknown": {"models": []}}}}',
        ):
            path.write_text(content, encoding="utf-8")
            result = await client.call_tool("list_members", {})
            assert result.is_error and "members.json" in str(result.content)
            assert "AgentNave App" in str(result.content)
            for tool, arguments in (
                ("start_agent", {"provider": "claude", "prompt": "x", "cwd": str(tmp_path)}),
                (
                    "run_round",
                    {
                        "title": "Broken",
                        "cwd": str(tmp_path),
                        "members": [
                            {"id": "a", "provider": "claude"},
                            {"id": "b", "provider": "claude"},
                        ],
                    },
                ),
            ):
                rejected = await client.call_tool(tool, arguments)
                assert rejected.is_error and "AgentNave App" in str(rejected.content)
                assert "Invalid invocation request" not in str(rejected.content)
                assert "Correct the arguments" not in str(rejected.content)
                assert "Supported " not in str(rejected.content)
            assert path.read_text(encoding="utf-8") == content
        path.write_bytes(b"\xff")
        result = await client.call_tool("list_members", {})
        assert result.is_error and "members.json" in str(result.content)


@pytest.mark.asyncio
async def test_members_apply_defaults_and_enforce_each_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json

    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "members.json"
    document: dict[str, object] = {
        "version": 1,
        "clis": {
            "claude": {
                "models": {
                    "chosen": {
                        "efforts": ["low", "medium", "high"],
                        "effort": "high",
                        "options": {"permission_mode": "default"},
                        "notes": "共享",
                    }
                }
            }
        },
        "modes": {
            "task": {
                "claude": {
                    "models": ["chosen"],
                    "default": "chosen",
                    "overrides": {
                        "chosen": {
                            "effort": "medium",
                            "efforts": ["medium", "high"],
                            "options": {"agent": "reviewer"},
                            "notes": "任务",
                        }
                    },
                }
            },
            "discussion": {"claude": {"models": [], "default": "chosen"}},
        },
    }
    path.write_text(json.dumps(document), encoding="utf-8")
    async with Client(mcp) as client:

        async def start(
            options: dict[str, object],
            session_id: str | None = None,
            role: str | None = None,
            seat: str | None = None,
            provider: str | None = "claude",
            access: str | None = None,
        ) -> dict[str, object]:
            result = await client.call_tool(
                "start_agent",
                {
                    "provider": provider,
                    "prompt": "ok",
                    "cwd": str(tmp_path),
                    "provider_options": options,
                    "session_id": session_id,
                    "role": role,
                    "seat": seat,
                    "access": access,
                },
            )
            assert not result.is_error, result.content
            payload = _payload(result.structured_content)
            await client.call_tool("wait", {"ids": [payload["invocation_id"]]})
            return payload

        monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        modes = cast(dict[str, dict[str, object]], listed["modes"])
        task_member = cast(list[dict[str, object]], modes["task"]["members"])[0]
        assert task_member == {
            "provider": "claude",
            "model": "chosen",
            "default": True,
            "effort": "medium",
            "efforts": ["medium", "high"],
            "options": {"agent": "reviewer"},
            "notes": "任务",
            "permitted": False,
            "valid": True,
            "error": None,
        }
        discussion = cast(list[dict[str, object]], modes["discussion"]["members"])
        assert discussion[-1] == {
            "provider": "claude",
            "model": "*",
            "default": False,
            "effort": None,
            "efforts": [],
            "options": {},
            "notes": "不限模型",
            "permitted": True,
            "valid": True,
            "error": None,
        }
        monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset[str]())
        defaulted = await start({})
        assert defaulted["model"] == "chosen" and defaulted["effort"] == "medium"
        # Without permissions.json every CLI with a tier runs it at "auto".
        applied = ["model", "effort", "agent", "permission_mode"]
        assert defaulted["defaults_applied"] == applied
        resumed = await start({}, "session-e2e")
        assert resumed["defaults_applied"] == applied
        explicit = await start({"model": "chosen", "effort": "high", "agent": "explicit"})
        assert explicit["defaults_applied"] == ["permission_mode"] and explicit["effort"] == "high"
        # A CLI-wide setting joins the model's own defaults.
        (tmp_path / "cli-options.json").write_text(
            json.dumps({"version": 1, "clis": {"claude": {"max_budget_usd": 1}}})
        )
        wide = await start({"model": "chosen"})
        assert wide["defaults_applied"] == ["effort", "agent", "max_budget_usd", "permission_mode"]
        (tmp_path / "cli-options.json").unlink()
        # A review or Explorer/Worker role brings the seat the user picked for this CLI.
        seat = {"provider": "claude", "model": "chosen", "effort": "high"}
        (tmp_path / "review.json").write_text(
            json.dumps({"version": 1, "seats": {"correctness-reviewer": seat}})
        )
        (tmp_path / "orchestration.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "preset": "split",
                    "worker": {"easy": seat},
                    "explorer": {"hard": seat},
                }
            )
        )
        seated = await start({}, role="correctness-reviewer")
        assert seated["effort"] == "high"
        assert seated["defaults_applied"] == [
            "correctness-reviewer seat",
            "model",
            "effort",
            "agent",
            "permission_mode",
            "access (correctness-reviewer seat)",
            "read-only instruction (access=read)",
        ]
        assert seated["access"] == "read"
        worker = await start({"effort": "medium"}, role="worker-easy")
        assert worker["effort"] == "medium"
        assert worker["defaults_applied"] == [
            "worker-easy seat",
            "model",
            "agent",
            "permission_mode",
            "access (worker-easy seat)",
        ]
        assert worker["access"] == "write"
        assert (await start({}, role="explorer-easy"))["defaults_applied"] == [
            *applied,
            "access (explorer-easy seat)",
            "read-only instruction (access=read)",
        ]
        # seat picks the difficulty, so role stays free for the desk's own words.
        named = await start({}, role="接手方视角", seat="worker-easy")
        assert cast(list[str], named["defaults_applied"])[0] == "worker-easy seat"
        inferred = await start({}, role="实现参数推断", seat="worker-easy", provider=None)
        assert inferred["provider"] == "claude" and inferred["access"] == "write"
        assert "provider (worker-easy seat)" in cast(list[str], inferred["defaults_applied"])
        review = await start({}, role="correctness-reviewer", provider=None)
        assert review["access"] == "read"
        assert "access (correctness-reviewer seat)" in cast(list[str], review["defaults_applied"])
        contradiction = await client.call_tool(
            "start_agent",
            {"prompt": "ok", "cwd": str(tmp_path), "seat": "worker-easy", "access": "read"},
        )
        assert contradiction.is_error
        assert "worker-easy declares access=write" in str(contradiction.content)
        # The current host's off override suppresses role defaults without deleting saved seats.
        from agentnave.host import Host

        monkeypatch.setattr(mcp_server, "HOST", Host("grok", "env"))
        (tmp_path / "orchestration.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "preset": "split",
                    "apps": {"grok": "off"},
                    "worker": {"easy": seat},
                    "explorer": {"hard": seat},
                }
            )
        )
        disabled = _payload((await client.call_tool("list_members", {})).structured_content)
        assert disabled["orchestration"] == {"enabled": False, "explorer": {}, "worker": {}}
        assert (await start({}, role="worker-easy"))["defaults_applied"] == [
            *applied,
            "access (worker-easy seat)",
        ]
        assert (await start({}, role="explorer-hard"))["defaults_applied"] == [
            *applied,
            "access (explorer-hard seat)",
            "read-only instruction (access=read)",
        ]
        (tmp_path / "review.json").write_text(
            json.dumps(
                {"version": 1, "seats": {"correctness-reviewer": {**seat, "provider": "codex"}}}
            )
        )
        assert (await start({}, role="correctness-reviewer"))["defaults_applied"] == [
            *applied,
            "access (correctness-reviewer seat)",
            "read-only instruction (access=read)",
        ]
        (tmp_path / "review.json").write_text(
            json.dumps({"version": 1, "seats": {"correctness-reviewer": {**seat, "effort": "low"}}})
        )
        # The seat is the user's pick for the role: the task members' effort range does not bind it.
        assert (await start({}, role="correctness-reviewer"))["effort"] == "low"
        assert (await start({"model": "chosen", "effort": "medium"}, role="correctness-reviewer"))[
            "effort"
        ] == "medium"
        (tmp_path / "review.json").unlink()
        (tmp_path / "orchestration.json").unlink()
        for options in ({"model": "outside"}, {"effort": "low"}, {"unknown": True}):
            result = await client.call_tool(
                "start_agent",
                {
                    "provider": "claude",
                    "prompt": "ok",
                    "cwd": str(tmp_path),
                    "provider_options": options,
                },
            )
            assert result.is_error
        rejected = await client.call_tool(
            "start_agent", {"provider": "codex", "prompt": "ok", "cwd": str(tmp_path)}
        )
        assert rejected.is_error and "claude/chosen" in str(rejected.content)
        monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
        rejected = await client.call_tool(
            "start_agent", {"provider": "claude", "prompt": "ok", "cwd": str(tmp_path)}
        )
        assert rejected.is_error and "excluded by this host" in str(rejected.content)
        assert mcp_server.PROVIDER_SELECTION in str(rejected.content)
        grouped = await client.call_tool(
            "run_round",
            {
                "title": "Defaults",
                "cwd": str(tmp_path),
                "members": [
                    {"id": "a", "provider": "claude"},
                    {"id": "b", "provider": "claude", "provider_options": {"model": "outside"}},
                ],
            },
        )
        assert not grouped.is_error, grouped.content
        seats = cast(list[dict[str, object]], _payload(grouped.structured_content)["members"])
        assert seats[0]["model"] == "chosen" and seats[0]["effort"] == "high"
        assert seats[0]["defaults_applied"] == ["model", "effort", "permission_mode"]
        assert seats[1]["model"] == "outside" and seats[1]["effort"] is None
        cast(dict[str, object], document["modes"])["discussion"] = {}
        path.write_text(json.dumps(document), encoding="utf-8")
        rejected = await client.call_tool(
            "run_round",
            {
                "title": "Restricted",
                "cwd": str(tmp_path),
                "members": [{"id": "a", "provider": "claude"}, {"id": "b", "provider": "claude"}],
            },
        )
        assert rejected.is_error and "allowed members: none" in str(rejected.content)


def test_host_startup_merges_exclusions_and_reports_identity(tmp_path: Path) -> None:
    import json

    environment = {
        key: value
        for key, value in os.environ.items()
        if key not in {"AGENTNAVE_HOST", "AGENTNAVE_EXCLUDED_PROVIDERS"}
    }
    environment.update(AGENTNAVE_HOST="workbuddy", AGENTNAVE_EXCLUDED_PROVIDERS="grok")
    completed = subprocess.run(
        [
            sys.executable,
            "-c",
            "import asyncio, json; from agentnave import mcp_server as s; "
            "print(json.dumps([asyncio.run(s.list_members('codebuddy')).model_dump(mode='json'), "
            "sorted(s.EXCLUDED_PROVIDERS), s.PROVIDER_SELECTION]))",
        ],
        env=environment,
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=True,
    )
    description, excluded, instructions = json.loads(completed.stdout)
    assert description["host"] == "workbuddy" and description["host_source"] == "env"
    assert description["provider_details"]["permitted"] is False
    assert excluded == ["codebuddy", "grok"]
    assert "Excluded providers: codebuddy, grok" in instructions
    environment["AGENTNAVE_HOST"] = "unknown"
    invalid = subprocess.run(
        [sys.executable, "-c", "import agentnave.mcp_server"],
        env=environment,
        cwd=tmp_path,
        capture_output=True,
        text=True,
    )
    assert invalid.returncode != 0 and "Unknown AGENTNAVE_HOST" in invalid.stderr

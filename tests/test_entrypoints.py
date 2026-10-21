from __future__ import annotations

import asyncio
import os
import subprocess
import sys
from pathlib import Path
from typing import cast

import pytest
from mcp import Client, StdioServerParameters
from mcp_types import TextContent

from agentnave import __version__, mcp_server
from agentnave.mcp_server import mcp

EXPECTED_TOOLS = [
    "start_agent",
    "wait_agent",
    "wait_any",
    "cancel_agent",
    "list_invocations",
    "describe_provider",
    "list_members",
    "open_group",
    "run_round",
    "wait_round",
    "cancel_round",
    "decide_answers",
    "post_host_message",
    "list_conversations",
    "read_conversation",
    "update_conversation",
]


@pytest.fixture(autouse=True)
def host_context(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave.host import Host

    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setattr(mcp_server, "HOST", Host())
    monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset[str]())


def _server_args(module: str, status: str) -> list[str]:
    # Model the host context explicitly: the test runner itself may be a dispatched CLI.
    return [
        "-c",
        "import os; os.environ.setdefault('AGENTNAVE_HOST', 'gemini'); "
        f"import {module} as server; "
        "from agentnave.nesting import NestingResult; "
        f"server.detect_nesting = lambda: NestingResult({status!r}, 'test context'); "
        "server.main()",
    ]


def _install_fake_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    script = tmp_path / ("claude.py" if os.name == "nt" else "claude")
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys, time\n"
        "prompt = sys.stdin.read()\n"
        "if prompt == 'finish':\n"
        "    for _ in range(8):\n"
        "        print(json.dumps({'type':'stream_event','event':{'delta':{'type':'text_delta','text':'working '}}}), flush=True)\n"
        "        time.sleep(0.05)\n"
        "if prompt == 'blocker':\n"
        "    for text in ['旧' * 1100, '新进展']:\n"
        "        print(json.dumps({'type':'stream_event','event':{'delta':{'type':'text_delta','text':text}}}), flush=True)\n"
        "    print(json.dumps({'type':'assistant','message':{'content':[{'type':'tool_use','id':'t1','name':'Bash','input':{'secret':'DO_NOT_RETURN'}}]}}), flush=True)\n"
        "    for _ in range(5):\n"
        "        print(json.dumps({'type':'system','subtype':'api_retry','error':'authentication_failed'}), flush=True)\n"
        "        time.sleep(0.05)\n"
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
        async with asyncio.timeout(3):
            response = await client.call_tool("wait_agent", {"invocation_id": invocation_id})
        payload = _payload(response.structured_content)
        assert payload["status"] == "running"
        assert payload["reason"] == "execution_blocked"
        assert _payload(payload["error"])["code"] == "authentication_failed"
        assert payload["output"] == "旧" * 997 + "新进展"
        assert "DO_NOT_RETURN" not in str(payload)
        assert "provider_usage" not in payload
        monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.4)
        later = await client.call_tool("wait_agent", {"invocation_id": invocation_id})
        assert _payload(later.structured_content)["reason"] == "wait_elapsed"
        assert _payload(later.structured_content)["output"] == payload["output"]
        assert cast(int, _payload(later.structured_content)["output_age_ms"]) > cast(
            int, payload["output_age_ms"]
        )
        cancelled = await client.call_tool("cancel_agent", {"invocation_id": invocation_id})
        assert _payload(cancelled.structured_content)["status"] == "cancelled"


@pytest.mark.asyncio
async def test_mcp_lists_lifecycle_and_discovery_tools_with_structured_contracts() -> None:
    async with Client(mcp) as client:
        result = await client.list_tools()

    assert [tool.name for tool in result.tools] == EXPECTED_TOOLS
    assert all(tool.output_schema is not None for tool in result.tools)
    assert result.tools[0].annotations is not None
    assert result.tools[0].annotations.open_world_hint is True
    assert result.tools[0].annotations.destructive_hint is True
    assert result.tools[1].annotations is not None
    assert result.tools[1].annotations.read_only_hint is True
    start_properties = _payload(result.tools[0].input_schema["properties"])
    assert "timeout_seconds" not in start_properties
    wait_properties = _payload(result.tools[1].input_schema["properties"])
    assert set(wait_properties) == {"invocation_id"}
    assert mcp_server.WAIT_SECONDS == 300
    assert result.tools[3].annotations is not None
    assert result.tools[3].annotations.destructive_hint is True
    assert all(
        tool.annotations is not None and tool.annotations.read_only_hint is True
        for tool in (result.tools[2], result.tools[4])
    )


@pytest.mark.asyncio
async def test_provider_details_are_disclosed_only_on_request(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("PATH", "")
    async with Client(mcp) as client:
        before = await client.list_tools()
        catalog = str(before) + str(client.instructions)
        assert "hy4-preview" not in catalog
        assert "gpt-6-astra" not in catalog
        assert "permission_mode" not in catalog
        assert "describe_provider" in catalog
        described = await client.call_tool("describe_provider", {"provider": "codebuddy"})
        payload = _payload(described.structured_content)
        assert described.is_error is False
        assert payload["provider"] == "codebuddy"
        assert set(payload) == {
            "provider",
            "permitted",
            "show_permitted",
            "supported_options",
            "host",
            "host_source",
        }
        assert "permission_mode" in str(payload["supported_options"])
        assert "gpt-6-astra" not in str(payload)
        assert await client.list_tools() == before
        invalid = await client.call_tool("describe_provider", {"provider": "unknown"})
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
                    "wait_agent",
                    {
                        "invocation_id": started_payload["invocation_id"],
                    },
                )
                payload = _payload(finished.structured_content)
                if payload["status"] != "running":
                    break
                running_payloads.append(payload)

    assert len(running_payloads) >= 2
    assert all(payload["reason"] == "wait_elapsed" for payload in running_payloads)
    assert any("working" in str(payload.get("output", "")) for payload in running_payloads)
    finished_payload = _payload(finished.structured_content)
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
        opened = await client.call_tool(
            "open_group",
            {
                "title": "Parallel",
                "cwd": str(tmp_path),
                "seats": [{"id": s, "provider": "claude"} for s in "ab"],
            },
        )
        group_id = _payload(opened.structured_content)["group_id"]
        started = await client.call_tool(
            "run_round",
            {
                "group_id": group_id,
                "seats": ["a", "b"],
                "order": "parallel",
                "prompt": "compare notes",
                "instructions": {"b": "argue the other side"},
            },
        )
        started_payload = _payload(started.structured_content)
        assert started_payload["reason"] == "started"
        seats = cast(list[dict[str, object]], started_payload["seats"])
        assert [seat["status"] for seat in seats] == ["running", "running"]
        async with asyncio.timeout(10):
            while True:
                waited = await client.call_tool(
                    "wait_round", {"round_id": started_payload["round_id"]}
                )
                payload = _payload(waited.structured_content)
                if payload["status"] != "running":
                    break
        rejected = await client.call_tool(
            "run_round",
            {"group_id": group_id, "seats": ["c"], "order": "serial", "prompt": "x"},
        )
        status_only = await client.call_tool(
            "wait_round", {"round_id": started_payload["round_id"], "include_output": False}
        )
        compact = _payload(status_only.structured_content)
        assert all("output" not in seat for seat in cast(list[dict[str, object]], compact["seats"]))
        reread = await client.call_tool("wait_round", {"round_id": started_payload["round_id"]})
        assert _payload(reread.structured_content) == payload

    assert payload["status"] == "finished" and payload["reason"] == "finished"
    outputs = [str(seat["output"]) for seat in cast(list[dict[str, object]], payload["seats"])]
    assert all("COMPARE NOTES" in output for output in outputs)
    assert "ARGUE THE OTHER SIDE" not in outputs[0] and "ARGUE THE OTHER SIDE" in outputs[1]
    assert rejected.is_error is True and "unknown seat ids: c" in str(rejected.content)


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
        finished = await client.call_tool("wait_agent", {"invocation_id": invocation_id})

    finished_payload = _payload(finished.structured_content)
    invocation_result = finished_payload
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
        ids: list[object] = []
        for prompt in ("sleep", "private task text"):
            started = await client.call_tool(
                "start_agent", {"provider": "claude", "prompt": prompt, "cwd": str(tmp_path)}
            )
            ids.append(_payload(started.structured_content)["invocation_id"])
        sleeping, finished = ids
        async with asyncio.timeout(3):
            for invalid in ([], [finished, finished], [sleeping, "missing"]):
                rejected = await client.call_tool("wait_any", {"invocation_ids": invalid})
                assert rejected.is_error is True
            ready = await client.call_tool("wait_any", {"invocation_ids": [sleeping, finished]})
        ready_payload = _payload(ready.structured_content)
        assert ready_payload["reason"] == "ready"
        (update,) = cast(list[dict[str, object]], ready_payload["updates"])
        assert update["invocation_id"] == finished
        assert (update["status"], update["reason"]) == ("succeeded", "finished")

        listed = _payload((await client.call_tool("list_invocations", {})).structured_content)
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

        monkeypatch.setattr("agentnave.mcp_server.WAIT_SECONDS", 0.2)
        elapsed = _payload(
            (await client.call_tool("wait_any", {"invocation_ids": [sleeping]})).structured_content
        )
        assert elapsed["reason"] == "wait_elapsed"
        (running,) = cast(list[dict[str, object]], elapsed["updates"])
        assert (running["invocation_id"], running["status"]) == (sleeping, "running")
        await asyncio.sleep(1.0)
        listed = _payload((await client.call_tool("list_invocations", {})).structured_content)
        assert [
            item["invocation_id"] for item in cast(list[dict[str, object]], listed["invocations"])
        ] == [sleeping]
        await client.call_tool("cancel_agent", {"invocation_id": sleeping})


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
        running = await client.call_tool(
            "wait_agent",
            {"invocation_id": invocation_id},
        )
        cancelled = await client.call_tool("cancel_agent", {"invocation_id": invocation_id})

    running_payload = _payload(running.structured_content)
    cancelled_payload = _payload(cancelled.structured_content)
    cancelled_result = cancelled_payload
    assert running_payload["status"] == "running"
    assert isinstance(running_payload["elapsed_ms"], int)
    assert running_payload["reason"] == "wait_elapsed"
    assert "snapshot" not in running_payload
    assert cancelled_payload["reason"] == "finished"
    assert cancelled_result["status"] == "cancelled"


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
        finished = await client.call_tool("wait_agent", {"invocation_id": invocation_id})
        assert _payload(finished.structured_content)["status"] == "succeeded"
        await asyncio.sleep(0.6)
        for tool in ("wait_agent", "cancel_agent"):
            expired = await client.call_tool(tool, {"invocation_id": invocation_id})
            assert expired.is_error
            assert "expired" in str(expired.content)
            assert "seven days" in str(expired.content)


@pytest.mark.asyncio
async def test_mcp_unknown_invocation_error_tells_agent_how_to_recover() -> None:
    async with Client(mcp) as client:
        result = await client.call_tool("wait_agent", {"invocation_id": "missing"})

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
        finished = await client.call_tool(
            "wait_agent",
            {"invocation_id": invocation_id},
        )

    finished_payload = _payload(finished.structured_content)
    invocation_result = finished_payload
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
        args=_server_args("agentnave.mcp_server", "not_nested"),
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
    [" CODEX, codex, ", "antigravity,claude,codebuddy,codex,deepseek,grok,pi"],
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
        args=_server_args("agentnave.mcp_server", "not_nested"),
        env={
            "PATH": os.environ["PATH"],
            "AGENTNAVE_EXCLUDED_PROVIDERS": excluded,
            "AGENTNAVE_DATA_DIR": str(tmp_path / "data"),
        },
    )
    async with Client(parameters) as client:
        tools = (await client.list_tools()).tools
        description = str(tools[0].input_schema)
        assert client.instructions is not None
        assert "describe_provider" in client.instructions
        assert "Excluded providers:" in client.instructions
        assert "Excluded providers:" in description
        described = await client.call_tool("describe_provider", {"provider": "codex"})
        assert _payload(described.structured_content)["permitted"] is False
        rejected = await client.call_tool(
            "start_agent", {"provider": "codex", "prompt": "finish", "cwd": str(tmp_path)}
        )
        assert rejected.is_error is True
        assert "excluded by this host" in str(rejected.content)
        grouped = await client.call_tool(
            "open_group",
            {
                "title": "Excluded",
                "cwd": str(tmp_path),
                "seats": [{"id": p, "provider": p} for p in ("codex", "grok")],
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
                "wait_agent",
                {
                    "invocation_id": _payload(started.structured_content)["invocation_id"],
                },
            )
            result = _payload(finished.structured_content)
            assert result["output"] == "FINISH"


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
async def test_entrypoint_disables_all_tools_without_side_effects(
    tmp_path: Path,
) -> None:
    for status in ("nested", "unavailable"):
        parameters = StdioServerParameters(
            command=sys.executable,
            args=_server_args("agentnave.mcp_server", status),
            env={
                "HOME": str(tmp_path),
                "USERPROFILE": str(tmp_path),
                "AGENTNAVE_DATA_DIR": str(tmp_path / "data"),
            },
            cwd=tmp_path,
        )
        async with Client(parameters) as client:
            assert (await client.list_tools()).tools == []
            assert client.instructions is not None
            assert "已禁用编排工具" in client.instructions
            assert "test context" in client.instructions
            assert ("无法确认" in client.instructions) == (status == "unavailable")
        assert not list(tmp_path.iterdir())


@pytest.mark.asyncio
async def test_members_read_missing_and_fresh_files_without_preparing_commands(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json

    from agentnave.adapters.antigravity import AntigravityAdapter
    from agentnave.adapters.grok import GrokAdapter

    root = tmp_path / "data"
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(root))
    monkeypatch.setattr(mcp_server, "EXCLUDED_PROVIDERS", frozenset({"codex"}))

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
        content = json.dumps(document, ensure_ascii=False)
        path = root / "members.json"
        path.write_text(content, encoding="utf-8")
        listed = await client.call_tool("list_members", {})
        assert not listed.is_error
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
        assert list(root.iterdir()) == [path]
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
                    "open_group",
                    {
                        "title": "Broken",
                        "cwd": str(tmp_path),
                        "seats": [
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
            options: dict[str, object], session_id: str | None = None
        ) -> dict[str, object]:
            result = await client.call_tool(
                "start_agent",
                {
                    "provider": "claude",
                    "prompt": "ok",
                    "cwd": str(tmp_path),
                    "provider_options": options,
                    "session_id": session_id,
                },
            )
            assert not result.is_error, result.content
            payload = _payload(result.structured_content)
            await client.call_tool("wait_agent", {"invocation_id": payload["invocation_id"]})
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
        assert defaulted["defaults_applied"] == ["model", "effort", "agent"]
        resumed = await start({}, "session-e2e")
        assert resumed["defaults_applied"] == ["model", "effort", "agent"]
        explicit = await start({"model": "chosen", "effort": "high", "agent": "explicit"})
        assert explicit["defaults_applied"] == [] and explicit["effort"] == "high"
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
            "open_group",
            {
                "title": "Defaults",
                "cwd": str(tmp_path),
                "seats": [
                    {"id": "a", "provider": "claude"},
                    {"id": "b", "provider": "claude", "provider_options": {"model": "outside"}},
                ],
            },
        )
        assert not grouped.is_error, grouped.content
        seats = cast(list[dict[str, object]], _payload(grouped.structured_content)["seats"])
        assert seats[0]["model"] == "chosen" and seats[0]["effort"] == "high"
        assert seats[0]["defaults_applied"] == ["model", "effort", "permission_mode"]
        assert seats[1]["model"] == "outside" and seats[1]["effort"] is None
        cast(dict[str, object], document["modes"])["discussion"] = {}
        path.write_text(json.dumps(document), encoding="utf-8")
        rejected = await client.call_tool(
            "open_group",
            {
                "title": "Restricted",
                "cwd": str(tmp_path),
                "seats": [{"id": "a", "provider": "claude"}, {"id": "b", "provider": "claude"}],
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
            "print(json.dumps([asyncio.run(s.describe_provider('codebuddy')), "
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
    assert description["permitted"] is False and excluded == ["codebuddy", "grok"]
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

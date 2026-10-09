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

    assert [tool.name for tool in result.tools] == [
        "start_agent",
        "wait_agent",
        "cancel_agent",
        "describe_provider",
        "open_discussion",
        "list_conversations",
        "start_discussion_turn",
        "read_conversation",
        "decide_discussion_turn",
        "decide_discussion_round",
        "post_discussion_message",
        "reset_discussion_seat",
        "open_workbench",
        "update_conversation",
        "mark_discussion_absent",
        "continue_discussion",
    ]
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
    assert result.tools[2].annotations is not None
    assert result.tools[2].annotations.destructive_hint is True


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
            "discussion_permitted",
            "supported_options",
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
    assert finished.is_error is False
    assert finished_payload["reason"] == "finished"
    assert invocation_result["status"] == "succeeded"
    assert invocation_result["output"] == "FINISH"
    assert invocation_result["session_id"] == "session-e2e"


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


@pytest.mark.asyncio
@pytest.mark.parametrize("cwd", ["relative", "~", "~missing-user"])
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
    assert "Unable to prepare invocation" in str(result.content)
    assert "Invalid invocation request" not in str(result.content)


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
async def test_stdio_entrypoint_exposes_mcp_tools() -> None:
    parameters = StdioServerParameters(
        command=sys.executable,
        args=["-m", "agentnave.mcp_server"],
        env={"AGENTNAVE_DATA_DIR": os.environ["AGENTNAVE_DATA_DIR"]},
    )
    async with Client(parameters) as client:
        assert client.server_info is not None
        assert client.server_info.name == "AgentNave"
        assert client.server_info.version == __version__
        result = await client.list_tools()

    assert [tool.name for tool in result.tools] == [
        "start_agent",
        "wait_agent",
        "cancel_agent",
        "describe_provider",
        "open_discussion",
        "list_conversations",
        "start_discussion_turn",
        "read_conversation",
        "decide_discussion_turn",
        "decide_discussion_round",
        "post_discussion_message",
        "reset_discussion_seat",
        "open_workbench",
        "update_conversation",
        "mark_discussion_absent",
        "continue_discussion",
    ]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "excluded",
    [" CODEX, codex, ", "antigravity,claude,codebuddy,codex,grok"],
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
        args=["-m", "agentnave.mcp_server"],
        env={
            "PATH": os.environ["PATH"],
            "AGENTNAVE_EXCLUDED_PROVIDERS": excluded,
            "AGENTNAVE_DATA_DIR": os.environ["AGENTNAVE_DATA_DIR"],
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
        assert _payload(described.structured_content)["discussion_permitted"] is True
        opened = await client.call_tool(
            "open_discussion",
            {
                "directory": str(tmp_path / "show"),
                "title": "Host CLI is a separate contestant",
                "seats": [
                    {"id": p, "label": p, "provider": p, "model": "explicit", "effort": "low"}
                    for p in ("codex", "grok")
                ],
            },
        )
        assert opened.is_error is False
        rejected = await client.call_tool(
            "start_agent", {"provider": "codex", "prompt": "finish", "cwd": str(tmp_path)}
        )
        assert rejected.is_error is True
        assert "excluded by this host" in str(rejected.content)
        assert not marker.exists()
        if "claude" in excluded:
            assert (
                "Providers permitted for ordinary start_agent by this host configuration: none"
                in description
            )
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
async def test_workbench_auto_collects_private_grouped_tasks_and_restores_history(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json
    from urllib.error import HTTPError
    from urllib.request import urlopen

    _install_fake_claude(tmp_path, monkeypatch)

    def fetch(url: str) -> str:
        with urlopen(url, timeout=3) as response:
            return response.read().decode()

    async with Client(mcp) as client:
        first = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "finish",
                        "cwd": str(tmp_path),
                        "title": "A private job",
                    },
                )
            ).structured_content
        )
        rid = first["conversation_id"]
        second = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "private second output",
                        "cwd": str(tmp_path),
                        "conversation_id": rid,
                    },
                )
            ).structured_content
        )
        for started in (first, second):
            await client.call_tool("wait_agent", {"invocation_id": started["invocation_id"]})
        view = _payload(
            (await client.call_tool("read_conversation", {"room_id": rid})).structured_content
        )
        room = _payload(view["room"])
        runs = cast(list[dict[str, object]], room["tasks"])
        assert [run["output"] for run in runs] == ["FINISH", "PRIVATE SECOND OUTPUT"]
        assert all(run["status"] == "succeeded" for run in runs)
        assert view["public_url"] == ""
        desk = await asyncio.to_thread(fetch, str(view["director_url"]) + "state")
        assert "PRIVATE SECOND OUTPUT" in desk
        assert "session-e2e" not in desk and str(first["invocation_id"]) not in desk
        with pytest.raises(HTTPError) as error:
            await asyncio.to_thread(fetch, str(view["director_url"]) + "transcript.jsonl")
        assert error.value.code == 404
        public = _payload(
            (
                await client.call_tool(
                    "open_discussion",
                    {
                        "title": "Separate discussion",
                        "seats": [
                            {
                                "id": p,
                                "label": p,
                                "provider": p,
                                "model": "explicit",
                                "effort": "low",
                            }
                            for p in ("claude", "grok")
                        ],
                    },
                )
            ).structured_content
        )
        public_body = await asyncio.to_thread(fetch, str(public["public_url"]) + "state")
        assert "PRIVATE SECOND OUTPUT" not in public_body and "A private job" not in public_body
        assert "workbench_url" not in public_body and "storage" not in public_body
        denied = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "do not mix",
                "cwd": str(tmp_path),
                "conversation_id": _payload(public["room"])["id"],
            },
        )
        assert denied.is_error
        sleeping = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "sleep",
                        "cwd": str(tmp_path),
                        "conversation_id": rid,
                        "session_id": "session-e2e",
                    },
                )
            ).structured_content
        )
        duplicate = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "duplicate",
                "cwd": str(tmp_path),
                "conversation_id": rid,
                "session_id": "session-e2e",
            },
        )
        assert duplicate.is_error
        assert (
            await client.call_tool("update_conversation", {"room_id": rid, "archived": True})
        ).is_error
        await client.call_tool("cancel_agent", {"invocation_id": sleeping["invocation_id"]})
        await client.call_tool("read_conversation", {"room_id": rid})
        changed = await client.call_tool(
            "update_conversation", {"room_id": rid, "title": "Renamed job", "archived": True}
        )
        assert not changed.is_error
        homepage = json.loads(await asyncio.to_thread(fetch, str(first["workbench_url"]) + "state"))
        assert len(homepage["shows"]) == 2
        assert "PRIVATE SECOND OUTPUT" not in json.dumps(homepage)

    # A durable running record represents an interrupted process, not a resumable invocation.
    directory = Path(
        json.loads(
            (Path(os.environ["AGENTNAVE_DATA_DIR"]) / "catalog" / (str(rid) + ".json")).read_text()
        )
    )
    state_file = directory / "room.json"
    saved = json.loads(state_file.read_text())
    saved["tasks"][0]["status"] = "running"
    saved["tasks"][0]["invocation_id"] = "expired-invocation"
    state_file.write_text(json.dumps(saved))
    async with Client(mcp) as client:
        view = _payload(
            (await client.call_tool("read_conversation", {"room_id": rid})).structured_content
        )
        room = _payload(view["room"])
        assert room["title"] == "Renamed job" and room["archived"] is True
        runs = cast(list[dict[str, object]], room["tasks"])
        assert runs[0]["status"] == "interrupted" and runs[0]["invocation_id"] is None
        assert runs[1]["output"] == "PRIVATE SECOND OUTPUT"
        await client.call_tool("update_conversation", {"room_id": rid, "archived": False})
        resumed = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "continued",
                        "cwd": str(tmp_path),
                        "session_id": "session-e2e",
                    },
                )
            ).structured_content
        )
        assert resumed["conversation_id"] == rid
        await client.call_tool("wait_agent", {"invocation_id": resumed["invocation_id"]})

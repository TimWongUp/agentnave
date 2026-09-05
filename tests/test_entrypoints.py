from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from typing import cast

import pytest
from mcp import Client, StdioServerParameters
from mcp_types import TextContent

from agentnave import __version__
from agentnave.mcp_server import mcp


def _install_fake_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    executable = tmp_path / "claude"
    executable.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys, time\n"
        "prompt = sys.stdin.read()\n"
        "if prompt == 'sleep':\n"
        "    time.sleep(30)\n"
        "if prompt == 'malformed usage':\n"
        '    print(\'{"type":"result","result":"MALFORMED USAGE",\' '
        '+ \'"session_id":"session-e2e","num_turns":{"invalid":true},\' '
        "+ '\"total_cost_usd\":' + '1' * 5001 + ',\"is_error\":false}')\n"
        "else:\n"
        "    print(json.dumps({'type': 'result', 'result': prompt.upper(), "
        "'session_id': 'session-e2e', 'num_turns': 1, 'is_error': False}))\n"
    )
    executable.chmod(0o755)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")


def _payload(value: object) -> dict[str, object]:
    return cast(dict[str, object], value)


@pytest.mark.asyncio
async def test_mcp_lists_only_agent_lifecycle_tools_with_structured_contracts() -> None:
    async with Client(mcp) as client:
        result = await client.list_tools()

    assert [tool.name for tool in result.tools] == [
        "start_agent",
        "wait_agent",
        "cancel_agent",
    ]
    assert all(tool.output_schema is not None for tool in result.tools)
    assert result.tools[0].annotations is not None
    assert result.tools[0].annotations.open_world_hint is True
    assert result.tools[0].annotations.destructive_hint is True
    assert result.tools[1].annotations is not None
    assert result.tools[1].annotations.read_only_hint is True
    assert result.tools[2].annotations is not None
    assert result.tools[2].annotations.destructive_hint is True


@pytest.mark.asyncio
async def test_mcp_starts_and_waits_for_provider_with_structured_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)

    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "finish", "cwd": str(tmp_path)},
        )
        started_payload = _payload(started.structured_content)
        finished = await client.call_tool(
            "wait_agent",
            {
                "invocation_id": started_payload["invocation_id"],
                "wait_timeout_seconds": 3,
            },
        )

    finished_payload = _payload(finished.structured_content)
    invocation_result = _payload(finished_payload["result"])
    assert started.is_error is False
    assert started_payload["state"] == "running"
    assert finished.is_error is False
    assert finished_payload["state"] == "finished"
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
        finished = await client.call_tool(
            "wait_agent", {"invocation_id": invocation_id, "wait_timeout_seconds": 3}
        )

    finished_payload = _payload(finished.structured_content)
    invocation_result = _payload(finished_payload["result"])
    assert finished.is_error is False
    assert invocation_result["status"] == "succeeded"
    assert invocation_result["output"] == "MALFORMED USAGE"
    assert invocation_result["session_id"] == "session-e2e"
    assert invocation_result["provider_usage"] == {}


@pytest.mark.asyncio
async def test_mcp_running_result_can_be_cancelled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)

    async with Client(mcp) as client:
        started = await client.call_tool(
            "start_agent",
            {"provider": "claude", "prompt": "sleep", "cwd": str(tmp_path)},
        )
        invocation_id = _payload(started.structured_content)["invocation_id"]
        running = await client.call_tool(
            "wait_agent",
            {"invocation_id": invocation_id, "wait_timeout_seconds": 0.01},
        )
        cancelled = await client.call_tool("cancel_agent", {"invocation_id": invocation_id})

    running_payload = _payload(running.structured_content)
    snapshot = _payload(running_payload["snapshot"])
    cancelled_payload = _payload(cancelled.structured_content)
    cancelled_result = _payload(cancelled_payload["result"])
    assert running_payload["state"] == "running"
    assert snapshot["phase"] in {"preparing", "running"}
    assert isinstance(snapshot["elapsed_ms"], int)
    assert cancelled_payload["state"] == "finished"
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
            {"invocation_id": invocation_id, "wait_timeout_seconds": 3},
        )

    finished_payload = _payload(finished.structured_content)
    invocation_result = _payload(finished_payload["result"])
    error = _payload(invocation_result["error"])
    assert finished.is_error is False
    assert invocation_result["status"] == "failed"
    assert error["code"] == "launch_error"
    assert "Traceback" not in str(finished.content)
    assert str(tmp_path) not in str(finished.content)


@pytest.mark.asyncio
async def test_stdio_entrypoint_exposes_mcp_tools() -> None:
    parameters = StdioServerParameters(command=sys.executable, args=["-m", "agentnave.mcp_server"])
    async with Client(parameters) as client:
        assert client.server_info is not None
        assert client.server_info.name == "AgentNave"
        assert client.server_info.version == __version__
        result = await client.list_tools()

    assert [tool.name for tool in result.tools] == [
        "start_agent",
        "wait_agent",
        "cancel_agent",
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
        env={"PATH": os.environ["PATH"], "AGENTNAVE_EXCLUDED_PROVIDERS": excluded},
    )
    async with Client(parameters) as client:
        tools = (await client.list_tools()).tools
        description = str(tools[0].input_schema)
        assert client.instructions is not None
        assert "model=opus, effort=max" in client.instructions
        assert "Excluded providers:" in client.instructions
        assert "Excluded providers:" in description
        assert "model=gpt-5.6-sol" in description
        rejected = await client.call_tool(
            "start_agent", {"provider": "codex", "prompt": "finish", "cwd": str(tmp_path)}
        )
        assert rejected.is_error is True
        assert "excluded by this host" in str(rejected.content)
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
                    "wait_timeout_seconds": 3,
                },
            )
            result = _payload(_payload(finished.structured_content)["result"])
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

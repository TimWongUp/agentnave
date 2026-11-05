from __future__ import annotations

import json
from typing import Any

import pytest
from jsonschema import Draft202012Validator
from jsonschema.protocols import Validator
from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp_types import CallToolResult, InputRequiredResult, TextContent

from agentnave import mcp_server
from agentnave.adapters import base


@pytest.fixture(autouse=True)
def no_common_provider_dirs(monkeypatch: pytest.MonkeyPatch) -> None:
    # Never let a test fall back to a real provider CLI installed on this machine.
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", ())


@pytest.fixture(autouse=True)
def manager_chat(monkeypatch: pytest.MonkeyPatch) -> None:
    # Dispatch and inbox receiving need a boss chat identity; tests run as one injected chat.
    monkeypatch.setenv("AGENTNAVE_HOST_SESSION_ID", "test-chat")
    # Running the suite inside Claude Code must not hand tests that chat's native identity.
    monkeypatch.delenv("CLAUDE_CODE_SESSION_ID", raising=False)


@pytest.fixture(autouse=True)
def check_mcp_reply_contract(monkeypatch: pytest.MonkeyPatch) -> None:
    """Check every successful in-process tool reply against its published contract."""
    server = mcp_server.mcp
    call_tool = server.call_tool
    validators: dict[str, Validator] = {}

    async def checked_call(
        name: str,
        arguments: dict[str, Any],
        context: Context[mcp_server.Runtime, Any] | None = None,
    ) -> CallToolResult | InputRequiredResult:
        result = await call_tool(name, arguments, context)
        if isinstance(result, CallToolResult) and not result.is_error:
            if not validators:
                # Include list_invocations, hidden from agents but called by the relay.
                for tool in await MCPServer.list_tools(server):
                    assert tool.output_schema is not None
                    validators[tool.name] = Draft202012Validator(tool.output_schema)
            validators[name].validate(result.structured_content)
            assert len(result.content) == 1
            text = result.content[0]
            assert isinstance(text, TextContent)
            assert json.loads(text.text) == result.structured_content
        return result

    monkeypatch.setattr(server, "call_tool", checked_call)

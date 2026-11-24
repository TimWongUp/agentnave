from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

import pytest
from jsonschema import Draft202012Validator
from jsonschema.protocols import Validator
from mcp.server import MCPServer
from mcp.server.mcpserver.context import Context
from mcp_types import CallToolResult, InputRequiredResult, TextContent

from agentnave import mcp_server, members, t3
from agentnave.adapters import base, codex
from agentnave.core import InvocationManager


@pytest.fixture(autouse=True)
def no_spawn_delay(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(InvocationManager, "spawn_interval", (0.0, 0.0))


@pytest.fixture(autouse=True)
def no_common_provider_dirs(monkeypatch: pytest.MonkeyPatch) -> None:
    # Never let a test fall back to a real provider CLI installed on this machine.
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", ())


@pytest.fixture(autouse=True)
def codex_auto_source(monkeypatch: pytest.MonkeyPatch) -> None:
    # The machine's saved Codex source must not choose which CLI a test finds.
    monkeypatch.setattr(codex, "_source", lambda: None)


@pytest.fixture(autouse=True)
def claude_over_print(monkeypatch: pytest.MonkeyPatch) -> None:
    # Fake Claude and agy CLIs here speak print mode; a test of an interactive default restores it.
    monkeypatch.setattr(members, "read_claude_interactive", lambda: False)
    monkeypatch.setattr(members, "read_antigravity_interactive", lambda: False)


@pytest.fixture(autouse=True)
def manager_chat(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # Dispatch and inbox receiving need a boss chat identity; tests run as one injected chat.
    monkeypatch.setenv("AGENTNAVE_HOST_SESSION_ID", "test-chat")
    # Running the suite inside Claude Code must not hand tests that chat's native identity.
    monkeypatch.delenv("CLAUDE_CODE_SESSION_ID", raising=False)
    # Nor the configuration directory it runs under: it decides which account the host is.
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    # Nor, inside any host or dispatched CLI, its connection identity and real data root: a
    # relay under test would otherwise leave receipts and faults in ~/.agentnave.
    for name in (
        "AGENTNAVE_HOST",
        "AGENTNAVE_HOST_APP",
        "AGENTNAVE_DISPATCHED",
        "AGENTNAVE_EXCLUDED_PROVIDERS",
        "CLAUDE_CODE_ENTRYPOINT",
    ):
        monkeypatch.delenv(name, raising=False)
    for name in [key for key in os.environ if key.upper().startswith("T3CODE_")]:
        monkeypatch.delenv(name, raising=False)
    t3.reset_t3_mode_cache()
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "agentnave-data"))


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

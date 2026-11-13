from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, cast

import pytest
from fake_cli import write_fake_cli
from jsonschema import Draft202012Validator
from mcp import Client, StdioServerParameters
from mcp.server import MCPServer
from mcp_types import CallToolResult, TextContent

from agentnave import __version__, mcp_runtime, mcp_server, seat_recommendation
from agentnave.adapters.base import AdapterHooks
from agentnave.decision_model import Decision, DecisionError, DecisionQuestion
from agentnave.host import Host
from agentnave.mcp_projection import without_empty_fields
from agentnave.mcp_server import mcp
from agentnave.orchestration_roles import role_catalog
from agentnave.processes import spawn_process, terminate_process_tree

EXPECTED_TOOLS = [
    "start_agent",
    "wait",
    "answer_prompt",
    "cancel",
    "list",
    "list_members",
    "clear_provider_cooldown",
    "run_round",
    "decide_answers",
    "handover",
    "read_conversation",
    "isolated_dirs",
]


@pytest.fixture(autouse=True)
def host_context(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave.host import Host

    def installed(_adapter: AdapterHooks) -> bool:
        return True

    monkeypatch.delenv("AGENTNAVE_HOST", raising=False)
    monkeypatch.delenv("AGENTNAVE_EXCLUDED_PROVIDERS", raising=False)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setattr(mcp_runtime, "HOST", Host())
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
    monkeypatch.setattr(AdapterHooks, "is_installed", installed)


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
        "assert prompt.startswith('You are an AgentNave subagent.')\n"
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
        "print(json.dumps({'type': 'result', 'result': prompt.upper(), "
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
async def test_specialist_seats_dispatch_both_difficulties_and_reject_disabled_resume(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    catalog = role_catalog()
    document = {
        "version": 1,
        "preset": "split",
        "enabled_roles": [role.group for role in catalog],
        **{
            role.group: {
                difficulty: {"provider": "claude", "model": f"{role.group}-{difficulty}"}
                for difficulty in ("easy", "hard")
            }
            for role in catalog
        },
    }
    path = tmp_path / "orchestration.json"
    path.write_text(json.dumps(document))
    async with Client(mcp) as client:
        for role in catalog:
            for difficulty in ("easy", "hard"):
                result = await _start_and_wait(
                    client, cwd=str(tmp_path), seat=f"{role.seat_prefix}-{difficulty}"
                )
                assert result["provider"] == "claude"
                assert result["model"] == f"{role.group}-{difficulty}"
                assert result["access"] == role.access
                assert {"permission_mode", "permission_fallback"} <= set(
                    cast(list[str], result["defaults_applied"])
                )
                assert (
                    "read-only instruction (access=read)"
                    in cast(list[str], result["defaults_applied"])
                ) is (role.access == "read")
        document["enabled_roles"] = []
        path.write_text(json.dumps(document))
        for role in catalog:
            for arguments in (
                {"cwd": str(tmp_path), "seat": f"{role.seat_prefix}-hard"},
                {"cwd": str(tmp_path), "seat": f"{role.seat_prefix}-hard", "provider": "claude"},
                {"session_id": "session-e2e", "seat": f"{role.seat_prefix}-hard"},
            ):
                rejected = await client.call_tool("start_agent", {"prompt": "ok", **arguments})
                assert rejected.is_error and "not enabled" in str(rejected.content)
                assert "inherited" not in str(rejected.content)
        for arguments in ({}, {"provider": "claude", "cwd": str(tmp_path)}):
            rejected = await client.call_tool(
                "start_agent", {"prompt": "ok", "session_id": "session-e2e", **arguments}
            )
            assert rejected.is_error and "worker-documents-hard seat is not enabled" in str(
                rejected.content
            )
            assert "inherited seat" in str(rejected.content)
        # Existing history stays readable and a new duty keeps the existing inheritance rule.
        resumed = await _start_and_wait(client, session_id="session-e2e", role="普通续接")
        assert "access" not in resumed
    assert json.loads(path.read_text()) == document
    path.unlink()
    async with Client(mcp) as client:
        rejected = await client.call_tool(
            "start_agent",
            {"prompt": "ok", "cwd": str(tmp_path), "provider": "claude", "seat": "worker-easy"},
        )
        assert rejected.is_error and "not enabled" in str(rejected.content)


@pytest.mark.asyncio
async def test_role_switches_filter_complete_published_schemas_and_member_fields(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "orchestration.json"
    catalog = role_catalog()
    document = {
        "version": 1,
        "preset": "split",
        **{role.group: {"easy": {"provider": "claude", "model": "saved"}} for role in catalog},
    }
    async with Client(mcp) as client:
        for enabled in (
            None,
            [],
            ["worker_writing", "explorer_web"],
            [role.group for role in catalog],
            [],
        ):
            path.write_text(json.dumps({**document, "enabled_roles": enabled}))
            expected = (
                set(enabled) if enabled is not None else {"explorer", "worker", "worker_frontend"}
            )
            tools = await client.list_tools(cache_mode="refresh")
            schemas = json.dumps([tool.model_dump(by_alias=True) for tool in tools.tools])
            start = next(tool for tool in tools.tools if tool.name == "start_agent")
            seats = {
                f"{role.seat_prefix}-{difficulty}"
                for role in catalog
                if role.group in expected
                for difficulty in ("easy", "hard")
            }
            if seats:
                assert set(start.input_schema["$defs"]["OrchestrationRole"]["enum"]) == seats
            else:
                assert "seat" not in start.input_schema["properties"]
            for tool in tools.tools:
                Draft202012Validator.check_schema(tool.input_schema)
                if tool.output_schema is not None:
                    Draft202012Validator.check_schema(tool.output_schema)
            listed = _payload((await client.call_tool("list_members", {})).structured_content)
            status = _payload(listed["orchestration"])
            for role in catalog:
                assert (role.group in status) is (role.group in expected)
                # Inspect all definitions, including serialization-mode model names.
                assert (f'"{role.group}"' in schemas) is (role.group in expected)
                for difficulty in ("easy", "hard"):
                    assert (f'"{role.seat_prefix}-{difficulty}"' in schemas) is (
                        role.group in expected
                    )
            assert '"image-generator"' not in schemas
            assert '"image_generation"' not in schemas


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
        assert "access" not in resumed


@pytest.mark.asyncio
async def test_resume_keeps_the_threads_last_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    async with Client(mcp) as client:
        await _start_and_wait(
            client,
            cwd=str(tmp_path),
            provider="claude",
            provider_options={"model": "cheap", "effort": "low"},
        )
        resumed = await _start_and_wait(client, session_id="session-e2e")
        assert (resumed["model"], resumed["effort"]) == ("cheap", "low")
        assert {"model (session_id history)", "effort (session_id history)"} <= set(
            cast(list[str], resumed["defaults_applied"])
        )
        # A role-named seat that supplies no model for this CLI must still keep the thread's.
        root = tmp_path / "data"
        (root / "review.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "seats": {"correctness-reviewer": {"provider": "codex", "model": "elsewhere"}},
                }
            )
        )
        seated = await _start_and_wait(
            client, session_id="session-e2e", role="correctness-reviewer"
        )
        assert (seated["model"], seated["effort"]) == ("cheap", "low")
        assert {"model (session_id history)", "effort (session_id history)"} <= set(
            cast(list[str], seated["defaults_applied"])
        )
        # A seat that does supply the model still wins over the thread's history.
        (root / "review.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "seats": {
                        "correctness-reviewer": {"provider": "claude", "model": "seat-model"}
                    },
                }
            )
        )
        chosen = await _start_and_wait(
            client, session_id="session-e2e", role="correctness-reviewer"
        )
        assert chosen["model"] == "seat-model"
        assert "model (session_id history)" not in cast(list[str], chosen["defaults_applied"])
        switched = await _start_and_wait(
            client, session_id="session-e2e", provider_options={"model": "other"}
        )
        assert switched["model"] == "other" and "effort" not in switched


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
        # Explicit identity wins; the unconfigured seat supplies no model, so the thread's stays.
        assert (explicit["model"], explicit["effort"]) == ("chosen", "high")
        assert {
            "provider (session_id history)",
            "cwd (session_id history)",
            "role (session_id history)",
            "seat (session_id history)",
        }.isdisjoint(cast(list[str], explicit["defaults_applied"]))
        latest = await _start_and_wait(client, session_id="session-e2e")
        assert latest["access"] == "read"
        renamed = await _start_and_wait(client, session_id="session-e2e", role="实现参数推断")
        assert "access" not in renamed
        assert "seat (session_id history)" not in cast(list[str], renamed["defaults_applied"])
        await _start_and_wait(
            client, session_id="session-e2e", role="只读复核", seat="explorer-hard"
        )
        kept = await _start_and_wait(client, session_id="session-e2e", role=" 只读复核 ")
        assert kept["access"] == "read"
        saved = Record.read(Database(root), root / "locks", str(started["conversation_id"]))
        assert saved.state.tasks[1].cwd == str(tmp_path)
        assert saved.state.tasks[-1].cwd == str(alternate)
        # An inherited seat that supplies no model keeps the thread's model and effort.
        assert (latest["model"], latest["effort"]) == ("chosen", "high")
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
    from agentnave.native_sessions import boss_key
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
                            model="own-model" if owner == "test-chat" else "PRIVATE_MODEL",
                            cwd="/PRIVATE_CWD",
                            prompt="PRIVATE_PROMPT",
                            role="own-role" if owner == "test-chat" else "PRIVATE_ROLE",
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
        assert str(cast(dict[str, object], selected["identity"])["handle"]).endswith("·own-role")
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
        assert "access" not in foreign_explicit
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
            ({"role": "ordinary duty"}, "Pass provider"),
            ({"role": "correctness-reviewer"}, "not configured"),
            ({"seat": "worker-easy"}, "not enabled"),
        ):
            result = await client.call_tool(
                "start_agent", {"prompt": "ok", "cwd": str(tmp_path), **arguments}
            )
            assert result.is_error and expected in str(result.content)
        for content, expected in (
            ("broken", "Invalid orchestration.json"),
            (json.dumps({"version": 1, "preset": "off", "worker": {"easy": seat}}), "not enabled"),
            (json.dumps({"version": 1, "preset": "split"}), "not configured"),
        ):
            path.write_text(content)
            result = await client.call_tool(
                "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "worker-easy"}
            )
            assert result.is_error and expected in str(result.content)
        path.write_text(json.dumps({"version": 1, "preset": "split", "worker": {"easy": seat}}))
        missing_cwd = await client.call_tool("start_agent", {"prompt": "ok", "seat": "worker-easy"})
        assert missing_cwd.is_error and "cwd is required" in str(missing_cwd.content)
        monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
        native = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "worker-easy"}
        )
        assert native.is_error and "native" in str(native.content)
        assert "built-in subagent" in str(native.content)


@pytest.mark.asyncio
async def test_image_generator_seat_defaults_access_resume_and_native(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.host import Host

    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "orchestration.json"
    seat = {"provider": "claude", "model": "chosen", "effort": "high"}
    document: dict[str, object] = {
        "version": 1,
        "preset": "split",
        "enabled_roles": [],
        "worker": {"easy": seat, "hard": seat},
    }
    path.write_text(json.dumps(document))
    async with Client(mcp) as client:
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        assert "image_generation" not in _payload(listed["orchestration"])
        catalog = str(await client.list_tools()) + str(client.instructions)
        assert "image-generator" not in catalog and "image_generation" not in catalog
        missing = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "image-generator"}
        )
        assert missing.is_error and "not configured" in str(missing.content)

        document["image_generation"] = seat
        path.write_text(json.dumps(document))
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        assert _payload(listed["orchestration"])["image_generation"] == {**seat, "fallbacks": []}
        assert "worker" not in _payload(listed["orchestration"])
        assert "worker-easy" not in str(await client.list_tools())
        assert "image-generator" in str(await client.list_tools())
        started = await _start_and_wait(
            client, cwd=str(tmp_path), seat="image-generator", role="生成插图"
        )
        assert started["provider"] == "claude" and started["model"] == "chosen"
        assert started["effort"] == "high" and started["access"] == "write"
        assert started["defaults_applied"] == [
            "image-generator seat",
            "model",
            "effort",
            "permission_mode",
            "permission_fallback",
            "provider (image-generator seat)",
            "access (image-generator seat)",
        ]
        resumed = await _start_and_wait(client, session_id="session-e2e")
        assert resumed["conversation_id"] == started["conversation_id"]
        assert resumed["model"] == "chosen" and resumed["access"] == "write"
        assert "seat (session_id history)" in cast(list[str], resumed["defaults_applied"])
        explicit = await _start_and_wait(
            client,
            cwd=str(tmp_path),
            seat="image-generator",
            provider="claude",
            provider_options={"model": "explicit", "effort": "medium"},
            access="write",
        )
        assert explicit["model"] == "explicit" and explicit["effort"] == "medium"
        assert explicit["defaults_applied"] == ["permission_mode", "permission_fallback"]
        other_model = await _start_and_wait(
            client,
            cwd=str(tmp_path),
            seat="image-generator",
            provider_options={"model": "explicit"},
        )
        assert other_model["model"] == "explicit" and "effort" not in other_model
        contradiction = await client.call_tool(
            "start_agent",
            {"prompt": "ok", "cwd": str(tmp_path), "seat": "image-generator", "access": "read"},
        )
        assert contradiction.is_error and "declares access=write" in str(contradiction.content)

        document["image_generation"] = {"provider": "codex", "model": "other-cli"}
        path.write_text(json.dumps(document))
        replacement = await _start_and_wait(
            client, cwd=str(tmp_path), seat="image-generator", provider="claude"
        )
        assert "model" not in replacement and replacement["access"] == "write"
        assert "image-generator seat" not in cast(list[str], replacement["defaults_applied"])

        document["image_generation"] = seat
        document["preset"] = "off"
        path.write_text(json.dumps(document))
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        assert listed["orchestration"] == {"enabled": False}
        assert "image-generator" not in str(await client.list_tools())
        disabled = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "image-generator"}
        )
        assert disabled.is_error and "not enabled" in str(disabled.content)
        bypass = await _start_and_wait(
            client, cwd=str(tmp_path), seat="image-generator", provider="claude"
        )
        assert "model" not in bypass and "effort" not in bypass

        document["preset"] = "split"
        path.write_text(json.dumps(document))
        monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
        monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        assert "image_generation" not in _payload(listed["orchestration"])
        assert "image-generator" not in str(await client.list_tools())
        native = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "image-generator"}
        )
        assert native.is_error and "not configured" in str(native.content)


@pytest.mark.asyncio
async def test_mcp_blocker_interrupts_default_wait_with_bounded_public_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    # Later dispatches must use a healthy CLI once Claude reports an auth blocker.
    executable = "codebuddy.cmd" if os.name == "nt" else "codebuddy"
    source = tmp_path / ("claude.cmd" if os.name == "nt" else "claude")
    healthy_cli = tmp_path / executable
    healthy_cli.write_bytes(source.read_bytes())
    healthy_cli.chmod(0o755)
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
                    {"provider": "codebuddy", "prompt": "hi", "cwd": str(tmp_path)},
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
        monkeypatch.setattr("agentnave.mcp_runtime.WAIT_SECONDS", 0.4)
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
async def test_mcp_lists_lifecycle_and_discovery_tools_with_structured_contracts(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
    async with Client(mcp) as client:
        result = await client.list_tools()

    assert [tool.name for tool in result.tools] == EXPECTED_TOOLS
    names = json.loads(Path(mcp_runtime.__file__).with_name("tool_names.json").read_text())
    assert names == sorted(tool.name for tool in result.tools)
    assert all(tool.output_schema is not None for tool in result.tools)
    tools = {tool.name: tool for tool in result.tools}
    start = tools["start_agent"].annotations
    assert start is not None and start.open_world_hint is True and start.destructive_hint is True
    start_properties = _payload(tools["start_agent"].input_schema["properties"])
    assert "timeout_seconds" not in start_properties
    assert tools["start_agent"].input_schema["required"] == ["prompt"]
    wait_properties = _payload(tools["wait"].input_schema["properties"])
    assert set(wait_properties) == {
        "ids",
        "include_output",
        "wait_seconds",
        "manager_session_id",
        "seq",
    }
    assert mcp_server.WAIT_SECONDS == 270
    wait_modes = cast(list[dict[str, object]], _payload(wait_properties["wait_seconds"])["anyOf"])
    assert wait_modes[0]["enum"] == [0, 270, 1500]
    description = str(wait_properties["wait_seconds"])
    assert "1500s" in description and "Claude Code" in description
    assert "terminal interactive session" in description
    assert tools["answer_prompt"].input_schema["required"] == [
        "invocation_id",
        "prompt_id",
        "manager_session_id",
    ]
    assert tools["answer_prompt"].annotations is not None
    cancel = tools["cancel"].annotations
    assert cancel is not None and cancel.destructive_hint is True
    for name in ("wait", "list", "list_members", "read_conversation"):
        annotations = tools[name].annotations
        assert annotations is not None and annotations.read_only_hint is True


def test_provider_option_schema_offers_booleans_before_strings() -> None:
    from pydantic import TypeAdapter

    from agentnave.models import ProviderOption

    adapter: TypeAdapter[ProviderOption] = TypeAdapter(ProviderOption)
    # A host that coerces types before validating would turn false into "false" when the
    # string branch comes first, so the schema offers typeless booleans and numbers first.
    schema = adapter.json_schema()
    assert [branch["type"] for branch in schema["anyOf"]] == [
        "boolean",
        "integer",
        "number",
        "string",
    ]
    # The order must not make plain strings or numbers parse as booleans.
    assert adapter.validate_python("true") == "true"
    assert adapter.validate_python("false") == "false"
    assert adapter.validate_python(1) == 1
    assert adapter.validate_python(False) is False


def test_mcp_projection_preserves_user_data_and_array_positions() -> None:
    opaque_fields = (
        "options",
        "provider_options",
        "output",
        "text",
        "prompt",
        "note",
        "notes",
    )
    opaque: dict[str, object] = {
        "empty": "",
        "null": None,
        "list": [],
        "dict": {},
        "nested": {"null": None},
    }
    source: dict[str, object] = {
        "null": None,
        "empty": "",
        "list": [],
        "dict": {"null": None},
        "false": False,
        "zero": 0,
        "items": [None, "", [], {}, {"null": None, "kept": "中文"}],
        **{key: opaque for key in opaque_fields},
    }
    assert without_empty_fields(source) == {
        "false": False,
        "zero": 0,
        "items": [None, "", [], {}, {"kept": "中文"}],
        **{key: opaque for key in opaque_fields},
    }
    # Non-null empty body fields retain their keys; empty options and notes are omitted.
    empty_values: list[object] = [None, [], {}, ""]
    for value in empty_values:
        values = dict.fromkeys(opaque_fields, value)
        kept = {} if value is None else dict.fromkeys(("output", "text", "prompt"), value)
        assert without_empty_fields(values) == kept
    assert source["dict"] == {"null": None}


def _is_container(prop: object, defs: dict[str, object]) -> bool:
    if isinstance(prop, list):
        return any(_is_container(item, defs) for item in cast(list[object], prop))
    if not isinstance(prop, dict):
        return False
    node = cast(dict[str, object], prop)
    ref = node.get("$ref")
    if isinstance(ref, str):
        return _is_container(defs.get(ref.rsplit("/", 1)[-1]), defs)
    if node.get("type") in ("array", "object"):
        return True
    return any(_is_container(node.get(key), defs) for key in ("anyOf", "oneOf", "allOf"))


def _required_containers(
    schema: object, defs: dict[str, object], all_defs: dict[str, object]
) -> set[str]:
    """Required array/object properties that projection could drop when empty."""
    found: set[str] = set()
    if isinstance(schema, list):
        for item in cast(list[object], schema):
            found |= _required_containers(item, defs, all_defs)
        return found
    if not isinstance(schema, dict):
        return found
    node = cast(dict[str, object], schema)
    ref = node.get("$ref")
    if isinstance(ref, str):
        return _required_containers(defs.pop(ref.rsplit("/", 1)[-1], None), defs, all_defs)
    properties = cast(dict[str, dict[str, object]], node.get("properties") or {})
    for name in cast(list[str], node.get("required") or []):
        prop = properties.get(name, {})
        if _is_container(prop, all_defs) and "minItems" not in prop and "minProperties" not in prop:
            found.add(name)
    for value in node.values():
        found |= _required_containers(value, defs, all_defs)
    return found


@pytest.mark.asyncio
async def test_every_required_reply_container_survives_projection() -> None:
    missing: dict[str, set[str]] = {}
    for tool in await mcp.list_tools():
        schema = cast(dict[str, object], tool.output_schema or {})
        all_defs = cast(dict[str, object], schema.get("$defs") or {})
        names = {
            name
            for name in _required_containers(schema, dict(all_defs), all_defs)
            if without_empty_fields({name: []}) != {name: []}
        }
        if names:
            missing[tool.name] = names
    assert missing == {}


@pytest.mark.asyncio
async def test_mcp_projection_leaves_error_replies_unchanged(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    error = CallToolResult(
        is_error=True,
        content=[TextContent(type="text", text="original error")],
        structured_content={"null": None, "empty": [], "dict": {}},
    )

    async def failed(*_args: object, **_kwargs: object) -> CallToolResult:
        return error

    monkeypatch.setattr(MCPServer, "call_tool", failed)
    assert await mcp.call_tool("list_members", {}) is error


@pytest.mark.asyncio
async def test_mcp_empty_replies_keep_required_containers() -> None:
    async with Client(mcp) as client:
        for name, args, containers in (
            ("list", {}, ("invocations", "conversations", "unavailable")),
            ("list_invocations", {}, ("invocations",)),
            ("wait", {}, ("pending_ids", "updates", "rounds")),
            ("handover", {}, ("waiting",)),
            ("isolated_dirs", {"action": "list", "manager_session_id": "empty-chat"}, ("items",)),
        ):
            result = await client.call_tool(name, args)
            assert not result.is_error, result.content
            payload = _payload(result.structured_content)
            assert all(payload[key] == [] for key in containers)


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
    monkeypatch.setattr("agentnave.mcp_runtime.WAIT_SECONDS", 0.05)

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
    assert "effort" not in started_payload and "elapsed_ms" not in started_payload
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
                    "wait", {"ids": [started_payload["round_id"]], "wait_seconds": 270}
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
    monkeypatch.setattr("agentnave.mcp_runtime.WAIT_SECONDS", 0.05)
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
        result = await client.call_tool("wait", {"ids": ["missing"], "wait_seconds": 45})
        assert result.is_error is True
        assert "0" in str(result.content) and "270" in str(result.content)
        assert "Unknown or expired id" not in str(result.content)


@pytest.mark.asyncio
async def test_long_wait_is_restricted_to_claude_host(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
    async with Client(mcp) as client:
        for host in (Host(), Host("codex", "env"), Host("claude", "env")):
            monkeypatch.setattr(mcp_runtime, "HOST", host)
            result = await client.call_tool("wait", {"ids": ["missing"], "wait_seconds": 1500})
            assert result.is_error is True
            if host.name == "claude":
                assert "Unknown or expired id" in str(result.content)
            else:
                assert "only available in the Claude Code terminal interactive session" in str(
                    result.content
                )
        # The desktop app inherits the Claude host name but cannot background long calls.
        monkeypatch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
        monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")
        tools = {tool.name: tool for tool in (await client.list_tools()).tools}
        wait_seconds = _payload(tools["wait"].input_schema["properties"])["wait_seconds"]
        modes = cast(list[dict[str, object]], _payload(wait_seconds)["anyOf"])
        assert modes[0]["enum"] == [0, 270]
        assert "1500" not in str(wait_seconds)
        rejected = await client.call_tool("wait", {"ids": ["missing"], "wait_seconds": 1500})
        assert rejected.is_error is True
        assert "only available in the Claude Code terminal interactive session" in str(
            rejected.content
        )


@pytest.mark.asyncio
async def test_wait_window_and_sequences_preserve_inbox_delivery(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    windows: list[float] = []

    async def elapsed(*args: object) -> tuple[dict[str, object], dict[str, object]]:
        windows.append(cast(float, args[-1]))
        return {}, {}

    async with Client(mcp) as client:
        started = _payload(
            (
                await client.call_tool(
                    "start_agent", {"provider": "claude", "prompt": "sleep", "cwd": str(tmp_path)}
                )
            ).structured_content
        )
        with monkeypatch.context() as patch:
            patch.setattr("agentnave.mcp_tasks._ready_handles", elapsed)
            patch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
            patch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
            for arguments in (
                {"seq": 1},
                {"seq": 2, "wait_seconds": 270},
                {"seq": 3, "wait_seconds": 1500},
            ):
                reply = _payload(
                    (
                        await client.call_tool(
                            "wait", {"ids": [started["invocation_id"]], **arguments}
                        )
                    ).structured_content
                )
                assert reply["reason"] == "wait_elapsed" and reply["seq"] == arguments["seq"]
                assert reply["pending_ids"] == [started["invocation_id"]]
                assert f"seq={int(arguments['seq']) + 1}" in str(reply["next"])
            # Omitting keeps 270 elsewhere, including the Claude desktop app.
            for seq, host, entrypoint in (
                (4, Host("codex", "env"), "cli"),
                (5, Host("claude", "env"), "claude-desktop"),
            ):
                patch.setattr(mcp_runtime, "HOST", host)
                patch.setenv("CLAUDE_CODE_ENTRYPOINT", entrypoint)
                reply = _payload(
                    (
                        await client.call_tool(
                            "wait", {"ids": [started["invocation_id"]], "seq": seq}
                        )
                    ).structured_content
                )
                assert reply["reason"] == "wait_elapsed" and reply["seq"] == seq
        assert len(windows) == 5
        assert 1499 < windows[0] <= 1500
        assert 269 < windows[1] <= 270
        assert 1499 < windows[2] <= 1500
        assert all(269 < window <= 270 for window in windows[3:])
        # Reusing seq never replays a response: a later terminal result is still received once.
        await client.call_tool("cancel", {"id": started["invocation_id"]})
        ended = _payload(
            (
                await client.call_tool(
                    "start_agent", {"provider": "claude", "prompt": "ok", "cwd": str(tmp_path)}
                )
            ).structured_content
        )
        reply = _payload((await client.call_tool("wait", {"seq": 2})).structured_content)
        assert reply["reason"] == "ready" and reply["seq"] == 2
        assert (
            _payload(cast(list[object], reply["updates"])[0])["invocation_id"]
            == ended["invocation_id"]
        )
        empty = _payload((await client.call_tool("wait", {"seq": 3})).structured_content)
        assert empty["reason"] == "nothing_pending" and empty["seq"] == 3


@pytest.mark.asyncio
async def test_dispatch_wait_keeps_short_window_in_long_wait_session(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    windows: list[float] = []

    async def elapsed(*args: object) -> tuple[dict[str, object], dict[str, object]]:
        windows.append(cast(float, args[-1]))
        return {}, {}

    async with Client(mcp) as client:
        with monkeypatch.context() as patch:
            patch.setattr("agentnave.mcp_tasks._ready_handles", elapsed)
            patch.setattr(mcp_runtime, "HOST", Host("claude", "env"))
            patch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
            started = _payload(
                (
                    await client.call_tool(
                        "start_agent",
                        {
                            "provider": "claude",
                            "prompt": "sleep",
                            "cwd": str(tmp_path),
                            "wait": True,
                        },
                    )
                ).structured_content
            )
        assert len(windows) == 1 and 269 < windows[0] <= 270
        assert _payload(started["wait"])["reason"] == "wait_elapsed"
        await client.call_tool("cancel", {"id": started["invocation_id"]})


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
        monkeypatch.setattr("agentnave.mcp_runtime.WAIT_SECONDS", 0.2)
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
@pytest.mark.parametrize("title", ["汉" * 25, "🚀" * 25, "a" * 49])
async def test_mcp_rejects_titles_wider_than_one_short_line(tmp_path: Path, title: str) -> None:
    async with Client(mcp) as client:
        task = await client.call_tool(
            "start_agent",
            {"provider": "codex", "prompt": "x", "cwd": str(tmp_path), "title": title},
        )
        show = await client.call_tool("run_round", {"prompt": "x", "title": title})
        one_seat = await client.call_tool(
            "run_round",
            {"title": "ok", "members": [{"id": "a", "provider": "claude"}]},
        )

    for result in (task, show):
        assert result.is_error is True
        text = cast(TextContent, result.content[0]).text
        assert "within 48" in text
        assert all(noise not in text for noise in ("validation error", "input_value", "http", "\n"))
    assert one_seat.is_error is True
    seat_text = cast(TextContent, one_seat.content[0]).text
    assert "members" in seat_text and "at least 2" in seat_text and "got 1" in seat_text
    assert all(noise not in seat_text for noise in ("validation error", "input_value", "http"))


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
        ("codex", {"approve_for_me": "true"}, "must be a boolean"),
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
    [" CODEX, codex, ", "antigravity,claude,codebuddy,codex,deepseek,devin,grok,pi,zcode"],
)
async def test_stdio_enforces_host_exclusions_before_launch(
    excluded: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    # The server runs in its own process, outside the suite's print-only Claude fixture.
    (tmp_path / "data").mkdir()
    (tmp_path / "data" / "settings.json").write_text('{"version": 1, "claude_interactive": false}')
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
        assert "manager_session_id" in client.instructions
        assert "Excluded providers:" in client.instructions
        assert "MCP instructions" in description
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
            assert "Providers permitted by this host configuration: none" in client.instructions
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
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"codex"}))
    monkeypatch.setattr(mcp_runtime, "HOST", Host(name="claude", source="env"))

    def no_prepare(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("list_members must not prepare CLI commands")

    monkeypatch.setattr(GrokAdapter, "prepare", no_prepare)
    monkeypatch.setattr(AntigravityAdapter, "prepare", no_prepare)
    async with Client(mcp) as client:
        missing = await client.call_tool("list_members", {})
        assert not missing.is_error
        missing_payload = _payload(missing.structured_content)
        assert missing_payload["configured"] is False
        assert missing_payload["members"] == []
        assert "restricted" not in missing_payload and "modes" not in missing_payload
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
        clis["codex"]["models"]["bad-type"] = {"options": {"approve_for_me": "true"}}
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
        assert "review" not in broken and "review.json" in cast(str, broken["review_error"])
        (root / "review.json").write_text(json.dumps({"version": 1, "seats": seats}))
        listed = await client.call_tool("list_members", {})
        review = cast(dict[str, dict[str, object]], _payload(listed.structured_content)["review"])
        assert "available" not in review["correctness-reviewer"]
        assert review["correctness-reviewer"]["effort"] == "high"
        assert "native" not in review["correctness-reviewer"]
        assert review["security-reviewer"]["available"] is False
        assert _payload(review["security-reviewer"]["replacement"])["provider"] == "claude"
        assert review["security-reviewer"]["reason"] == "CLI is excluded by this host"
        # The host's own CLI goes to its built-in subagent rather than being unavailable.
        assert "available" not in review["simplicity-reviewer"]
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
        assert "available" not in review["performance-reviewer"]
        assert "alternatives" not in review["performance-reviewer"]
        payload = _payload(listed.structured_content)
        assert "modes" not in payload and payload["restricted"] == ["task"]
        items = cast(list[dict[str, object]], payload["members"])
        valid = [item for item in items if item["model"] == "chosen"]
        assert len(valid) == 6
        assert all("valid" not in item and "error" not in item for item in valid)
        assert all(item["modes"] == ["discussion"] for item in valid)
        invalid = [item for item in items if str(item["model"]).startswith("bad-")]
        assert len(invalid) == 3 and all("valid" not in item and item["error"] for item in invalid)
        assert [(item["provider"], item["default"]) for item in items if "default" in item] == [
            ("codex", ["discussion"])
        ]
        assert path.read_text(encoding="utf-8") == content
        assert sorted(root.iterdir()) == [path, root / "orchestration.json", root / "review.json"]

        def installed(adapter: AdapterHooks) -> bool:
            return adapter.name != "claude"

        monkeypatch.setattr(AdapterHooks, "is_installed", installed)
        uninstalled = _payload((await client.call_tool("list_members", {})).structured_content)
        claude_member = next(
            item
            for item in cast(list[dict[str, object]], uninstalled["members"])
            if item["provider"] == "claude"
        )
        assert claude_member["error"] == "CLI is not installed"
        native_review = _payload(_payload(uninstalled["review"])["simplicity-reviewer"])
        assert native_review["native"] is True and "available" not in native_review
        replacement = _payload(
            _payload(_payload(uninstalled["review"])["security-reviewer"])["replacement"]
        )
        assert replacement["provider"] == "codebuddy"
        path.write_text('{"version": 1, "clis": {}, "modes": {"discussion": {}}}', encoding="utf-8")
        fresh = _payload((await client.call_tool("list_members", {})).structured_content)
        assert fresh["configured"] is True
        assert fresh["members"] == [] and fresh["restricted"] == ["task", "discussion"]
        # The App file currently accepts an empty model key; its reply must still validate.
        path.write_text(
            json.dumps({"version": 1, "clis": {"claude": {"models": {"": {}}}}, "modes": {}})
        )
        empty_model = _payload((await client.call_tool("list_members", {})).structured_content)
        member = cast(list[dict[str, object]], empty_model["members"])[0]
        assert not {"model", "options", "notes"}.intersection(member)
        assert "cli_options" not in empty_model


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

        monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        assert listed["restricted"] == ["task", "discussion"]
        members = cast(list[dict[str, object]], listed["members"])
        task_member, discussion_member, wildcard = members
        assert task_member == {
            "provider": "claude",
            "model": "chosen",
            "default": ["task"],
            "effort": "medium",
            "efforts": ["medium", "high"],
            "options": {"agent": "reviewer"},
            "notes": "任务",
            "modes": [],
        }
        assert discussion_member["effort"] == "high"
        assert discussion_member["default"] == ["discussion"]
        assert discussion_member["modes"] == ["discussion"]
        assert wildcard == {
            "provider": "claude",
            "model": "*",
            "notes": "不限模型",
            "modes": ["discussion"],
        }
        monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset[str]())
        defaulted = await start({})
        assert defaulted["model"] == "chosen" and defaulted["effort"] == "medium"
        # Without permissions.json every CLI with a tier runs it at "auto".
        applied = ["model", "effort", "agent", "permission_mode", "permission_fallback"]
        assert defaulted["defaults_applied"] == applied
        resumed = await start({}, "session-e2e")
        # A resumed thread keeps its own model and effort; the App still fills the rest.
        assert resumed["model"] == "chosen" and resumed["effort"] == "medium"
        assert resumed["defaults_applied"] == [
            *applied[2:],
            "model (session_id history)",
            "effort (session_id history)",
        ]
        explicit = await start({"model": "chosen", "effort": "high", "agent": "explicit"})
        assert (
            explicit["defaults_applied"] == ["permission_mode", "permission_fallback"]
            and explicit["effort"] == "high"
        )
        # Old CLI-wide settings are ignored even if their file is malformed.
        (tmp_path / "cli-options.json").write_text("{")
        wide = await start({"model": "chosen"})
        assert wide["defaults_applied"] == [
            "effort",
            "agent",
            "permission_mode",
            "permission_fallback",
        ]
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
            "permission_mode",
            "permission_fallback",
            "access (correctness-reviewer seat)",
            "read-only instruction (access=read)",
        ]
        assert seated["access"] == "read"
        # An Explorer/Worker name in role is only a label, even with a configured seat.
        missing_model = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "ok",
                "cwd": str(tmp_path),
                "role": "worker-easy",
            },
        )
        assert missing_model.is_error and "Model is required" in str(missing_model.content)
        labelled = await start({"model": "chosen"}, role="worker-easy")
        assert labelled["defaults_applied"] == ["effort", "permission_mode", "permission_fallback"]
        assert labelled["model"] == "chosen" and "access" not in labelled
        label_only = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "role": "worker-easy"}
        )
        assert label_only.is_error and "Pass provider" in str(label_only.content)
        worker = await start({"effort": "medium"}, seat="worker-easy")
        assert worker["effort"] == "medium"
        assert worker["defaults_applied"] == [
            "worker-easy seat",
            "model",
            "permission_mode",
            "permission_fallback",
            "access (worker-easy seat)",
        ]
        assert worker["access"] == "write"
        assert (await start({"model": "chosen"}, seat="explorer-easy"))["defaults_applied"] == [
            "effort",
            "permission_mode",
            "permission_fallback",
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

        monkeypatch.setattr(mcp_runtime, "HOST", Host("grok", "env"))
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
        assert disabled["orchestration"] == {"enabled": False}
        for name in ("worker-easy", "explorer-hard"):
            rejected = await client.call_tool(
                "start_agent",
                {"prompt": "ok", "cwd": str(tmp_path), "provider": "claude", "seat": name},
            )
            assert rejected.is_error and "not enabled" in str(rejected.content)
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
        monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"claude"}))
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
        assert seats[0]["defaults_applied"] == [
            "model",
            "effort",
            "permission_mode",
            "permission_fallback",
        ]
        assert seats[1]["model"] == "outside" and "effort" not in seats[1]
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


@pytest.mark.asyncio
async def test_cooldown_rejects_task_and_new_meeting_before_saving(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health
    from agentnave.db import Database

    root = tmp_path / "data"
    provider_health.record(root, "claude", "auth")
    async with Client(mcp) as client:
        for name, arguments in (
            ("start_agent", {"provider": "claude", "prompt": "ok", "cwd": str(tmp_path)}),
            (
                "run_round",
                {
                    "title": "Blocked",
                    "cwd": str(tmp_path),
                    "prompt": "work",
                    "seats": ["a", "b"],
                    "members": [{"id": s, "provider": "claude"} for s in "ab"],
                },
            ),
        ):
            result = await client.call_tool(name, arguments)
            assert result.is_error and "auth" in str(result.content)
            assert "replacement" in str(result.content) and "until" in str(result.content)
        assert Database(root).revisions() == {}
        # Creating a room without starting seats remains possible during cooldown.
        room = await client.call_tool(
            "run_round",
            {
                "title": "Idle",
                "cwd": str(tmp_path),
                "members": [{"id": s, "provider": "claude"} for s in "ab"],
            },
        )
        assert not room.is_error
        group_id = str(_payload(room.structured_content)["group_id"])
        before, _ = Database(root).load(group_id)
        rejected = await client.call_tool(
            "run_round",
            {
                "group_id": group_id,
                "prompt": "work",
                "seats": ["a"],
                "forward": ["missing:e:0"],
            },
        )
        assert rejected.is_error and "cooling down" in str(rejected.content)
        after, _ = Database(root).load(group_id)
        assert after.entries == before.entries and after.rounds == before.rounds
        settled_retry = await client.call_tool(
            "run_round",
            {"group_id": group_id, "retry_seats": ["a"]},
        )
        assert settled_retry.is_error and "only a running parallel round" in str(
            settled_retry.content
        )


@pytest.mark.asyncio
async def test_cooldown_only_blocks_selected_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health

    _install_fake_claude(tmp_path, monkeypatch)
    root = tmp_path / "data"
    provider_health.record(root, "claude", "auth", "a")
    async with Client(mcp) as client:
        args = {
            "provider": "claude",
            "prompt": "finish",
            "cwd": str(tmp_path),
            "provider_options": {"model": "a"},
        }
        blocked = await client.call_tool("start_agent", args)
        assert blocked.is_error and "model a" in str(blocked.content)
        args["provider_options"] = {"model": "b"}
        started = await client.call_tool("start_agent", args)
        assert not started.is_error
        assert ("claude", "a") in provider_health.active(root)
        cleared = await client.call_tool(
            "clear_provider_cooldown", {"provider": "claude", "model": "a"}
        )
        assert not cleared.is_error and provider_health.active(root) == {}


@pytest.mark.asyncio
async def test_unmodelled_failure_blocks_followup_even_when_cli_reports_model(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health

    _install_fake_claude(tmp_path, monkeypatch)
    executable = tmp_path / ("claude.py" if os.name == "nt" else "claude")
    executable.write_text(
        f"#!{sys.executable}\n"
        "import json\n"
        "print(json.dumps({'type': 'assistant', 'message': {'model': 'reported', 'content': []}}))\n"
        "print(json.dumps({'type': 'result', 'is_error': True, 'errors': ['not logged in']}))\n"
    )
    executable.chmod(0o755)
    root = tmp_path / "data"
    args = {"provider": "claude", "prompt": "work", "cwd": str(tmp_path)}
    async with Client(mcp) as client:
        started = await client.call_tool("start_agent", args)
        assert not started.is_error
        invocation_id = _payload(started.structured_content)["invocation_id"]
        await client.call_tool("wait", {"ids": [invocation_id]})
        assert set(provider_health.active(root)) == {("claude", None)}
        blocked = await client.call_tool("start_agent", args)
        assert blocked.is_error and "cooling down" in str(blocked.content)
        cleared = await client.call_tool(
            "clear_provider_cooldown", {"provider": "claude", "model": "reported"}
        )
        assert not cleared.is_error and provider_health.active(root) == {}


def _init_git_repo(repo: Path) -> str:
    repo.mkdir(parents=True, exist_ok=True)
    git = [
        "git",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.com",
    ]
    subprocess.run([*git, "init", "-b", "main"], cwd=repo, check=True, capture_output=True)
    hook = repo / ".git" / "hooks" / "post-checkout"
    hook.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
    hook.chmod(0o755)
    (repo / ".gitignore").write_text("*.ignored\n", encoding="utf-8")
    (repo / ".gitattributes").write_text(
        "*.bin filter=lfs diff=lfs merge=lfs -text\n", encoding="utf-8"
    )
    sub = repo / "packages" / "app"
    sub.mkdir(parents=True)
    (sub / "index.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run([*git, "add", "."], cwd=repo, check=True, capture_output=True)
    subprocess.run([*git, "commit", "-m", "initial"], cwd=repo, check=True, capture_output=True)
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


@pytest.mark.asyncio
async def test_mcp_isolated_task_lifecycle_receipts_and_release_flow(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path / "data"))
    base_oid = _init_git_repo(tmp_path / "repo")
    repo = tmp_path / "repo"
    subdir = repo / "packages" / "app"
    (subdir / "dirty.txt").write_text("local draft\n", encoding="utf-8")

    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    write_fake_cli(
        bin_dir,
        "claude",
        "import json, os, pathlib, sys, time\n"
        "prompt = sys.stdin.read()\n"
        "if 'HOLD_OPEN' in prompt:\n"
        "    time.sleep(30)\n"
        "if 'WRITE_FILES' in prompt:\n"
        "    pathlib.Path('created.py').write_text('y = 2\\n', encoding='utf-8')\n"
        "    pathlib.Path('cache.ignored').write_text('tmp\\n', encoding='utf-8')\n"
        "print(json.dumps({'type': 'result', 'result': json.dumps({'cwd': os.getcwd(), "
        "'prompt': prompt}), 'session_id': 'iso-sess', 'num_turns': 1, 'is_error': False}))\n",
    )
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    async with Client(mcp) as client:
        plain = await _start_and_wait(
            client, provider="claude", cwd=str(subdir), manager_session_id="chat-1"
        )
        assert "isolation" not in plain and "unreleased_isolation_count" not in plain

        both = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "reject",
                "cwd": str(subdir),
                "session_id": "iso-sess",
                "isolation": {},
                "manager_session_id": "chat-1",
            },
        )
        assert both.is_error and "cannot be combined" in str(both.content)
        assert not (repo / ".agentnave-worktrees").exists()

        running = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "HOLD_OPEN",
                        "cwd": str(subdir),
                        "isolation": {"ref": "main"},
                        "manager_session_id": "chat-1",
                    },
                )
            ).structured_content
        )
        iso = _payload(running["isolation"])
        wt_id = str(iso["id"])
        wt_path = Path(str(iso["path"]))
        assert iso["cwd"] == str((wt_path / "packages" / "app").resolve())
        assert iso["branch"] == f"agentnave/{wt_id}" and iso["base_oid"] == base_oid
        assert running["unreleased_isolation_count"] == 1
        assert "isolated directory instructions" in cast(list[str], running["defaults_applied"])
        warnings_text = " ".join(cast(list[str], running["warnings"]))
        assert "Git hooks and LFS" in warnings_text and "Uncommitted changes" in warnings_text

        listed_busy = _payload(
            (
                await client.call_tool(
                    "isolated_dirs", {"action": "list", "manager_session_id": "chat-1"}
                )
            ).structured_content
        )
        items = cast(list[dict[str, object]], listed_busy["items"])
        assert len(items) == 1 and items[0]["id"] == wt_id and items[0]["in_use"] is True
        blocked = _payload(
            (
                await client.call_tool(
                    "isolated_dirs",
                    {"action": "release", "id": wt_id, "manager_session_id": "chat-1"},
                )
            ).structured_content
        )
        assert blocked["status"] == "blocked"

        cancelled = _payload(
            _payload(
                (
                    await client.call_tool("cancel", {"id": running["invocation_id"]})
                ).structured_content
            )["invocation"]
        )
        cancel_iso = _payload(cancelled["isolation"])
        assert (
            cancel_iso["summary_status"] == "complete"
            and cancel_iso["changed_files_from_base"] == []
            and cancel_iso["uncommitted"] is False
        )

        # Claim existing isolated cwd without passing isolation; write dirty + ignored files.
        second = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "WRITE_FILES original body",
                        "cwd": str(iso["cwd"]),
                        "manager_session_id": "chat-1",
                    },
                )
            ).structured_content
        )
        assert _payload(second["isolation"])["id"] == wt_id
        assert second["unreleased_isolation_count"] == 1
        done = _first(await client.call_tool("wait", {"ids": [second["invocation_id"]]}))
        seen = json.loads(str(done["output"]))
        assert Path(seen["cwd"]).resolve() == Path(str(iso["cwd"])).resolve()
        assert (
            "When the task asks you to change a file under the main worktree, change the "
            "corresponding path under this isolated worktree instead."
        ) in seen["prompt"]
        done_iso = _payload(done["isolation"])
        assert done_iso["summary_status"] == "complete" and done_iso["uncommitted"] is True

        saved = _payload(
            (
                await client.call_tool(
                    "read_conversation", {"conversation_id": second["conversation_id"]}
                )
            ).structured_content
        )
        saved_task = cast(list[dict[str, object]], saved["tasks"])[-1]
        assert saved_task["prompt"] == "WRITE_FILES original body"
        assert saved_task["isolation_id"] == wt_id
        assert saved_task["isolation_path"] == str(wt_path)
        assert saved_task["isolation_branch"] == f"agentnave/{wt_id}"

        rel = _payload(
            (
                await client.call_tool(
                    "handover",
                    {"release": True, "note": "check wt", "manager_session_id": "chat-1"},
                )
            ).structured_content
        )
        claimed = _payload(
            (
                await client.call_tool(
                    "handover",
                    {"workspace_id": rel["workspace_id"], "manager_session_id": "chat-2"},
                )
            ).structured_content
        )
        claimed_dirs = cast(list[dict[str, object]], claimed["isolated_dirs"])
        assert [d["id"] for d in claimed_dirs] == [wt_id]

        bad_list = await client.call_tool(
            "isolated_dirs", {"action": "list", "id": wt_id, "manager_session_id": "chat-2"}
        )
        assert bad_list.is_error
        bad_rel = await client.call_tool(
            "isolated_dirs", {"action": "release", "manager_session_id": "chat-2"}
        )
        assert bad_rel.is_error

        confirm_req = _payload(
            (
                await client.call_tool(
                    "isolated_dirs",
                    {"action": "release", "id": wt_id, "manager_session_id": "chat-2"},
                )
            ).structured_content
        )
        assert confirm_req["status"] == "confirmation_required"
        assert confirm_req["uncommitted"] is True and confirm_req["confirmation"]
        assert (wt_path / "packages" / "app" / "cache.ignored").is_file()

        deleted = _payload(
            (
                await client.call_tool(
                    "isolated_dirs",
                    {
                        "action": "release",
                        "id": wt_id,
                        "confirmation": confirm_req["confirmation"],
                        "manager_session_id": "chat-2",
                    },
                )
            ).structured_content
        )
        assert deleted["status"] == "deleted" and not wt_path.exists()
        after = _payload(
            (
                await client.call_tool(
                    "isolated_dirs", {"action": "list", "manager_session_id": "chat-2"}
                )
            ).structured_content
        )
        assert after["items"] == []


@pytest.mark.asyncio
async def test_mcp_isolated_dirs_lists_a_repo_and_releases_other_workspaces_with_a_token(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _init_git_repo(tmp_path / "repo")
    repo = tmp_path / "repo"
    _install_fake_claude(tmp_path, monkeypatch)

    async with Client(mcp) as client:
        started = _payload(
            (
                await client.call_tool(
                    "start_agent",
                    {
                        "provider": "claude",
                        "prompt": "ok",
                        "cwd": str(repo),
                        "isolation": {},
                        "manager_session_id": "chat-old",
                    },
                )
            ).structured_content
        )
        wt_id = str(_payload(started["isolation"])["id"])
        await client.call_tool("wait", {"ids": [started["invocation_id"]]})

        own = _payload(
            (
                await client.call_tool(
                    "isolated_dirs", {"action": "list", "manager_session_id": "chat-new"}
                )
            ).structured_content
        )
        assert own["items"] == []
        everywhere = _payload(
            (
                await client.call_tool(
                    "isolated_dirs",
                    {"action": "list", "cwd": str(repo), "manager_session_id": "chat-new"},
                )
            ).structured_content
        )
        items = cast(list[dict[str, object]], everywhere["items"])
        assert [(item["id"], item["own_workspace"]) for item in items] == [(wt_id, False)]

        relative = await client.call_tool(
            "isolated_dirs",
            {"action": "list", "cwd": "repo", "manager_session_id": "chat-new"},
        )
        assert relative.is_error
        release_with_cwd = await client.call_tool(
            "isolated_dirs",
            {"action": "release", "id": wt_id, "cwd": str(repo), "manager_session_id": "chat-new"},
        )
        assert release_with_cwd.is_error

        first = _payload(
            (
                await client.call_tool(
                    "isolated_dirs",
                    {"action": "release", "id": wt_id, "manager_session_id": "chat-new"},
                )
            ).structured_content
        )
        assert first["status"] == "confirmation_required" and first["confirmation"]
        assert first["uncommitted"] is False
        done = _payload(
            (
                await client.call_tool(
                    "isolated_dirs",
                    {
                        "action": "release",
                        "id": wt_id,
                        "confirmation": first["confirmation"],
                        "manager_session_id": "chat-new",
                    },
                )
            ).structured_content
        )
        assert done["status"] == "deleted"


@pytest.mark.asyncio
async def test_creating_an_isolated_directory_does_not_block_the_event_loop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import time

    from agentnave.isolated import IsolatedDirs

    _init_git_repo(tmp_path / "repo")
    _install_fake_claude(tmp_path, monkeypatch)
    original = IsolatedDirs.create

    def slow_create(self: IsolatedDirs, *args: object, **kwargs: object):  # noqa: ANN202
        time.sleep(0.5)
        return original(self, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(IsolatedDirs, "create", slow_create)
    ticks = 0
    stop = asyncio.Event()

    async def ticker() -> None:
        nonlocal ticks
        while not stop.is_set():
            await asyncio.sleep(0.02)
            ticks += 1

    async with Client(mcp) as client:
        tick_task = asyncio.create_task(ticker())
        result = await client.call_tool(
            "start_agent",
            {
                "provider": "claude",
                "prompt": "ok",
                "cwd": str(tmp_path / "repo"),
                "isolation": {},
                "manager_session_id": "chat-1",
            },
        )
        stop.set()
        await tick_task
        assert not result.is_error
    assert ticks >= 10


def _own_seat(seat_id: str, provider: str, access: str, **extra: object) -> dict[str, object]:
    return {
        "id": seat_id,
        "kind": "custom",
        "role": f"role {seat_id}",
        "description": f"role {seat_id}",
        "provider": provider,
        "model": f"model-{seat_id}",
        "access": access,
        "difficulty": "medium",
        **extra,
    }


def _write_own_seats(seats: list[dict[str, object]], **extra: object) -> None:
    """The seat file, with the boss on the decision mode unless extra sets another preset."""
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    root.mkdir(parents=True, exist_ok=True)
    preset = extra.pop("preset", "decision")
    (root / "orchestration.json").write_text(json.dumps({"version": 1, "preset": preset}))
    document = {"version": 1, "seats": seats, **extra}
    (root / "decision-seats.json").write_text(json.dumps(document))


def _write_decision_key(key: str = "test-key-123") -> None:
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    (root / "decision-keys.json").write_text(json.dumps({"version": 1, "keys": {"gemini": key}}))


class _FakeDecisionClient:
    """Stands in for the decision model; records what the service would have sent."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch) -> None:
        self.result: Decision | DecisionError | None = None
        self.delay = 0.0
        self.created: list[tuple[str, str]] = []
        self.calls: list[tuple[str, dict[str, str]]] = []
        # decide_many: the answers to give, in question order, or a failure of the whole request.
        self.answers: Sequence[Decision | DecisionError] | DecisionError | None = None
        self.batches: list[list[tuple[str, dict[str, str]]]] = []
        monkeypatch.setattr(seat_recommendation, "new_client", self._create)

    def _create(self, model: str, key: str) -> _FakeDecisionClient:
        self.created.append((model, key))
        return self

    def decide(self, text: str, candidates: Mapping[str, str]) -> Decision:
        self.calls.append((text, dict(candidates)))
        time.sleep(self.delay)
        assert self.result is not None, "the decision model must not be called"
        if isinstance(self.result, DecisionError):
            raise self.result
        return self.result

    def decide_many(self, questions: Sequence[DecisionQuestion]) -> list[Decision | DecisionError]:
        self.batches.append([(one.text, dict(one.candidates)) for one in questions])
        time.sleep(self.delay)
        assert self.answers is not None, "the decision model must not be called"
        if isinstance(self.answers, DecisionError):
            raise self.answers
        assert len(self.answers) == len(questions)
        return list(self.answers)


@pytest.fixture
def decision(monkeypatch: pytest.MonkeyPatch) -> _FakeDecisionClient:
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex", "env", "codex"))
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"codex"}))
    return _FakeDecisionClient(monkeypatch)


async def _recommend(client: Client, task: str = "给设置页加一个开关", access: str = "write"):
    return await client.call_tool(
        "recommend_seat", {"task": task, "access": access, "manager_session_id": "boss-1"}
    )


@pytest.mark.asyncio
async def test_own_seats_stay_unpublished_and_recommend_refuses_while_off(
    decision: _FakeDecisionClient,
) -> None:
    _ = decision
    async with Client(mcp) as client:
        for inactive in (
            None,
            # Any mode but decision leaves the file unused.
            {"preset": "split"},
            {"preset": "parallel"},
            {"preset": "off"},
            {"preset": "meetings"},
        ):
            if inactive is not None:
                _write_own_seats([_own_seat("a", "claude", "write")], **inactive)
            tools = await client.list_tools(cache_mode="refresh")
            assert [tool.name for tool in tools.tools] == EXPECTED_TOOLS
            dumped = json.dumps([tool.model_dump() for tool in tools.tools])
            assert "CustomSeat" not in dumped and "recommendation" not in dumped
            start = next(tool for tool in tools.tools if tool.name == "start_agent")
            assert "task" not in start.input_schema["properties"]
            assert "auto" not in start.input_schema.get("$defs", {}).get(
                "OrchestrationRole", {}
            ).get("enum", [])
            args = {"prompt": "ok", "cwd": str(Path.cwd())}
            unknown = await client.call_tool("start_agent", {**args, "task": "x", "seat": "a"})
            assert unknown.is_error and "Unknown argument(s) for start_agent: task" in str(
                unknown.content
            )
            auto = await client.call_tool(
                "start_agent", {**args, "seat": "auto", "access": "write"}
            )
            assert auto.is_error  # the seat enum still decides, as before
            refused = await _recommend(client)
            assert refused.is_error and str(refused.content).count("not_enabled")
        assert decision.calls == []


@pytest.mark.asyncio
async def test_own_seats_publish_dispatch_and_recommend_end_to_end(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    seats = [
        _own_seat("write-a", "claude", "write", effort="high"),
        _own_seat("write-b", "claude", "write", difficulty="hard"),
        # A group of the role catalog: its scope text is the description.
        _own_seat(
            "write-c",
            "claude",
            "write",
            kind="worker_writing",
            role="写作",
            description="写、改、翻译给人看的文章、文案和报告。",
            difficulty="easy",
        ),
        _own_seat("read-d", "claude", "read"),
        # An image seat: write access, no difficulty, a specialty of its own.
        {
            k: v
            for k, v in _own_seat(
                "draw-comic",
                "claude",
                "write",
                kind="image",
                role="漫画风格",
                description="漫画风格",
            ).items()
            if k != "difficulty"
        },
        # The description of an image seat is sent as well, when it says more than the role.
        {
            k: v
            for k, v in _own_seat(
                "draw-real",
                "claude",
                "write",
                kind="image",
                role="写实",
                description="照片质感的写实人像",
            ).items()
            if k != "difficulty"
        },
    ]
    _write_own_seats(seats, image_generation={"provider": "grok", "model": "fixed-image"})
    _write_decision_key()
    decision.result = Decision(
        "write-b", {"write-b": 0.7, "write-a": 0.2, "write-c": 0.1}, confidence=0.82
    )
    async with Client(mcp) as client:
        tools = await client.list_tools(cache_mode="refresh")
        by_name = {tool.name: tool for tool in tools.tools}
        assert "recommend_seat" in by_name
        assert by_name["recommend_seat"].annotations is not None
        assert by_name["recommend_seat"].annotations.read_only_hint is True
        start = by_name["start_agent"].input_schema
        assert start["$defs"]["OrchestrationRole"]["enum"] == [s["id"] for s in seats] + ["auto"]
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        status = _payload(listed["orchestration"])
        assert set(_payload(status["custom_seats"])) == {s["id"] for s in seats}
        assert not {"explorer", "worker", "scopes", "image_generation"} & set(status)
        comic = _payload(_payload(status["custom_seats"])["draw-comic"])
        assert comic["kind"] == "image" and comic["access"] == "write"
        assert (
            "difficulty" not in comic
            and "image-generator" not in start["$defs"]["OrchestrationRole"]["enum"]
        )

        result = await _recommend(client)
        assert not result.is_error, result.content
        picked = _payload(result.structured_content)
        assert picked["seat"] == "write-b" and picked["confidence"] == 0.82
        assert picked["decision_model_called"] is True and "native" not in picked
        assert [c["seat"] for c in cast(list[dict[str, object]], picked["candidates"])] == [
            "write-b",
            "write-a",
            "write-c",
        ]
        assert [c["probability"] for c in cast(list[dict[str, object]], picked["candidates"])] == [
            0.7,
            0.2,
            0.1,
        ]
        # Only the summary, the access and each candidate's specialty, difficulty and access.
        assert decision.created == [("gemini-decision-1", "test-key-123")]
        (text, sent), *_ = decision.calls
        assert text == "Task:\n给设置页加一个开关\nRequired access: write"
        assert sent == {
            "write-a": "role write-a | difficulty: medium | access: write",
            "write-b": "role write-b | difficulty: hard | access: write",
            "write-c": "写作: 写、改、翻译给人看的文章、文案和报告。 | difficulty: easy | access: write",
            "draw-comic": "漫画风格 (image generation) | access: write",
            "draw-real": "写实: 照片质感的写实人像 (image generation) | access: write",
        }

        started = await _start_and_wait(client, cwd=str(tmp_path), seat="write-b", role="加开关")
        assert started["provider"] == "claude" and started["model"] == "model-write-b"
        assert started["access"] == "write"
        assert cast(list[str], started["defaults_applied"])[:2] == ["write-b seat", "model"]
        assert "provider (write-b seat)" in cast(list[str], started["defaults_applied"])
        drawn = await _start_and_wait(client, cwd=str(tmp_path), seat="draw-comic")
        assert drawn["provider"] == "claude" and drawn["model"] == "model-draw-comic"
        assert drawn["access"] == "write"
        effort = await _start_and_wait(client, cwd=str(tmp_path), seat="write-a")
        assert effort["effort"] == "high"
        resumed = await _start_and_wait(client, session_id="session-e2e")
        assert resumed["model"] == "model-write-a"
        assert "seat (session_id history)" in cast(list[str], resumed["defaults_applied"])

        for arguments, expected in (
            ({"seat": "nobody"}, "Unknown task seat: nobody"),
            ({"seat": "worker-easy"}, "not used in the decision-model mode"),
            ({"seat": "image-generator"}, "not used in the decision-model mode"),
            ({"seat": "read-d", "access": "write"}, "declares access=read"),
            ({"seat": "x" * 129}, "seat"),
        ):
            rejected = await client.call_tool(
                "start_agent", {"prompt": "ok", "cwd": str(tmp_path), **arguments}
            )
            assert rejected.is_error and expected in str(rejected.content)

        # The user deleted a seat the boss was about to use: no other seat takes its place.
        _write_own_seats([s for s in seats if s["id"] != "write-b"])
        gone = await client.call_tool(
            "start_agent", {"prompt": "ok", "cwd": str(tmp_path), "seat": "write-b"}
        )
        assert gone.is_error and "Unknown task seat: write-b" in str(gone.content)
        tools = await client.list_tools(cache_mode="refresh")
        by_name = {tool.name: tool for tool in tools.tools}
        assert (
            "write-b"
            not in by_name["start_agent"].input_schema["$defs"]["OrchestrationRole"]["enum"]
        )


@pytest.mark.asyncio
async def test_recommendation_candidates_are_only_what_can_be_dispatched(
    decision: _FakeDecisionClient, monkeypatch: pytest.MonkeyPatch
) -> None:
    from datetime import UTC, datetime, timedelta

    from agentnave import provider_health

    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    _write_own_seats(
        [
            _own_seat("claude-w", "claude", "write"),
            _own_seat("grok-w", "grok", "write"),
            _own_seat("pi-w", "pi", "write"),
            _own_seat("grok-r", "grok", "read"),
            _own_seat("codex-w", "codex", "write"),
        ]
    )
    _write_decision_key()
    async with Client(mcp) as client:
        # The host's own CLI is a candidate and marked native; a cooling CLI is not.
        provider_health.record(
            root, "claude", "quota", "model-claude-w", datetime.now(UTC) + timedelta(hours=1)
        )
        provider_health.record(root, "pi", "auth", None, None)
        decision.result = Decision("codex-w", {"codex-w": 0.9, "grok-w": 0.1}, 0.9)
        result = _payload((await _recommend(client)).structured_content)
        assert [text for _, sent in decision.calls for text in sent] == ["grok-w", "codex-w"]
        assert result["seat"] == "codex-w" and result["native"] is True
        assert cast(list[dict[str, object]], result["candidates"])[0]["native"] is True

        # Only one seat can be dispatched: it is returned and the decision model is not called.
        decision.calls.clear()
        result = _payload((await _recommend(client, access="read")).structured_content)
        assert result["seat"] == "grok-r" and result["decision_model_called"] is False
        assert "confidence" not in result and "not called" in str(result["note"])
        assert decision.calls == []

        # No seat can be dispatched: the reason comes back without a call.
        provider_health.record(root, "grok", "billing", None, None)
        refused = await _recommend(client, access="read")
        assert refused.is_error and "no_candidates" in str(refused.content)
        assert "grok-r" in str(refused.content) and "billing" in str(refused.content)
        # The host's own CLI is never cooled by the others: its native seat stays alone.
        direct = _payload((await _recommend(client)).structured_content)
        assert direct["seat"] == "codex-w" and direct["native"] is True
        assert direct["decision_model_called"] is False
        assert decision.calls == []


@pytest.mark.asyncio
async def test_recommendation_errors_are_distinguishable_and_never_retried(
    decision: _FakeDecisionClient,
) -> None:
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "grok", "write")])
    async with Client(mcp) as client:
        missing = await _recommend(client)
        assert missing.is_error and str(missing.content).count("no_key")
        for content in ("{broken", json.dumps({"version": 1, "keys": {"gemini": 12345}})):
            (root / "decision-keys.json").write_text(content)
            broken = await _recommend(client)
            assert broken.is_error and "config_error" in str(broken.content)
            assert "12345" not in str(broken.content)
        _write_decision_key("  ")
        assert "no_key" in str((await _recommend(client)).content)
        _write_decision_key()
        assert decision.calls == []
        for code in ("network", "rate_limited", "auth", "rejected", "bad_response"):
            decision.result = DecisionError(code, f"{code} detail")  # type: ignore[arg-type]
            failed = await _recommend(client)
            assert failed.is_error and f"{code}: {code} detail" in str(failed.content)
        # One call per request, no retry and no fallback to another seat.
        assert len(decision.calls) == 5
        decision.result = Decision("not-offered", {}, None)
        stray = await _recommend(client)
        assert stray.is_error and "bad_response" in str(stray.content)
        # A broken catalog is a configuration error, not "off".
        (root / "decision-seats.json").write_text("{broken")
        assert "config_error" in str((await _recommend(client)).content)


@pytest.mark.asyncio
async def test_a_key_with_a_line_break_is_a_config_error_that_does_not_quote_it(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "grok", "write")])
    _write_decision_key("AIza-private-part\nX-Evil: 1")
    async with Client(mcp) as client:
        tasks = [
            _batch_task("一", "write", tmp_path / "x"),
            _batch_task("二", "write", tmp_path / "y"),
        ]
        for name, arguments in (
            ("recommend_seat", {"task": "x", "access": "write"}),
            ("start_agents", {"tasks": tasks}),
        ):
            result = await client.call_tool(name, arguments)
            text = str(result.content)
            assert result.is_error and "config_error" in text and "decision-keys.json" in text
            assert "AIza" not in text and "Evil" not in text and "private-part" not in text
        assert decision.calls == [] and decision.batches == []


def _started_count(listed: object) -> int:
    return len(cast(list[object], _payload(listed)["invocations"]))


@pytest.mark.asyncio
async def test_start_agent_auto_asks_the_decision_model_and_starts_in_one_call(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [
            _own_seat("write-a", "claude", "write", effort="high"),
            _own_seat("write-b", "claude", "write"),
            _own_seat("read-c", "claude", "read"),
        ]
    )
    _write_decision_key()
    decision.result = Decision("write-b", {"write-b": 0.8, "write-a": 0.2}, confidence=0.9)
    async with Client(mcp) as client:
        tools = await client.list_tools(cache_mode="refresh")
        by_name = {tool.name: tool for tool in tools.tools}
        start = by_name["start_agent"]
        assert "task" not in start.input_schema["properties"]
        assert "sent to an external decision model" in json.dumps(
            start.input_schema["properties"]["seat"]
        )
        assert start.input_schema["$defs"]["OrchestrationRole"]["enum"] == [
            "write-a",
            "write-b",
            "read-c",
            "auto",
        ]
        assert "recommendation" in json.dumps(start.output_schema)

        started = await _start_and_wait(
            client,
            cwd=str(tmp_path),
            seat="auto",
            access="write",
            prompt="给设置页加一个开关",
        )
        assert started["provider"] == "claude" and started["model"] == "model-write-b"
        assert cast(list[str], started["defaults_applied"])[:2] == ["write-b seat", "model"]
        assert started["recommendation"] == {
            "seat": "write-b",
            "role": "role write-b",
            "confidence": 0.9,
            "decision_model_called": True,
            "candidates": [
                {"seat": "write-b", "role": "role write-b", "probability": 0.8},
                {"seat": "write-a", "role": "role write-a", "probability": 0.2},
            ],
        }
        # Only the prompt, the access and the candidate descriptions leave the machine.
        [(text, sent)] = decision.calls
        assert text == "Task:\n给设置页加一个开关\nRequired access: write"
        assert set(sent) == {"write-a", "write-b"}
        # The history holds the real seat, so a resume keeps it.
        resumed = await _start_and_wait(client, session_id="session-e2e")
        assert resumed["model"] == "model-write-b"
        assert "seat (session_id history)" in cast(list[str], resumed["defaults_applied"])
        # Explicit options win over the seat's, as with a named seat.
        explicit = await _start_and_wait(
            client,
            cwd=str(tmp_path),
            seat="auto",
            access="write",
            prompt="again",
            provider_options={"model": "explicit"},
        )
        explicit_defaults = cast(list[str], explicit["defaults_applied"])
        assert explicit["model"] == "explicit" and "model" not in explicit_defaults
        # As with a named seat, nothing of the seat is applied then; the choice still shows.
        assert "write-b seat" not in explicit_defaults
        assert _payload(explicit["recommendation"])["seat"] == "write-b"
        # One dispatchable seat of that access: no decision call, still started.
        decision.calls.clear()
        single = await _start_and_wait(
            client, cwd=str(tmp_path), seat="auto", access="read", prompt="look around"
        )
        recommendation = _payload(single["recommendation"])
        assert recommendation["seat"] == "read-c"
        assert recommendation["decision_model_called"] is False and decision.calls == []


@pytest.mark.asyncio
async def test_start_agents_isolates_every_writer_when_there_are_several(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    from agentnave.mcp_tasks import AUTO_ISOLATION_APPLIED

    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [
            _own_seat("write-a", "claude", "write"),
            _own_seat("write-b", "claude", "write", difficulty="hard"),
            _own_seat("read-c", "claude", "read"),
            _own_seat("read-d", "claude", "read", difficulty="easy"),
        ]
    )
    _write_decision_key()
    pick = Decision("write-a", {"write-a": 1.0}, 1.0)
    read = Decision("read-c", {"read-c": 1.0}, 1.0)
    one, two, three = (_light_repo(tmp_path / name) for name in ("one", "two", "three"))
    plain = tmp_path / "plain"
    plain.mkdir()

    async def run(answers: list[Decision], tasks: list[dict[str, object]]) -> list[dict[str, Any]]:
        decision.answers = answers
        result = await client.call_tool("start_agents", {"tasks": tasks})
        assert not result.is_error, result.content
        found = cast(list[dict[str, Any]], _payload(result.structured_content)["results"])
        await _finish(client, found)
        return found

    async with Client(mcp) as client:
        # Two writers: each gets its own directory; the reader does not.
        first, second, reader = await run(
            [pick, pick, read],
            [
                _batch_task("甲", "write", one),
                _batch_task("乙", "write", two),
                _batch_task("丙", "read", plain),
            ],
        )
        assert first["isolation"]["id"] != second["isolation"]["id"]
        assert first["isolation"]["path"] != second["isolation"]["path"]
        assert str(one) in first["isolation"]["path"] and str(two) in second["isolation"]["path"]
        for item in (first, second):
            assert AUTO_ISOLATION_APPLIED in item["defaults_applied"]
            assert "isolated directory instructions" in item["defaults_applied"]
        assert (
            "isolation" not in reader and AUTO_ISOLATION_APPLIED not in reader["defaults_applied"]
        )

        # One writer and one reader: nothing is isolated, as with start_agent alone.
        writer, reader = await run(
            [pick, read], [_batch_task("甲", "write", one), _batch_task("丙", "read", plain)]
        )
        assert "isolation" not in writer and "isolation" not in reader
        assert AUTO_ISOLATION_APPLIED not in writer["defaults_applied"]

        # The task's own isolation wins and is not marked as automatic.
        own, auto = await run(
            [pick, pick],
            [
                _batch_task("甲", "write", one, isolation={"ref": "main"}),
                _batch_task("乙", "write", two),
            ],
        )
        assert "isolation" in own and AUTO_ISOLATION_APPLIED not in own["defaults_applied"]
        assert "isolation" in auto and AUTO_ISOLATION_APPLIED in auto["defaults_applied"]

        # A directory that cannot be isolated fails alone and is never started without one.
        before = _started_count((await client.call_tool("list")).structured_content)
        good, broken = await run(
            [pick, pick],
            [
                _batch_task("甲", "write", three),
                {"access": "write", "prompt": "乙", "cwd": str(plain)},
            ],
        )
        assert good["status"] == "running" and "isolation" in good
        assert broken["status"] == "not_started" and "invocation_id" not in broken
        assert broken["error"]["code"] == "start_failed"
        assert broken["error"]["message"].startswith("automatic isolation failed")
        assert "git" in broken["error"]["message"].lower()
        assert broken["recommendation"]["seat"] == "write-a"
        after = _started_count((await client.call_tool("list")).structured_content)
        assert after == before + 1


def test_the_decision_model_reads_all_of_a_short_text_and_the_ends_of_a_long_one() -> None:
    from agentnave.seat_recommendation import MAX_DECISION_TEXT, clip_for_decision

    exact = "a" * 2000 + "b" * 2000
    assert MAX_DECISION_TEXT == 4000 and clip_for_decision(exact) == exact
    over = "h" * 3000 + "m" * 1 + "t" * 1000
    assert len(over) == 4001
    assert clip_for_decision(over) == "h" * 3000 + "\n…\n" + "t" * 1000
    long = "H" * 3000 + "M" * 50_000 + "T" * 1000
    clipped = clip_for_decision(long)
    assert clipped == "H" * 3000 + "\n…\n" + "T" * 1000 and "M" not in clipped
    # Characters, not bytes.
    assert clip_for_decision("字" * 4000) == "字" * 4000


@pytest.mark.asyncio
async def test_only_the_prompt_reaches_the_decision_model_clipped_when_long(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [_own_seat("write-a", "claude", "write"), _own_seat("write-b", "claude", "write")]
    )
    _write_decision_key()
    decision.result = Decision("write-a", {"write-a": 1.0}, 1.0)
    prompt = "START" + "x" * 3000 + "MIDDLE" + "y" * 1000 + "END"
    async with Client(mcp) as client:
        earlier = await _start_and_wait(client, cwd=str(tmp_path), seat="write-b")
        finished = _first(await client.call_tool("wait", {"ids": [earlier["invocation_id"]]}))
        forwarded = str(finished["message_id"])
        started = await _start_and_wait(
            client,
            cwd=str(tmp_path),
            seat="auto",
            access="write",
            prompt=prompt,
            forward=[forwarded],
        )
        assert started["model"] == "model-write-a"
        [(text, sent)] = decision.calls
        assert text == f"Task:\n{prompt[:3000]}\n…\n{prompt[-1000:]}\nRequired access: write"
        assert text.startswith("Task:\nSTART") and text.endswith("END\nRequired access: write")
        assert "MIDDLE" not in text and "fake claude" not in text + json.dumps(sent)
        # What the service adds to the prompt, and what is forwarded, is not sent.
        for added in ("noninteractive", "Forwarded", forwarded, "read-only"):
            assert added not in text + json.dumps(sent)
        # A batch sends each prompt the same way, and nothing else of the task.
        decision.calls.clear()
        decision.answers = [Decision("write-a", {"write-a": 1.0}, 1.0)] * 2
        batch = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    _batch_task(prompt, "write", tmp_path, forward=[forwarded]),
                    _batch_task("short", "write", tmp_path),
                ]
            },
        )
        assert not batch.is_error, batch.content
        [sent_batch] = decision.batches
        assert [one for one, _ in sent_batch] == [
            f"Task:\n{prompt[:3000]}\n…\n{prompt[-1000:]}\nRequired access: write",
            "Task:\nshort\nRequired access: write",
        ]
        await _finish(
            client, cast(list[dict[str, object]], _payload(batch.structured_content)["results"])
        )
        # recommend_seat clips a long task the same way instead of refusing it.
        decision.calls.clear()
        long = _payload((await _recommend(client, task=prompt)).structured_content)
        assert long["seat"] == "write-a"
        assert decision.calls[0][0] == text


@pytest.mark.asyncio
async def test_start_agent_auto_never_starts_when_the_decision_fails(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    _write_own_seats(
        [
            _own_seat("claude-w", "claude", "write"),
            _own_seat("grok-w", "grok", "write"),
            _own_seat("codex-w", "codex", "write"),
        ]
    )
    arguments = {
        "prompt": "ok",
        "cwd": str(tmp_path),
        "seat": "auto",
        "access": "write",
    }
    async with Client(mcp) as client:

        async def refused(code: str) -> None:
            result = await client.call_tool("start_agent", arguments)
            assert result.is_error and str(result.content).count(code), result.content
            assert _started_count((await client.call_tool("list")).structured_content) == 0

        await refused("no_key")
        (root / "decision-keys.json").write_text("{broken")
        await refused("config_error")
        _write_decision_key()
        for code in ("network", "rate_limited", "auth", "rejected", "bad_response"):
            decision.result = DecisionError(code, "detail")  # type: ignore[arg-type]
            await refused(f"{code}: detail")
        assert len(decision.calls) == 5  # no retry, no fallback to another seat
        # The decision picked the host's own CLI: its seat is named, nothing starts.
        decision.result = Decision("codex-w", {"codex-w": 0.9}, 0.9)
        await refused("native_seat")
        native = await client.call_tool("start_agent", arguments)
        assert "codex-w (role codex-w)" in str(native.content)
        assert "subagent using model " in str(native.content)
        # Nothing can be dispatched.
        (root / "decision-seats.json").write_text(json.dumps({"version": 1, "seats": []}))
        await refused("no_candidates")


@pytest.mark.asyncio
async def test_start_agent_auto_refuses_conflicting_arguments_before_any_decision(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "grok", "write")])
    _write_decision_key()
    base = {"prompt": "ok", "cwd": str(tmp_path)}
    auto = {**base, "seat": "auto", "access": "write"}
    async with Client(mcp) as client:
        for arguments, expected in (
            ({**auto, "session_id": "native-1"}, "cannot be combined with session_id"),
            ({**auto, "provider": "claude"}, "cannot be combined with provider"),
            ({k: v for k, v in auto.items() if k != "access"}, "needs access"),
            ({k: v for k, v in auto.items() if k != "cwd"}, "cwd is required"),
            ({**auto, "task": "x"}, "Unknown argument(s) for start_agent: task"),
        ):
            result = await client.call_tool("start_agent", arguments)
            assert result.is_error and expected in str(result.content), (arguments, result.content)
        assert decision.calls == []


@pytest.mark.asyncio
async def test_a_slow_decision_does_not_block_other_calls(decision: _FakeDecisionClient) -> None:
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "grok", "write")])
    _write_decision_key()
    decision.result = Decision("a", {"a": 0.6, "b": 0.4}, 0.6)
    decision.delay = 1.0
    order: list[str] = []
    async with Client(mcp) as client:

        async def run(name: str, tool: str, arguments: dict[str, object]) -> None:
            await client.call_tool(tool, arguments)
            order.append(name)

        await asyncio.gather(
            run("recommend", "recommend_seat", {"task": "x", "access": "write"}),
            run("members", "list_members", {}),
            run("list", "list", {}),
        )
    # The blocking HTTP call runs in a worker thread: the others finish while it waits.
    assert order[-1] == "recommend" and set(order[:2]) == {"members", "list"}


@pytest.mark.asyncio
async def test_a_broken_catalog_is_a_config_error_in_the_decision_mode_and_nothing_elsewhere(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    _write_own_seats([_own_seat("a", "claude", "write")])
    _write_decision_key()
    (root / "decision-seats.json").write_text("{broken")
    tasks = [_batch_task("一", "write", tmp_path / "x"), _batch_task("二", "write", tmp_path / "y")]
    async with Client(mcp) as client:
        # The mode is on, so the decision tools and seat='auto' stay published.
        tools = {tool.name: tool for tool in (await client.list_tools(cache_mode="refresh")).tools}
        assert {"recommend_seat", "start_agents"} <= set(tools)
        assert "auto" in tools["start_agent"].input_schema["$defs"]["OrchestrationRole"]["enum"]
        assert "seat" in tools["start_agent"].input_schema["properties"]
        calls = {
            "recommend_seat": {"task": "x", "access": "write"},
            "start_agents": {"tasks": tasks},
            "start_agent": {
                "prompt": "ok",
                "cwd": str(tmp_path),
                "seat": "auto",
                "access": "write",
            },
        }
        for name, arguments in calls.items():
            result = await client.call_tool(name, arguments)
            assert result.is_error and "config_error" in str(result.content), (name, result.content)
            assert "not_enabled" not in str(result.content)
            assert "decision-seats.json" in str(result.content)
        assert decision.calls == [] and decision.batches == []
        assert _started_count((await client.call_tool("list")).structured_content) == 0
        # Without the decision mode, the same file changes nothing: the tools are not there.
        (root / "orchestration.json").write_text(json.dumps({"version": 1, "preset": "split"}))
        tools = {tool.name for tool in (await client.list_tools(cache_mode="refresh")).tools}
        assert tools == set(EXPECTED_TOOLS)
        refused = await client.call_tool("start_agents", {"tasks": tasks})
        assert refused.is_error and "not_enabled" in str(refused.content)
        refused = await client.call_tool("recommend_seat", calls["recommend_seat"])
        assert refused.is_error and "not_enabled" in str(refused.content)


def _light_repo(path: Path) -> Path:
    """A Git repository with one empty commit, enough for an isolated directory."""
    path.mkdir(exist_ok=True)
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid"]
    subprocess.run([*git, "init", "-q", "-b", "main"], cwd=path, check=True)
    subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "init"], cwd=path, check=True)
    return path


def _batch_task(task: str, access: str, cwd: Path, **extra: object) -> dict[str, object]:
    # Several writers are isolated, which needs a repository; one that is not fails alone.
    if access == "write" and not (cwd / ".git").exists():
        _light_repo(cwd)
    cwd.mkdir(exist_ok=True)
    return {"access": access, "prompt": task, "cwd": str(cwd), **extra}


async def _finish(client: Client, results: Sequence[Mapping[str, object]]) -> None:
    ids = [str(item["invocation_id"]) for item in results if "invocation_id" in item]
    if ids:
        waited: dict[str, object] = {}
        for _ in range(20):
            waited = _payload((await client.call_tool("wait", {"ids": ids})).structured_content)
            if waited["all_finished"]:
                return
            ids = cast(list[str], waited["pending_ids"])
        raise AssertionError(f"the started tasks did not finish: {waited}")


@pytest.mark.asyncio
async def test_start_agents_chooses_every_seat_in_one_decision_request(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [
            _own_seat("write-a", "claude", "write"),
            _own_seat("write-b", "claude", "write", difficulty="hard"),
            _own_seat("read-c", "claude", "read"),
            _own_seat("read-d", "claude", "read", difficulty="easy"),
        ]
    )
    _write_decision_key()
    decision.answers = [
        Decision("write-a", {"write-a": 0.8, "write-b": 0.2}, 0.8),
        Decision("read-d", {"read-c": 0.4, "read-d": 0.6}, 0.6),
        Decision("write-b", {"write-a": 0.3, "write-b": 0.7}, 0.7),
    ]
    async with Client(mcp) as client:
        tools = await client.list_tools(cache_mode="refresh")
        published = {tool.name: tool for tool in tools.tools}
        schema = published["start_agents"].input_schema
        assert set(schema["properties"]) == {
            "tasks",
            "cwd",
            "title",
            "manager_model",
            "manager_session_id",
        }
        assert schema["properties"]["tasks"]["minItems"] == 2
        assert published["start_agents"].output_schema
        result = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    _batch_task("加开关", "write", tmp_path / "one", role="开关"),
                    _batch_task("看日志", "read", tmp_path / "two"),
                    _batch_task("写文档", "write", tmp_path / "three"),
                ]
            },
        )
        assert not result.is_error, result.content
        payload = _payload(result.structured_content)
        # One decision request for all three, each with its own access's candidates.
        assert decision.calls == [] and len(decision.batches) == 1
        assert [(text, set(candidates)) for text, candidates in decision.batches[0]] == [
            ("Task:\n加开关\nRequired access: write", {"write-a", "write-b"}),
            ("Task:\n看日志\nRequired access: read", {"read-c", "read-d"}),
            ("Task:\n写文档\nRequired access: write", {"write-a", "write-b"}),
        ]
        results = cast(list[dict[str, object]], payload["results"])
        assert [(item["provider"], item["model"]) for item in results] == [
            ("claude", "model-write-a"),
            ("claude", "model-read-d"),
            ("claude", "model-write-b"),
        ]
        picked = [_payload(item["recommendation"]) for item in results]
        assert [one["seat"] for one in picked] == ["write-a", "read-d", "write-b"]
        assert all(one["decision_model_called"] is True for one in picked)
        assert [item["access"] for item in results] == ["write", "read", "write"]
        assert "next" in payload
        await _finish(client, results)


@pytest.mark.asyncio
async def test_one_task_that_cannot_start_does_not_stop_the_others(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [
            _own_seat("write-a", "claude", "write"),
            _own_seat("write-b", "claude", "write", difficulty="hard"),
            _own_seat("codex-w", "codex", "write", difficulty="easy"),
            _own_seat("read-solo", "claude", "read"),
        ]
    )
    _write_decision_key()
    decision.answers = [
        DecisionError("bad_response", "The decision model chose no listed seat."),
        Decision("codex-w", {"codex-w": 0.9}, 0.9),
        Decision("write-b", {"write-b": 0.8, "write-a": 0.2}, 0.8),
    ]
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    _batch_task("一", "write", tmp_path / "one"),
                    _batch_task("二", "read", tmp_path / "two"),  # one candidate: no question
                    _batch_task("三", "write", tmp_path / "three"),
                    _batch_task("四", "write", tmp_path / "four"),
                ]
            },
        )
        assert not result.is_error, result.content
        results = cast(list[dict[str, object]], _payload(result.structured_content)["results"])
        assert len(decision.batches) == 1 and len(decision.batches[0]) == 3
        bad, solo, native, started = results
        assert bad["status"] == "not_started"
        assert _payload(bad["error"])["code"] == "bad_response" and "recommendation" not in bad
        assert solo["model"] == "model-read-solo"
        assert _payload(solo["recommendation"])["decision_model_called"] is False
        assert native["status"] == "not_started"
        assert _payload(native["error"])["code"] == "native_seat"
        assert _payload(native["recommendation"])["native"] is True
        assert started["model"] == "model-write-b"
        # Only the two that started are waited for; the unstarted ones left nothing behind.
        listed = _payload((await client.call_tool("list")).structured_content)
        assert len(cast(list[object], listed["invocations"])) == 2
        await _finish(client, results)


@pytest.mark.asyncio
async def test_a_task_without_candidates_is_left_out_of_the_decision_request(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [_own_seat("write-a", "claude", "write"), _own_seat("write-b", "claude", "write")]
    )
    _write_decision_key()
    decision.answers = [Decision("write-a", {"write-a": 1.0}, 1.0)] * 2
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    _batch_task("读", "read", tmp_path / "one"),
                    _batch_task("写", "write", tmp_path / "two"),
                    _batch_task("再写", "write", tmp_path / "three"),
                ]
            },
        )
        assert not result.is_error, result.content
        results = cast(list[dict[str, object]], _payload(result.structured_content)["results"])
        assert _payload(results[0]["error"])["code"] == "no_candidates"
        assert [item["status"] for item in results[1:]] == ["running", "running"]
        assert len(decision.batches[0]) == 2
        await _finish(client, results)
        # Nothing to decide at all: no key is needed, and nothing starts.
        (Path(os.environ["AGENTNAVE_DATA_DIR"]) / "decision-keys.json").unlink()
        decision.batches.clear()
        none = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    _batch_task("读", "read", tmp_path / "a"),
                    _batch_task("读", "read", tmp_path / "b"),
                ]
            },
        )
        assert not none.is_error
        nothing = _payload(none.structured_content)
        codes = [
            _payload(item["error"])["code"]
            for item in cast(list[dict[str, object]], nothing["results"])
        ]
        assert codes == ["no_candidates", "no_candidates"]
        assert "next" not in nothing and decision.batches == []


@pytest.mark.asyncio
async def test_start_agents_never_starts_when_the_decision_fails_as_a_whole(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "grok", "write")])
    tasks = [_batch_task("一", "write", tmp_path / "x"), _batch_task("二", "write", tmp_path / "y")]
    async with Client(mcp) as client:

        async def refused(code: str) -> None:
            result = await client.call_tool("start_agents", {"tasks": tasks})
            assert result.is_error and str(result.content).count(code), result.content
            assert _started_count((await client.call_tool("list")).structured_content) == 0

        await refused("no_key")
        (root / "decision-keys.json").write_text("{broken")
        await refused("config_error")
        _write_decision_key()
        for code in ("network", "rate_limited", "auth", "rejected", "bad_response"):
            decision.answers = DecisionError(code, "detail")  # type: ignore[arg-type]
            await refused(f"{code}: detail")
        assert len(decision.batches) == 5  # one request each: no retry, no other seat
        assert decision.calls == []


@pytest.mark.asyncio
async def test_start_agents_checks_everything_free_before_any_decision(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "grok", "write")])
    _write_decision_key()
    (Path(os.environ["AGENTNAVE_DATA_DIR"]) / "settings.json").write_text(
        json.dumps({"version": 1, "max_running": 2})
    )
    good = _batch_task("一", "write", tmp_path / "x")
    async with Client(mcp) as client:

        async def refused(arguments: dict[str, object], expected: str) -> None:
            result = await client.call_tool("start_agents", arguments)
            assert result.is_error and expected in str(result.content), result.content
            assert decision.batches == [] and decision.calls == []

        # More tasks than the App's limit allows.
        await refused({"tasks": [good, good, good]}, "too_many_tasks")
        # A task with no directory, a missing one, a bad role.
        await refused({"tasks": [good, {**good, "cwd": None}]}, "tasks[1]: cwd is required")
        await refused(
            {"tasks": [good, {**good, "cwd": str(tmp_path / "gone")}]},
            "tasks[1]: cwd is not a directory",
        )
        await refused({"tasks": [good, {**good, "role": "x" * 41}]}, "tasks[1]: role must contain")
        # What the tool does not take.
        await refused(
            {"tasks": [good, good], "seat": "a"}, "Unknown argument(s) for start_agents: seat"
        )
        for key in ("seat", "provider", "session_id", "wait", "provider_options"):
            await refused({"tasks": [good, {**good, key: "x"}]}, key)
        await refused({"tasks": [good, {**good, "task": "summary"}]}, "task")
        await refused({"tasks": [good]}, "tasks")
        # The directory a top-level cwd gives is checked the same way.
        await refused(
            {"tasks": [{k: v for k, v in good.items() if k != "cwd"}] * 2, "cwd": "relative"},
            "tasks[0]: cwd must be an absolute path",
        )


@pytest.mark.asyncio
async def test_start_agents_refuses_the_whole_call_beyond_the_free_capacity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, decision: _FakeDecisionClient
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats([_own_seat("a", "claude", "write"), _own_seat("b", "claude", "write")])
    _write_decision_key()
    (Path(os.environ["AGENTNAVE_DATA_DIR"]) / "settings.json").write_text(
        json.dumps({"version": 1, "max_running": 3})
    )
    async with Client(mcp) as client:
        busy = await client.call_tool(
            "start_agent", {"prompt": "sleep", "cwd": str(tmp_path), "seat": "a"}
        )
        assert not busy.is_error, busy.content
        result = await client.call_tool(
            "start_agents",
            {"tasks": [_batch_task("x", "write", tmp_path / str(i)) for i in range(3)]},
        )
        assert result.is_error and "capacity" in str(result.content)
        assert "Nothing was started" in str(result.content)
        assert decision.batches == []
        assert _started_count((await client.call_tool("list")).structured_content) == 1
        await client.call_tool(
            "cancel", {"ids": [_payload(busy.structured_content)["invocation_id"]]}
        )


@pytest.mark.asyncio
async def test_start_agents_exists_only_in_the_decision_mode(
    tmp_path: Path, decision: _FakeDecisionClient
) -> None:
    _write_own_seats([_own_seat("a", "claude", "write")], preset="split")
    async with Client(mcp) as client:
        tools = await client.list_tools(cache_mode="refresh")
        assert [tool.name for tool in tools.tools] == EXPECTED_TOOLS
        assert "StartAgentsPayload" not in json.dumps([tool.model_dump() for tool in tools.tools])
        refused = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    _batch_task("x", "write", tmp_path / "a"),
                    _batch_task("y", "write", tmp_path / "b"),
                ]
            },
        )
        assert refused.is_error and "not_enabled" in str(refused.content)
        assert decision.batches == [] and decision.calls == []
        _write_own_seats([_own_seat("a", "claude", "write")])
        tools = await client.list_tools(cache_mode="refresh")
        assert "start_agents" in [tool.name for tool in tools.tools]


@pytest.mark.asyncio
async def test_review_seat_fallback_dispatch_and_resume(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health

    _install_fake_claude(tmp_path, monkeypatch)
    root = tmp_path / "data"
    provider_health.record(root, "claude", "auth", "first")
    (root / "review.json").write_text(
        json.dumps(
            {
                "version": 1,
                "seats": {
                    "correctness-reviewer": {
                        "provider": "claude",
                        "model": "first",
                        "fallbacks": [{"provider": "claude", "model": "second", "effort": "low"}],
                    }
                },
            }
        )
    )
    async with Client(mcp) as client:
        listed = _payload((await client.call_tool("list_members", {})).structured_content)
        seat = _payload(_payload(listed["review"])["correctness-reviewer"])
        assert seat["model"] == "second" and seat["fallback_position"] == 1
        assert _payload(cast(list[object], seat["fallbacks"])[0])["available"] is True
        started = await _start_and_wait(client, cwd=str(tmp_path), role="correctness-reviewer")
        assert started["model"] == "second"
        receipt = _payload(started["seat_fallback"])
        assert receipt["position"] == 1 and "auth" in str(receipt["skipped"])
        explicit = await _start_and_wait(
            client, cwd=str(tmp_path), role="correctness-reviewer", provider="claude"
        )
        assert explicit["model"] == "second" and "seat_fallback" in explicit
        resumed = await client.call_tool(
            "start_agent",
            {"session_id": "session-e2e", "role": "correctness-reviewer", "prompt": "ok"},
        )
        assert resumed.is_error and "auth" in str(resumed.content)
        provider_health.record(root, "claude", "auth", "second")
        failed = await client.call_tool(
            "start_agent", {"cwd": str(tmp_path), "role": "correctness-reviewer", "prompt": "ok"}
        )
        assert failed.is_error and "first" in str(failed.content)
        (root / "review.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "seats": {
                        "correctness-reviewer": {
                            "provider": "grok",
                            "model": "primary",
                            "fallbacks": [{"provider": "claude", "model": "third"}],
                        }
                    },
                }
            )
        )
        restricted = await _start_and_wait(
            client, cwd=str(tmp_path), role="correctness-reviewer", provider="claude"
        )
        assert restricted["model"] == "third"
        assert _payload(restricted["seat_fallback"])["skipped"] == []
        explicit_model = await _start_and_wait(
            client,
            cwd=str(tmp_path),
            role="correctness-reviewer",
            provider="claude",
            provider_options={"model": "third"},
        )
        assert "seat_fallback" not in explicit_model


@pytest.mark.asyncio
async def test_decision_candidate_and_batch_use_available_fallback(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health

    _install_fake_claude(tmp_path, monkeypatch)
    _write_own_seats(
        [_own_seat("mine", "grok", "read", fallbacks=[{"provider": "claude", "model": "backup"}])]
    )
    root = Path(os.environ["AGENTNAVE_DATA_DIR"])
    provider_health.record(root, "grok", "auth")
    async with Client(mcp) as client:
        result = await client.call_tool(
            "start_agents",
            {
                "tasks": [
                    {"prompt": "one", "access": "read", "cwd": str(tmp_path)},
                    {"prompt": "two", "access": "read", "cwd": str(tmp_path)},
                ]
            },
        )
        assert not result.is_error, result.content
        started = cast(list[dict[str, object]], _payload(result.structured_content)["results"])
        assert all(item.get("model") == "backup" and "seat_fallback" in item for item in started)
        await _finish(client, started)


@pytest.mark.asyncio
async def test_image_fallback_skips_host_cli_and_hides_all_native(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex", "env"))
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"codex"}))
    root = tmp_path / "data"
    root.mkdir()
    path = root / "orchestration.json"
    image = {
        "provider": "codex",
        "model": "host-image",
        "fallbacks": [
            {"provider": "claude", "model": "other-image"},
            {"provider": "codex", "model": "another-host-image"},
        ],
    }
    path.write_text(json.dumps({"version": 1, "preset": "split", "image_generation": image}))
    async with Client(mcp) as client:
        listing = _payload((await client.call_tool("list_members", {})).structured_content)
        status = _payload(_payload(listing["orchestration"])["image_generation"])
        assert status["provider"] == "claude" and status["fallback_position"] == 1
        fallback_status = cast(list[dict[str, object]], status["fallbacks"])
        assert fallback_status[1]["available"] is False
        assert "host's own CLI" in str(fallback_status[1]["reason"])
        assert "effort" not in fallback_status[0]
        started = await _start_and_wait(client, cwd=str(tmp_path), seat="image-generator")
        assert started["model"] == "other-image"
        receipt = _payload(started["seat_fallback"])
        assert receipt["position"] == 1 and "host's own CLI" in str(receipt["skipped"])
        image["fallbacks"] = [{"provider": "codex", "model": "another-host-image"}]
        path.write_text(json.dumps({"version": 1, "preset": "split", "image_generation": image}))
        listing = _payload((await client.call_tool("list_members", {})).structured_content)
        assert "image_generation" not in _payload(listing["orchestration"])


@pytest.mark.asyncio
async def test_native_seat_error_describes_selected_model_and_effort(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave import provider_health

    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex", "env"))
    root = tmp_path / "data"
    provider_health.record(root, "grok", "auth")
    path = root / "review.json"
    seat: dict[str, object] = {"provider": "codex", "model": "native-model", "effort": "high"}
    async with Client(mcp) as client:
        path.write_text(json.dumps({"version": 1, "seats": {"correctness-reviewer": seat}}))
        args = {"prompt": "ok", "cwd": str(tmp_path), "role": "correctness-reviewer"}
        result = await client.call_tool("start_agent", args)
        assert result.is_error and "using model native-model, effort high" in str(result.content)
        seat = {
            "provider": "grok",
            "model": "blocked",
            "fallbacks": [{"provider": "codex", "model": "native-backup"}],
        }
        path.write_text(json.dumps({"version": 1, "seats": {"correctness-reviewer": seat}}))
        result = await client.call_tool("start_agent", args)
        assert result.is_error and "using model native-backup." in str(result.content)


@pytest.mark.asyncio
async def test_decision_image_fallback_skips_host_cli(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_fake_claude(tmp_path, monkeypatch)
    monkeypatch.setattr(mcp_runtime, "HOST", Host("codex", "env"))
    monkeypatch.setattr(mcp_runtime, "EXCLUDED_PROVIDERS", frozenset({"codex"}))
    image = _own_seat(
        "draw",
        "codex",
        "write",
        kind="image",
        fallbacks=[{"provider": "claude", "model": "draw-model"}],
    )
    del image["difficulty"]
    _write_own_seats([image])
    async with Client(mcp) as client:
        listing = _payload((await client.call_tool("list_members", {})).structured_content)
        seat = _payload(_payload(_payload(listing["orchestration"])["custom_seats"])["draw"])
        assert seat["provider"] == "claude" and seat["fallback_position"] == 1
        started = await _start_and_wait(client, cwd=str(tmp_path), seat="draw")
        assert started["model"] == "draw-model" and "host's own CLI" in str(
            started["seat_fallback"]
        )
        image["fallbacks"] = [{"provider": "codex", "model": "another-model"}]
        _write_own_seats([image])
        listing = _payload((await client.call_tool("list_members", {})).structured_content)
        assert _payload(listing["orchestration"]).get("custom_seats", {}) == {}

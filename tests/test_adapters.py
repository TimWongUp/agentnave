from __future__ import annotations

import json
import os
import stat
import uuid
from pathlib import Path

import pytest

from agentnave.adapters import base, codex, get_adapter
from agentnave.adapters import deepseek as deepseek_adapter
from agentnave.adapters.antigravity import (
    AntigravityAdapter,
    session_model,
    write_restricted_profile,
)
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codebuddy import CodeBuddyAdapter
from agentnave.adapters.codex import CodexAdapter
from agentnave.adapters.deepseek import DeepSeekAdapter
from agentnave.adapters.grok import GrokAdapter
from agentnave.adapters.pi import PiAdapter
from agentnave.models import InvocationRequest, InvocationStatus, ProviderOption
from agentnave.processes import provider_environment


def test_native_models_do_not_attribute_child_or_usage_models_to_main_assistant() -> None:
    assistant: dict[str, object] = {"type": "assistant", "message": {"model": "main"}}
    assert ClaudeAdapter().event_model(assistant) == "main"
    assert CodeBuddyAdapter().event_model(assistant) == "main"
    assert ClaudeAdapter().event_model({**assistant, "parent_tool_use_id": "child"}) is None
    for model in ("unknown", "<synthetic>", "", "\nsecret"):
        assert (
            ClaudeAdapter().event_model({"type": "assistant", "message": {"model": model}}) is None
        )
    assert ClaudeAdapter().event_model({"type": "result", "modelUsage": {"child": {}}}) is None
    grok_end: dict[str, object] = {"type": "end", "modelUsage": {"grok-4.7-build": {}}}
    assert GrokAdapter().event_model(grok_end) == "grok-4.7-build"
    assert GrokAdapter().event_model({"type": "end", "modelUsage": {"a": {}, "b": {}}}) is None
    assert (
        ClaudeAdapter().event_model({"type": "system", "subtype": "init", "model": "selected"})
        is None
    )
    assert (
        PiAdapter().event_model(
            {
                "type": "message_end",
                "message": {"role": "assistant", "model": "requested", "responseModel": "served"},
            },
        )
        == "served"
    )
    assert (
        PiAdapter().event_model(
            {"type": "message_end", "message": {"role": "toolResult", "model": "tool-model"}}
        )
        is None
    )


@pytest.fixture(autouse=True)
def no_installed_providers(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Argv assertions use bare names; keep real CLIs on this machine out of resolution.
    monkeypatch.setenv("PATH", str(tmp_path / "no-bin"))


@pytest.mark.parametrize(
    ("provider", "event", "code"),
    [
        ("claude", {"type": "system", "subtype": "api_retry", "error": "overloaded_error"}, None),
        (
            "claude",
            {"type": "system", "subtype": "api_retry", "error": "authentication_failed"},
            "authentication_failed",
        ),
        (
            "claude",
            {
                "type": "user",
                "message": {
                    "content": [{"type": "tool_result", "is_error": True, "content": "test failed"}]
                },
            },
            None,
        ),
        (
            "codebuddy",
            {
                "type": "user",
                "message": {
                    "content": [
                        {
                            "type": "tool_result",
                            "content": [
                                {
                                    "type": "text",
                                    "text": "Error: Permission to use Bash has been denied because this tool requires approval but permission prompts are not available in non-interactive mode.",
                                }
                            ],
                        }
                    ]
                },
            },
            None,  # the CLI recovers with another tool, so a denied call does not wake the boss
        ),
        ("codex", {"type": "turn.failed", "error": {"message": "secret"}}, "provider_failed"),
        (
            "codex",
            {
                "type": "error",
                "message": "Reconnecting... 2/5 (unexpected status 401 Unauthorized: Missing bearer "
                "or basic authentication in header, url: wss://api.openai.com/v1/responses)",
            },
            None,  # the HTTPS fallback may still succeed, so a retry is never a blocker
        ),
        ("grok", {"type": "error", "message": "secret"}, "provider_failed"),
        ("deepseek", {"type": "error", "message": "secret"}, "provider_failed"),
        ("deepseek", {"type": "tool_result", "callId": "c1", "status": "error"}, None),
        ("antigravity", {"event": "result", "result": {"status": "WAITING"}}, "provider_blocked"),
        ("antigravity", {"event": "result", "result": {"status": "ERROR"}}, None),
    ],
)
def test_execution_blockers_are_distinct_from_recoverable_tool_errors(
    provider: str, event: dict[str, object], code: str | None
) -> None:
    activity = get_adapter(provider).activity(event)
    assert activity is not None
    assert activity.blocking_error == code
    assert activity.public_output is None


def request(
    tmp_path: Path,
    provider: str,
    *,
    session_id: str | None = None,
    provider_options: dict[str, str | int | float | bool] | None = None,
) -> InvocationRequest:
    return InvocationRequest(
        provider=provider,
        prompt="do the task",
        cwd=tmp_path,
        session_id=session_id,
        provider_options=provider_options or {},
    )


def test_claude_adapter_only_passes_transport_and_explicit_options(tmp_path: Path) -> None:
    prepared = ClaudeAdapter().prepare(
        request(
            tmp_path,
            "claude",
            session_id="session-1",
            provider_options={"model": "sonnet", "effort": "high"},
        )
    )

    assert prepared.argv == (
        "claude",
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--resume=session-1",
        "--model",
        "sonnet",
        "--effort",
        "high",
    )
    assert prepared.stdin == b"do the task"


def test_claude_adapter_preserves_result_session_and_usage_summary() -> None:
    payload = {
        "type": "result",
        "result": "finished",
        "session_id": "session-2",
        "usage": {"input_tokens": 12},
        "num_turns": 2,
        "total_cost_usd": 0.02,
        "is_error": False,
    }
    result = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "finished"
    assert result.session_id == "session-2"
    assert result.usage == {"num_turns": 2, "total_cost_usd": 0.02}


def test_codebuddy_adapter_only_passes_transport_and_explicit_options(tmp_path: Path) -> None:
    prepared = CodeBuddyAdapter().prepare(
        request(
            tmp_path,
            "codebuddy",
            session_id="session-3",
            provider_options={"model": "hy3", "effort": "high"},
        )
    )

    assert prepared.argv == (
        "codebuddy",
        "--print",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--resume=session-3",
        "--model",
        "hy3",
        "--effort",
        "high",
    )
    assert prepared.stdin == b"do the task"


def test_codebuddy_adapter_parses_documented_result_contract() -> None:
    payload: dict[str, object] = {
        "type": "result",
        "subtype": "success",
        "result": "finished",
        "session_id": "session-4",
        "num_turns": 3,
        "total_cost_usd": 0.03,
        "is_error": False,
        "permission_denials": [],
    }

    result = CodeBuddyAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "finished"
    assert result.session_id == "session-4"
    assert result.usage == {"num_turns": 3, "total_cost_usd": 0.03}


def test_codebuddy_permission_denials_block_only_a_failed_run() -> None:
    payload = {
        "type": "result",
        "subtype": "success",
        "result": "done another way",
        "session_id": "session-5",
        "is_error": False,
        "permission_denials": [{"tool": "Bash"}],
    }

    succeeded = CodeBuddyAdapter().parse(0, json.dumps(payload).encode(), b"")
    failed = CodeBuddyAdapter().parse(1, json.dumps({**payload, "is_error": True}).encode(), b"")

    assert succeeded.status is InvocationStatus.SUCCEEDED
    assert succeeded.output == "done another way"
    assert failed.status is InvocationStatus.BLOCKED
    assert failed.error_message == "codebuddy requires permission for one or more tool calls"


def test_codebuddy_authentication_failure_is_blocked() -> None:
    payload = {
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": True,
        "errors": ["Authentication failed. Please use /login command to sign in to your account"],
    }

    result = CodeBuddyAdapter().parse(1, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED


def test_codex_adapter_passes_only_transport_and_explicit_options(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", ())
    monkeypatch.setattr(codex, "_WINDOWS_DESKTOP_CODEX_PATHS", ())
    prepared = CodexAdapter().prepare(
        request(
            tmp_path,
            "codex",
            session_id="0199a213-81c0-7800-8aa1-bbab2a035a53",
            provider_options={
                "model": "gpt-5.6-sol",
                "effort": "high",
                "sandbox": "workspace-write",
                "skip_git_repo_check": True,
            },
        )
    )

    assert prepared.argv == (
        "codex",
        "exec",
        "resume",
        "--json",
        "--skip-git-repo-check",
        "--model",
        "gpt-5.6-sol",
        "--config",
        "model_reasoning_effort=high",
        "--config",
        'sandbox_mode="workspace-write"',
        "--skip-git-repo-check",
        "0199a213-81c0-7800-8aa1-bbab2a035a53",
        "-",
    )
    assert prepared.stdin == b"do the task"


def test_codex_adapter_does_not_supply_model_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", ())
    monkeypatch.setattr(codex, "_WINDOWS_DESKTOP_CODEX_PATHS", ())
    prepared = CodexAdapter().prepare(request(tmp_path, "codex"))

    assert prepared.argv == ("codex", "exec", "--json", "--skip-git-repo-check", "-")


@pytest.mark.skipif(os.name == "nt", reason="macOS bundle selection uses POSIX executability")
def test_codex_adapter_prefers_executable_desktop_bundle(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(codex.sys, "platform", "darwin")
    unavailable = tmp_path / "unavailable"
    unavailable.write_text("not executable")
    desktop = tmp_path / "desktop codex"
    desktop.write_text("#!/bin/sh\n")
    desktop.chmod(0o755)
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", (str(unavailable), str(desktop)))

    prepared = CodexAdapter().prepare(request(tmp_path, "codex", session_id="session-1"))
    assert prepared.argv == (
        str(desktop),
        "exec",
        "resume",
        "--json",
        "--skip-git-repo-check",
        "session-1",
        "-",
    )

    desktop.unlink()
    assert CodexAdapter().prepare(request(tmp_path, "codex")).argv[0] == "codex"


def test_codex_adapter_uses_path_outside_macos(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(codex.sys, "platform", "linux")
    desktop = tmp_path / "desktop-codex"
    desktop.write_text("#!/bin/sh\n")
    desktop.chmod(0o755)
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", (str(desktop),))

    assert CodexAdapter().prepare(request(tmp_path, "codex")).argv[0] == "codex"


def _fake_executable(directory: Path, name: str) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    path.write_text("#!/bin/sh\n")
    path.chmod(0o755)
    return path


@pytest.mark.skipif(os.name == "nt", reason="Windows keeps resolving bare names")
def test_provider_executable_prefers_path_then_common_dirs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    home = tmp_path / "home"
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", ("~/.local/bin", "~/.grok/bin"))
    monkeypatch.setattr(codex.sys, "platform", "linux")

    assert GrokAdapter().prepare(request(tmp_path, "grok")).argv[0] == "grok"
    later = _fake_executable(home / ".grok" / "bin", "grok")
    assert GrokAdapter().prepare(request(tmp_path, "grok")).argv[0] == str(later)
    earlier = _fake_executable(home / ".local" / "bin", "grok")
    assert GrokAdapter().prepare(request(tmp_path, "grok")).argv[0] == str(earlier)
    on_path = _fake_executable(tmp_path / "path-bin", "grok")
    monkeypatch.setenv("PATH", str(on_path.parent))
    assert GrokAdapter().prepare(request(tmp_path, "grok")).argv[0] == str(on_path)

    common_codex = _fake_executable(home / ".local" / "bin", "codex")
    assert CodexAdapter().prepare(request(tmp_path, "codex")).argv[0] == str(common_codex)
    monkeypatch.setattr(codex.sys, "platform", "darwin")
    desktop = _fake_executable(tmp_path / "desktop", "codex")
    monkeypatch.setattr(codex, "_DESKTOP_CODEX_PATHS", (str(desktop),))
    assert CodexAdapter().prepare(request(tmp_path, "codex")).argv[0] == str(desktop)


@pytest.mark.skipif(os.name == "nt", reason="Windows keeps the inherited environment")
def test_provider_environment_orders_executable_inherited_then_common_dirs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    common = tmp_path / "common"
    common.mkdir()
    monkeypatch.setattr(base, "PROVIDER_BIN_DIRS", (str(common), str(tmp_path / "absent")))
    monkeypatch.setenv("PATH", os.pathsep.join(("/usr/bin", "/bin", "/usr/bin")))
    executable = tmp_path / "cli" / "grok"

    env = provider_environment((str(executable), "--help"))

    assert env["PATH"].split(os.pathsep) == [
        str(executable.parent),
        "/usr/bin",
        "/bin",
        str(common),
    ]
    assert os.environ["PATH"] == os.pathsep.join(("/usr/bin", "/bin", "/usr/bin"))


def test_provider_environment_keeps_manager_chat_identity_private(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("AGENTNAVE_HOST_SESSION_ID", "private-manager-chat")
    monkeypatch.setenv("AGENTNAVE_HOST_APP", "codex@private-account")
    monkeypatch.setenv("CLAUDE_CODE_SESSION_ID", "boss-claude-chat")
    provider_keys = set(provider_environment(("claude",)))
    assert (
        not {"AGENTNAVE_HOST_SESSION_ID", "AGENTNAVE_HOST_APP", "CLAUDE_CODE_SESSION_ID"}
        & provider_keys
    )
    assert os.environ["AGENTNAVE_HOST_SESSION_ID"] == "private-manager-chat"


def test_codex_adapter_rejects_non_boolean_git_check_override(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="skip_git_repo_check must be a boolean"):
        CodexAdapter().prepare(
            request(
                tmp_path,
                "codex",
                provider_options={"skip_git_repo_check": "true"},
            )
        )


@pytest.mark.parametrize("value", [None, False, True])
def test_codex_yolo_requires_explicit_option(tmp_path: Path, value: bool | None) -> None:
    options: dict[str, str | int | float | bool] = (
        {} if value is None else {"dangerously_bypass_approvals_and_sandbox": value}
    )
    prepared = CodexAdapter().prepare(request(tmp_path, "codex", provider_options=options))
    flag = "--dangerously-bypass-approvals-and-sandbox"
    assert (flag in prepared.argv) is (value is True)


def test_codex_approve_for_me_maps_to_settings_on_resume(tmp_path: Path) -> None:
    options: dict[str, str | int | float | bool] = {"approve_for_me": True}
    fresh = CodexAdapter().prepare(request(tmp_path, "codex", provider_options=options))
    assert "--approve-for-me" in fresh.argv
    # `exec resume` has no such flag; the same settings go through --config.
    resumed = CodexAdapter().prepare(
        request(tmp_path, "codex", session_id="thread-1", provider_options=options)
    )
    assert "--approve-for-me" not in resumed.argv
    assert 'approvals_reviewer="auto_review"' in resumed.argv
    assert 'sandbox_mode="workspace-write"' in resumed.argv
    for other in ({"sandbox": "read-only"}, {"dangerously_bypass_approvals_and_sandbox": True}):
        with pytest.raises(ValueError, match="cannot be combined"):
            CodexAdapter().prepare(
                request(tmp_path, "codex", provider_options={**options, **other})
            )


def test_codex_yolo_requires_boolean(tmp_path: Path) -> None:
    with pytest.raises(
        ValueError, match="dangerously_bypass_approvals_and_sandbox must be a boolean"
    ):
        CodexAdapter().prepare(
            request(
                tmp_path,
                "codex",
                provider_options={"dangerously_bypass_approvals_and_sandbox": "true"},
            )
        )


def test_codex_adapter_preserves_final_message_and_session() -> None:
    payload = b"\n".join(
        (
            json.dumps(
                {
                    "type": "thread.started",
                    "thread_id": "0199a213-81c0-7800-8aa1-bbab2a035a53",
                }
            ).encode(),
            json.dumps(
                {
                    "type": "item.completed",
                    "item": {"id": "item_1", "type": "agent_message", "text": "finished"},
                }
            ).encode(),
            json.dumps(
                {
                    "type": "turn.completed",
                    "usage": {"input_tokens": 12, "output_tokens": 3},
                }
            ).encode(),
        )
    )

    result = CodexAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "finished"
    assert result.session_id == "0199a213-81c0-7800-8aa1-bbab2a035a53"
    assert result.usage == {}


def test_codex_adapter_fails_turn_without_reply_or_usage() -> None:
    usage = {"input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0}
    payload = b"\n".join(
        (
            json.dumps({"type": "thread.started", "thread_id": "session-1"}).encode(),
            json.dumps({"type": "turn.started"}).encode(),
            json.dumps({"type": "turn.completed", "usage": usage}).encode(),
        )
    )

    result = CodexAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.FAILED
    assert result.session_id == "session-1"
    assert result.error_message == "codex turn completed without a reply or token usage"


def test_codex_adapter_preserves_blocked_failure() -> None:
    payload = b"\n".join(
        (
            json.dumps({"type": "thread.started", "thread_id": "session-1"}).encode(),
            json.dumps(
                {
                    "type": "turn.failed",
                    "error": {"message": "Approval required for command execution"},
                }
            ).encode(),
        )
    )

    result = CodexAdapter().parse(1, payload, b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.session_id == "session-1"
    assert result.error_message == "Approval required for command execution"


def test_codex_adapter_marks_rejected_credentials_blocked() -> None:
    payload = json.dumps(
        {
            "type": "turn.failed",
            "error": {
                "message": "unexpected status 401 Unauthorized: Missing bearer authentication"
            },
        }
    ).encode()

    assert CodexAdapter().parse(1, payload, b"").status is InvocationStatus.BLOCKED


def test_claude_logged_out_result_is_blocked() -> None:
    payload = {
        "type": "result",
        "subtype": "success",
        "is_error": True,
        "result": "Not logged in · Please run /login",
    }

    result = ClaudeAdapter().parse(1, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED


def test_codex_adapter_preserves_standalone_error() -> None:
    payload = json.dumps({"type": "error", "message": "Authentication required"}).encode()

    result = CodexAdapter().parse(1, payload, b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == "Authentication required"


def test_codex_adapter_requires_a_terminal_turn_event() -> None:
    payload = json.dumps(
        {"type": "item.completed", "item": {"type": "agent_message", "text": "partial"}}
    ).encode()

    result = CodexAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.FAILED
    assert result.output == "partial"
    assert result.error_message == "codex stream ended without a completion event"


def test_provider_registry_returns_codebuddy_adapter() -> None:
    adapter = get_adapter("codebuddy")

    assert isinstance(adapter, CodeBuddyAdapter)


def test_provider_registry_returns_codex_adapter() -> None:
    adapter = get_adapter("codex")

    assert isinstance(adapter, CodexAdapter)


@pytest.mark.parametrize(
    ("adapter", "stdout", "expected_output", "expected_session"),
    [
        (
            ClaudeAdapter(),
            (
                b'{"type":"result","result":"claude answer",'
                b'"session_id":"claude-session","num_turns":{"invalid":true},'
                b'"total_cost_usd":' + b"1" * 5001 + b',"is_error":false}'
            ),
            "claude answer",
            "claude-session",
        ),
        (
            CodeBuddyAdapter(),
            (
                b'{"type":"result","result":"codebuddy answer",'
                b'"session_id":"codebuddy-session","num_turns":{"invalid":true},'
                b'"total_cost_usd":' + b"1" * 5001 + b',"is_error":false}'
            ),
            "codebuddy answer",
            "codebuddy-session",
        ),
        (
            AntigravityAdapter(),
            json.dumps(
                {
                    "event": "result",
                    "result": {
                        "conversation_id": "antigravity-session",
                        "status": "SUCCESS",
                        "response": "antigravity answer",
                        "num_turns": {"invalid": True},
                        "total_cost_usd": [0.1],
                    },
                }
            ).encode(),
            "antigravity answer",
            "antigravity-session",
        ),
        (
            GrokAdapter(),
            b"\n".join(
                (
                    json.dumps({"type": "text", "data": "grok answer"}).encode(),
                    json.dumps(
                        {
                            "type": "end",
                            "stopReason": "end_turn",
                            "sessionId": "grok-session",
                            "num_turns": {"invalid": True},
                            "total_cost_usd": [0.1],
                        }
                    ).encode(),
                )
            ),
            "grok answer",
            "grok-session",
        ),
        (
            DeepSeekAdapter(),
            b"\n".join(
                json.dumps(event).encode()
                for event in (
                    {"type": "session", "sessionId": "deepseek-session", "cwd": "/w"},
                    {"type": "text", "text": "intermediate"},
                    {"type": "status", "phase": "step_end", "usage": {"inputTokens": [1]}},
                    {"type": "status", "phase": "turn_end", "reason": {"kind": "completed"}},
                    {"type": "final", "text": "deepseek answer"},
                )
            ),
            "deepseek answer",
            "deepseek-session",
        ),
    ],
)
def test_adapters_drop_malformed_usage_without_losing_terminal_result(
    adapter: ClaudeAdapter | CodeBuddyAdapter | AntigravityAdapter | GrokAdapter | DeepSeekAdapter,
    stdout: bytes,
    expected_output: str,
    expected_session: str,
) -> None:
    result = adapter.parse(0, stdout, b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == expected_output
    assert result.session_id == expected_session
    assert result.usage == {}


def test_claude_permission_denials_block_only_a_failed_run(tmp_path: Path) -> None:
    # Claude 2.1.308 reports subtype=success when the model finishes after a denied tool call.
    payload = {
        "type": "result",
        "subtype": "success",
        "result": "FINISHED",
        "session_id": "session-2",
        "is_error": False,
        "permission_denials": [{"tool": "Bash"}],
    }

    succeeded = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")
    failed = ClaudeAdapter().parse(1, json.dumps({**payload, "is_error": True}).encode(), b"")

    assert succeeded.status is InvocationStatus.SUCCEEDED
    assert succeeded.output == "FINISHED"
    assert failed.status is InvocationStatus.BLOCKED
    assert failed.output == "FINISHED"
    # Success stays success, with a warning that names the denied tools.
    assert len(succeeded.warnings) == 1
    assert "denied 1 tool call(s)" in succeeded.warnings[0]
    clean = ClaudeAdapter().parse(
        0, json.dumps({**payload, "permission_denials": []}).encode(), b""
    )
    assert clean.warnings == ()
    # CodeBuddy only reports the denial in the tool result; the capture keeps a marker of it.
    capture = CodeBuddyAdapter().prepare(request(tmp_path, "codebuddy")).capture_line
    assert capture is not None
    text = "Error: Permission to use Bash has been denied because this tool requires approval"
    denial = {
        "type": "user",
        "parent_tool_use_id": None,
        "message": {
            "content": [{"type": "tool_result", "content": [{"type": "text", "text": text}]}]
        },
    }
    captured = capture(json.dumps(denial).encode()) + capture(
        json.dumps({**payload, "permission_denials": []}).encode()
    )
    warned = CodeBuddyAdapter().parse(0, captured, b"")
    assert warned.status is InvocationStatus.SUCCEEDED
    assert "codebuddy denied 1 tool call(s) (Bash)" in warned.warnings[0]


def test_claude_structured_error_preserves_usage_and_message() -> None:
    payload = {
        "type": "result",
        "subtype": "error_max_turns",
        "errors": ["maximum turns reached " + "x" * 10_000],
        "usage": {"output_tokens": 10},
        "num_turns": 4,
    }

    result = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.FAILED
    assert result.error_message is not None
    assert "maximum turns reached" in result.error_message
    assert len(result.error_message) == 512  # a long provider error is summarized, bounded
    assert result.usage == {"num_turns": 4}


def test_claude_api_error_uses_output_instead_of_success_subtype() -> None:
    payload = {
        "type": "result",
        "subtype": "success",
        "is_error": True,
        "result": "API Error: 500 Internal server error",
        "session_id": "session-error",
    }

    result = ClaudeAdapter().parse(1, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.FAILED
    assert result.output == "API Error: 500 Internal server error"
    assert result.error_message == "API Error: 500 Internal server error"
    assert result.session_id == "session-error"


def test_claude_non_success_subtype_still_describes_partial_output() -> None:
    payload = {
        "type": "result",
        "subtype": "error_max_turns",
        "is_error": True,
        "result": "partial",
    }

    result = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.FAILED
    assert result.error_message == "error_max_turns"


@pytest.mark.parametrize("filtered", [False, True])
def test_claude_incomplete_stream_preserves_session_without_partial_output(
    tmp_path: Path, filtered: bool
) -> None:
    lines = (
        json.dumps({"type": "system", "subtype": "init", "session_id": "session-partial"}).encode(),
        json.dumps({"type": "stream_event", "event": {"type": "content_block_delta"}}).encode(),
    )
    capture = ClaudeAdapter().prepare(InvocationRequest("claude", "work", tmp_path)).capture_line
    assert capture is not None
    payload = b"".join(map(capture, lines)) if filtered else b"\n".join(lines)

    result = ClaudeAdapter().parse(1, payload, b"")

    assert result.status is InvocationStatus.FAILED
    assert result.output == ""
    assert result.session_id == "session-partial"


def test_claude_refusal_is_blocked() -> None:
    payload = {
        "type": "result",
        "subtype": "success",
        "is_error": False,
        "stop_reason": "refusal",
        "result": "cannot comply",
    }

    result = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.output == "cannot comply"


def test_antigravity_adapter_passes_prompt_session_and_explicit_options(tmp_path: Path) -> None:
    prepared = AntigravityAdapter().prepare(
        request(
            tmp_path,
            "antigravity",
            session_id="conversation-1",
            provider_options={"model": "gemini-3.7-flash-high", "sandbox": True},
        )
    )

    assert prepared.argv == (
        "agy",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--conversation",
        "conversation-1",
        "--model",
        "gemini-3.7-flash-high",
        "--sandbox=true",
    )
    assert prepared.stdin is not None
    assert json.loads(prepared.stdin) == {
        "event": "user",
        "message": {"content": "do the task"},
    }


def test_antigravity_adapter_parses_documented_json_contract() -> None:
    payload = {
        "conversation_id": "conversation-2",
        "status": "SUCCESS",
        "response": "finished\n",
        "duration_seconds": 3.5,
        "num_turns": 2,
        "usage": {"total_tokens": 42},
    }

    event = {"event": "result", "result": payload}
    result = AntigravityAdapter().parse(0, json.dumps(event).encode(), b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "finished\n"
    assert result.session_id == "conversation-2"
    assert result.usage == {"num_turns": 2}


def test_antigravity_waiting_status_is_blocked() -> None:
    payload = {
        "conversation_id": "conversation-3",
        "status": "WAITING",
        "response": "",
    }

    event = {"event": "result", "result": payload}
    result = AntigravityAdapter().parse(0, json.dumps(event).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == "antigravity stopped with status WAITING"


def test_antigravity_authentication_error_is_blocked() -> None:
    payload = {
        "conversation_id": "",
        "status": "ERROR",
        "response": "",
        "error": "authentication required",
    }

    event = {"event": "result", "result": payload}
    result = AntigravityAdapter().parse(1, json.dumps(event).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.session_id is None
    assert result.error_message == "authentication required"


def test_antigravity_model_comes_from_its_run_log_for_that_conversation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from datetime import UTC, datetime, timedelta

    monkeypatch.setattr(Path, "home", staticmethod(lambda: tmp_path))
    logs = tmp_path / ".gemini" / "antigravity-cli" / "log"
    logs.mkdir(parents=True)
    label = 'Propagating selected model override to backend: label="{}"\n'
    (logs / "cli-1.log").write_text(
        label.format("Gemini 3.8 Flash (High)") + "Print mode: conversation=c1, sending message\n"
    )
    (logs / "cli-2.log").write_text(
        label.format("Gemini 3.8 Flash (Low)")
        + label.format("Gemini 3.8 Pro")
        + "Print mode: conversation=c2, sending message\n"
    )
    since = datetime.now(UTC) - timedelta(minutes=1)
    assert session_model("c1", since=since) == "gemini-3.8-flash-high"
    assert session_model("c2", since=since) is None  # two models in one run is ambiguous
    assert session_model("c3", since=since) is None
    assert session_model("c1", since=datetime.now(UTC) + timedelta(hours=1)) is None


def test_antigravity_result_falls_back_to_stream_session_id() -> None:
    stdout = b"\n".join(
        (
            json.dumps({"event": "init", "conversation_id": "conversation-partial"}).encode(),
            json.dumps(
                {
                    "event": "result",
                    "result": {
                        "conversation_id": "",
                        "status": "ERROR",
                        "response": "",
                        "error": "provider failed",
                    },
                }
            ).encode(),
        )
    )

    result = AntigravityAdapter().parse(1, stdout, b"")

    assert result.session_id == "conversation-partial"


def test_antigravity_result_uses_stderr_before_generic_status_message() -> None:
    stdout = json.dumps(
        {
            "event": "result",
            "result": {
                "conversation_id": "conversation-4",
                "status": "ERROR",
                "response": "",
            },
        }
    ).encode()

    result = AntigravityAdapter().parse(1, stdout, b"authentication required")

    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == "authentication required"


def test_antigravity_soft_permission_denial_is_blocked() -> None:
    stdout = json.dumps(
        {
            "event": "result",
            "result": {
                "conversation_id": "conversation-5",
                "status": "SUCCESS",
                "response": "",
            },
        }
    ).encode()

    notice = (
        b"a tool required the write_file permission that headless mode cannot prompt for, "
        b"so it was auto-denied"
    )
    result = AntigravityAdapter().parse(0, stdout, notice)

    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == notice.decode()


@pytest.mark.parametrize(
    ("last_step", "status"),
    [
        ({"step_type": "agent_response", "state": "DONE"}, InvocationStatus.SUCCEEDED),
        ({"step_type": "unknown", "state": "DONE"}, InvocationStatus.FAILED),
    ],
)
def test_antigravity_recovered_error_succeeds_only_after_final_reply(
    tmp_path: Path, last_step: dict[str, object], status: InvocationStatus
) -> None:
    stale_error = "API error (attempt 1): request failed: EOF"
    tool_step = {"step_type": "tool", "state": "DONE", "tool_info": {"output": "hidden" * 9}}
    lines = (
        json.dumps({"event": "step_update", "step_update": tool_step}).encode(),
        json.dumps(
            {
                "event": "step_update",
                "step_update": {
                    "step_type": "agent_response",
                    "state": "DONE",
                    "text_delta": "OK",
                },
            }
        ).encode(),
        json.dumps({"event": "step_update", "step_update": last_step}).encode(),
        json.dumps(
            {
                "event": "result",
                "result": {
                    "conversation_id": "conversation-6",
                    "status": "ERROR",
                    "response": "OK\n",
                    "error": stale_error,
                },
            }
        ).encode(),
    )
    capture = AntigravityAdapter().prepare(request(tmp_path, "antigravity")).capture_line
    assert capture is not None
    stdout = b"".join(capture(line) for line in lines)
    assert b"hidden" not in stdout

    result = AntigravityAdapter().parse(0, stdout, b"")

    assert result.status is status
    assert result.output == "OK\n"
    assert result.session_id == "conversation-6"
    assert result.error_message == (None if status is InvocationStatus.SUCCEEDED else stale_error)


def test_antigravity_capture_keeps_session_after_result(tmp_path: Path) -> None:
    events: list[dict[str, object]] = [
        {"event": "init", "conversation_id": "conversation-b"},
        {
            "event": "result",
            "conversation_id": "conversation-a",
            "result": {"status": "SUCCESS", "response": "OK"},
        },
        {"event": "step_update", "conversation_id": "conversation-b", "step_update": {}},
    ]
    lines = [json.dumps(event).encode() for event in events]
    capture = AntigravityAdapter().prepare(request(tmp_path, "antigravity")).capture_line
    assert capture is not None

    raw = AntigravityAdapter().parse(0, b"\n".join(lines), b"")
    captured = AntigravityAdapter().parse(0, b"".join(capture(line) for line in lines), b"")

    assert captured.session_id == raw.session_id == "conversation-b"


def test_antigravity_stream_without_result_is_failed() -> None:
    payload = b"\n".join(
        (
            json.dumps({"event": "init", "conversation_id": "conversation-partial"}).encode(),
            json.dumps({"event": "step_update", "step_update": {"text_delta": "partial"}}).encode(),
        )
    )

    result = AntigravityAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.FAILED
    assert result.output == ""
    assert result.session_id == "conversation-partial"
    assert result.error_message == "antigravity stream ended without a valid result event"


@pytest.mark.parametrize("option", ["sandbox", "dangerously_skip_permissions"])
def test_antigravity_permission_option_requires_boolean(tmp_path: Path, option: str) -> None:
    with pytest.raises(ValueError, match=f"{option} must be boolean"):
        AntigravityAdapter().prepare(
            request(tmp_path, "antigravity", provider_options={option: "true"})
        )


@pytest.mark.parametrize("value", [None, False, True])
def test_antigravity_yolo_requires_explicit_option(tmp_path: Path, value: bool | None) -> None:
    options: dict[str, str | int | float | bool] = (
        {} if value is None else {"dangerously_skip_permissions": value}
    )
    prepared = AntigravityAdapter().prepare(
        request(tmp_path, "antigravity", provider_options=options)
    )
    flags = [arg for arg in prepared.argv if arg.startswith("--dangerously-skip-permissions")]
    assert flags == (
        [] if value is None else [f"--dangerously-skip-permissions={str(value).lower()}"]
    )


def test_grok_adapter_uses_private_prompt_file_and_explicit_options(tmp_path: Path) -> None:
    prepared = GrokAdapter().prepare(
        request(tmp_path, "grok", provider_options={"effort": "high", "max_turns": 8})
    )
    prompt_path = prepared.cleanup_paths[0]
    try:
        assert prompt_path.read_text() == "do the task"
        if os.name == "posix":
            assert stat.S_IMODE(prompt_path.stat().st_mode) == 0o600
        assert prepared.argv[:7] == (
            "grok",
            "--prompt-file",
            str(prompt_path),
            "--output-format",
            "streaming-json",
            "--verbatim",
            "--cwd",
        )
        assert prepared.argv[-4:] == ("--reasoning-effort", "high", "--max-turns", "8")
        # A new conversation is named up front, so a cancelled run can still be continued.
        assert prepared.session_id is not None
        assert str(uuid.UUID(prepared.session_id)) == prepared.session_id
        assert f"--session-id={prepared.session_id}" in prepared.argv
        resumed = GrokAdapter().prepare(request(tmp_path, "grok", session_id="s-old"))
        resumed.cleanup_paths[0].unlink()
        assert "--resume=s-old" in resumed.argv and resumed.session_id == "s-old"
        assert not any(item.startswith("--session-id") for item in resumed.argv)
    finally:
        prompt_path.unlink()


def test_grok_adapter_returns_only_final_text_block() -> None:
    payload = b"\n".join(
        (
            json.dumps({"type": "thought", "data": "working"}).encode(),
            json.dumps({"type": "text", "data": "intermediate narration"}).encode(),
            json.dumps({"type": "usage", "usage": {"total_tokens": 10}}).encode(),
            json.dumps({"type": "text", "data": "fin"}).encode(),
            json.dumps({"type": "text", "data": "ished"}).encode(),
            json.dumps(
                {
                    "type": "end",
                    "stopReason": "end_turn",
                    "sessionId": "session-3",
                    "usage": {"total_tokens": 42},
                    "num_turns": 2,
                    "total_cost_usd": 0.01,
                }
            ).encode(),
        )
    )

    result = GrokAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "finished"
    assert result.session_id == "session-3"
    assert result.usage == {"num_turns": 2, "total_cost_usd": 0.01}


def test_grok_adapter_does_not_report_max_turns_as_success() -> None:
    payload = b"\n".join(
        (
            json.dumps({"type": "text", "data": "partial"}).encode(),
            json.dumps(
                {"type": "end", "stopReason": "MaxTurns", "sessionId": "session-4"}
            ).encode(),
        )
    )

    result = GrokAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.FAILED
    assert result.output == "partial"
    assert result.error_message == "grok stopped with reason MaxTurns"


def test_grok_incomplete_stream_does_not_return_intermediate_text() -> None:
    payload = b"\n".join(
        (
            json.dumps({"type": "thought", "data": "working"}).encode(),
            json.dumps({"type": "text", "data": "intermediate narration"}).encode(),
        )
    )

    result = GrokAdapter().parse(1, payload, b"")

    assert result.status is InvocationStatus.FAILED
    assert result.output == ""


def test_grok_documented_error_object_preserves_blocked_reason() -> None:
    payload = {"type": "error", "message": "Authentication required"}

    result = GrokAdapter().parse(1, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == "Authentication required"


def test_grok_refusal_is_blocked() -> None:
    payload = b"\n".join(
        (
            json.dumps({"type": "text", "data": "cannot comply"}).encode(),
            json.dumps({"type": "end", "stopReason": "refusal"}).encode(),
        )
    )

    result = GrokAdapter().parse(0, payload, b"")

    assert result.status is InvocationStatus.BLOCKED


def test_grok_invalid_prompt_encoding_does_not_create_temp_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def unexpected_temp_file(*_args: object, **_kwargs: object) -> tuple[int, str]:
        pytest.fail("temp file created before prompt encoding")

    monkeypatch.setattr("agentnave.adapters.grok.tempfile.mkstemp", unexpected_temp_file)
    invalid = InvocationRequest("grok", "\ud800", tmp_path)

    with pytest.raises(UnicodeEncodeError):
        GrokAdapter().prepare(invalid)


def test_adapter_rejects_implicit_or_unknown_provider_overrides(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="unsupported claude options"):
        ClaudeAdapter().prepare(
            request(tmp_path, "claude", provider_options={"dangerously_skip_permissions": True})
        )

    with pytest.raises(ValueError, match="unsupported antigravity options"):
        AntigravityAdapter().prepare(
            request(
                tmp_path,
                "antigravity",
                provider_options={"unsupported_permission_option": True},
            )
        )

    with pytest.raises(ValueError, match="unsupported codebuddy options"):
        CodeBuddyAdapter().prepare(
            request(
                tmp_path,
                "codebuddy",
                provider_options={"max_turns": 8},
            )
        )

    with pytest.raises(ValueError, match="unsupported codex options"):
        CodexAdapter().prepare(
            request(tmp_path, "codex", provider_options={"approval_policy": "never"})
        )

    with pytest.raises(ValueError, match="codex sandbox must be one of"):
        CodexAdapter().prepare(request(tmp_path, "codex", provider_options={"sandbox": "full"}))


@pytest.mark.parametrize(
    ("provider", "event", "state", "tool"),
    [
        (
            "claude",
            {
                "type": "stream_event",
                "event": {
                    "type": "content_block_start",
                    "content_block": {
                        "type": "tool_use",
                        "id": "t1",
                        "name": "Bash",
                        "input": {"secret": "hidden"},
                    },
                },
            },
            "started",
            "Bash",
        ),
        (
            "codebuddy",
            {
                "type": "user",
                "message": {
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "t1",
                            "content": "hidden",
                            "is_error": True,
                        }
                    ]
                },
            },
            "failed",
            None,
        ),
        (
            "codex",
            {
                "type": "item.started",
                "item": {
                    "id": "t1",
                    "type": "command_execution",
                    "status": "in_progress",
                    "command": "hidden",
                },
            },
            "in_progress",
            "command_execution",
        ),
        (
            "grok",
            {
                "type": "tool_call",
                "toolCallId": "t1",
                "toolName": "run_terminal_command",
                "status": "pending",
                "rawInput": "hidden",
            },
            "pending",
            "run_terminal_command",
        ),
        (
            "antigravity",
            {
                "event": "step_update",
                "step_update": {
                    "step_index": 1,
                    "step_type": "tool",
                    "state": "ACTIVE",
                    "tool_name": "run_command",
                    "tool_info": {"parameters": "hidden"},
                },
            },
            "ACTIVE",
            "run_command",
        ),
        (
            "deepseek",
            {"type": "tool_call", "callId": "c1", "tool": "bash", "input": {"command": "hidden"}},
            "running",
            "bash",
        ),
    ],
)
def test_activity_projects_native_tool_metadata_without_inputs(
    provider: str, event: dict[str, object], state: str, tool: str | None
) -> None:
    activity = get_adapter(provider).activity(event)
    assert activity is not None
    assert activity.kind == "tool"
    assert activity.state == state
    assert activity.tool_name == tool
    assert activity.tool_call_id is not None
    assert "hidden" not in json.dumps(activity.to_dict())


def test_activity_omits_thinking_and_unknown_payloads_and_bounds_public_text() -> None:
    claude = ClaudeAdapter()
    assert (
        claude.activity(
            {
                "type": "stream_event",
                "event": {"delta": {"type": "thinking_delta", "thinking": "hidden"}},
            }
        )
        is None
    )
    thought = GrokAdapter().activity({"type": "thought", "data": "hidden"})
    assert thought is not None and thought.message is None
    for provider in ("claude", "codebuddy", "codex", "grok", "antigravity"):
        adapter = get_adapter(provider)
        assert adapter.activity({"type": ["unknown"], "event": ["unknown"]}) is None
        assert adapter.activity({"type": "future_event", "data": "hidden"}) is None
    text = GrokAdapter().activity({"type": "text", "data": "x" * 1000})
    assert text is not None and text.message == "x" * 512


@pytest.mark.parametrize("provider", ["codex", "grok"])
def test_terminal_result_survives_invalid_event_discriminators(provider: str) -> None:
    events: list[dict[str, object]]
    if provider == "codex":
        events = [
            {"type": "thread.started", "thread_id": "session-1"},
            {"type": "item.completed", "item": {"type": "agent_message", "text": "done"}},
            {"type": "turn.completed"},
        ]
    else:
        events = [
            {"type": "text", "data": "done"},
            {"type": "end", "sessionId": "session-1"},
        ]
    events.append({"type": []})
    events.append({"type": {}})
    result = get_adapter(provider).parse(
        0, "\n".join(json.dumps(event) for event in events).encode(), b""
    )
    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == "done"
    assert result.session_id == "session-1"
    failed = get_adapter(provider).parse(1, b'{"type": []}\n{"type": {}}', b"")
    assert failed.status is InvocationStatus.FAILED


def test_pi_transport_and_explicit_options(tmp_path: Path) -> None:
    adapter = get_adapter("pi")
    command = adapter.prepare(request(tmp_path, "pi"))
    assert command.argv == ("pi", "--print", "--mode", "json")
    assert command.stdin == b"do the task"
    resumed = adapter.prepare(
        request(
            tmp_path,
            "pi",
            session_id="native-session",
            provider_options={"model": "openai/example", "effort": "high"},
        )
    )
    assert resumed.argv == (
        "pi",
        "--print",
        "--mode",
        "json",
        "--session",
        "native-session",
        "--model",
        "openai/example",
        "--thinking",
        "high",
    )
    assert resumed.stdin == b"\ndo the task"
    with pytest.raises(ValueError, match="unsupported pi options"):
        adapter.prepare(request(tmp_path, "pi", provider_options={"api_key": "secret"}))


def test_pi_filtered_capture_preserves_only_final_reply_and_session(tmp_path: Path) -> None:
    adapter = get_adapter("pi")
    command = adapter.prepare(request(tmp_path, "pi"))
    assert command.capture_line is not None
    events = [
        {"type": "session", "id": "native-session"},
        {
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "working"},
                    {"type": "toolCall", "arguments": {"secret": "hidden"}},
                ],
                "stopReason": "toolUse",
            },
        },
        {
            "type": "tool_execution_end",
            "result": {"content": [{"type": "image", "data": "x" * 10000}]},
        },
        {
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "hidden"},
                    {"type": "text", "text": "final answer\u2028next\u2029line\u0085end"},
                ],
                "stopReason": "stop",
                "usage": {"cost": {"total": 0.25}},
            },
        },
        {"type": "agent_end", "messages": [{"secret": "hidden"}]},
        {"type": "agent_settled", "aborted": False},
    ]
    stream = b"".join(command.capture_line(json.dumps(event).encode()) for event in events)
    assert b"hidden" not in stream and b"image" not in stream
    result = adapter.parse(0, stream, b"")
    assert result.status is InvocationStatus.SUCCEEDED
    assert (
        result.output == "final answer\u2028next\u2029line\u0085end"
        and result.session_id == "native-session"
    )
    assert result.usage == {"total_cost_usd": 0.25}


def test_pi_foreign_session_reports_cwd_instead_of_forking() -> None:
    stderr = b"Session found in different project: /other\nFork? [y/N] Aborted.\n"
    result = get_adapter("pi").parse(0, b"", stderr)
    assert result.status is InvocationStatus.FAILED
    assert result.error_message is not None and "/other" in result.error_message


@pytest.mark.parametrize(
    ("reason", "complete"), [("stop", False), ("length", True), ("toolUse", True), ("error", True)]
)
def test_pi_does_not_treat_incomplete_or_failed_reply_as_success(
    reason: str, complete: bool
) -> None:
    events = [
        {"type": "session", "id": "native-session"},
        {
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "partial"}],
                "stopReason": reason,
            },
        },
    ]
    if complete:
        events.append({"type": "agent_end"})
    result = get_adapter("pi").parse(
        0, "\n".join(json.dumps(event) for event in events).encode(), b""
    )
    assert result.status is InvocationStatus.FAILED
    assert result.session_id == "native-session"


@pytest.mark.parametrize(
    "error",
    [
        b"Authentication required: run /login",
        b"No API key for openai/example",
        b"No models available. Use /login to log into a provider",
    ],
)
def test_pi_authentication_failure_is_blocked(error: bytes) -> None:
    result = get_adapter("pi").parse(1, b"", error)
    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == error.decode()


def test_pi_activity_exposes_text_and_tool_names_without_private_payloads() -> None:
    adapter = get_adapter("pi")
    delta = adapter.activity(
        {
            "type": "message_update",
            "assistantMessageEvent": {"type": "text_delta", "delta": "hello"},
        }
    )
    assert delta is not None and delta.public_output == "hello" and delta.message_delta
    assert (
        adapter.activity(
            {
                "type": "message_update",
                "assistantMessageEvent": {"type": "thinking_delta", "delta": "hidden"},
            }
        )
        is None
    )
    tool = adapter.activity(
        {
            "type": "tool_execution_end",
            "toolName": "read",
            "toolCallId": "t1",
            "result": {"secret": "hidden"},
            "isError": True,
        }
    )
    assert tool is not None and tool.tool_name == "read" and tool.blocking_error is None
    assert "hidden" not in json.dumps(tool.to_dict())


def test_pi_retryable_message_error_does_not_signal_execution_blocker() -> None:
    activity = get_adapter("pi").activity(
        {
            "type": "message_end",
            "message": {"role": "assistant", "stopReason": "error", "errorMessage": "overloaded"},
        }
    )
    assert activity is not None and activity.blocking_error is None


@pytest.mark.parametrize(
    "provider", ["claude", "grok", "codebuddy", "codex", "antigravity", "pi", "deepseek"]
)
def test_restricted_profiles_resume_without_enabling_tools(tmp_path: Path, provider: str) -> None:
    adapter = get_adapter(provider)
    restricted = InvocationRequest(
        provider,
        "speak",
        tmp_path,
        session_id="own-seat-session",
        provider_options={"restricted_tools": True, "model": "explicit"},
    )
    if provider == "antigravity":
        with pytest.raises(ValueError, match="dedicated cwd"):
            adapter.prepare(restricted)
        assert not (tmp_path / ".agents").exists()
        write_restricted_profile(tmp_path)
    command = adapter.prepare(restricted)
    try:
        argv = command.argv
        if provider in ("claude", "grok", "codebuddy"):
            assert "--resume=own-seat-session" in argv
            assert argv[argv.index("--tools") + 1] == ""
        assert "--no-session-persistence" not in argv
        if provider == "claude":
            assert {"--safe-mode", "--strict-mcp-config", "--disable-slash-commands"} <= set(argv)
        elif provider == "grok":
            assert argv[argv.index("--deny") + 1] == "*"
            assert {"--no-subagents", "--disable-web-search"} <= set(argv)
        elif provider == "codebuddy":
            assert "--safe-mode" not in argv
            assert "--strict-mcp-config" in argv
            assert argv[argv.index("--mcp-config") + 1] == '{"mcpServers":{}}'
            assert argv[argv.index("--disallowedTools") + 1] == "mcp__*"
        elif provider == "codex":
            assert "resume" in argv and "own-seat-session" in argv
            assert {
                "--ignore-user-config",
                "--skip-git-repo-check",
                "shell_tool",
                "unified_exec",
                "view_image",
                "apps",
                "multi_agent",
            } <= set(argv)
            assert "--ephemeral" not in argv
        elif provider == "pi":
            assert argv[argv.index("--session") + 1] == "own-seat-session"
            assert {
                "--no-tools",
                "--no-extensions",
                "--no-mcp",
                "--no-skills",
                "--no-prompt-templates",
            } <= set(argv)
        elif provider == "deepseek":
            assert argv[argv.index("--session-id") + 1] == "own-seat-session"
            patch = Path(argv[argv.index("--patch") + 1]).read_text()
            assert "- id: tool-bash\n  disabled: true\n" in patch
            assert "- id: plan-mode\n  disabled: true\n" in patch
            assert "    mode: read-only\n" in patch
        else:
            assert argv[argv.index("--conversation") + 1] == "own-seat-session"
            assert {"--sandbox=true", "--disable-slash-commands=true"} <= set(argv)
            profile = tmp_path / ".agents" / "agents" / "agentnave-discussion.md"
            assert "tools: []" in profile.read_text() and "mcpServers: []" in profile.read_text()
    finally:
        for path in command.cleanup_paths:
            path.unlink()
    with pytest.raises(ValueError, match="only model and effort"):
        adapter.prepare(
            InvocationRequest(
                provider,
                "speak",
                tmp_path,
                provider_options={"restricted_tools": True, "permission_mode": "bypassPermissions"},
            )
        )


def test_restricted_pi_keeps_only_its_model_backend(tmp_path: Path) -> None:
    options: dict[str, ProviderOption] = {"restricted_tools": True, "model": "m", "provider": "p"}
    argv = get_adapter("pi").prepare(InvocationRequest("pi", "speak", tmp_path, None, options)).argv
    assert argv[argv.index("--provider") + 1] == "p" and "--no-tools" in argv
    with pytest.raises(ValueError, match="only model and effort"):
        get_adapter("codex").prepare(InvocationRequest("codex", "speak", tmp_path, None, options))


def test_deepseek_options_become_a_temporary_patch_overlay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    adapter = DeepSeekAdapter()

    def configured(_executable: str) -> tuple[tuple[str, str], dict[str, str]]:
        return ("gw", "gw/flash"), {"gw/flash": "gw", "gw/pro": "gw2"}

    monkeypatch.setattr(deepseek_adapter, "profile_routes", configured)
    plain = adapter.prepare(request(tmp_path, "deepseek", session_id="session-1"))
    # argv[0] may be the desktop bundle on this machine; resolution is covered elsewhere.
    assert plain.argv[1:] == (
        "--profile",
        "headless",
        "--json",
        "--session-id",
        "session-1",
    )
    assert plain.stdin == b"do the task" and plain.cleanup_paths == ()

    def patch_of(options: dict[str, ProviderOption]) -> str:
        command = adapter.prepare(request(tmp_path, "deepseek", provider_options=options))
        try:
            return Path(command.argv[command.argv.index("--patch") + 1]).read_text()
        finally:
            for path in command.cleanup_paths:
                path.unlink()

    # The default model and gateway models keep their route; the official route restores the
    # app's saved keys unless the caller says otherwise.
    assert '    provider: "gw"\n    model: "gw/flash"\n' in patch_of({"model": "gw/flash"})
    # App defaults can supply effort without pinning a model in the request. dsh's
    # patch still needs the live native identity because its config is replaced whole.
    assert (
        adapter.option_args(request(tmp_path, "deepseek", provider_options={"effort": "high"}))
        == ()
    )
    native_effort = patch_of({"effort": "high"})
    assert (
        '    provider: "gw"\n    model: "gw/flash"\n    reasoningEffort: "high"\n' in native_effort
    )
    assert "credentials" not in native_effort
    gateway = patch_of({"model": "gw/pro"})
    assert '    provider: "gw2"\n' in gateway and "credentials" not in gateway
    official = patch_of({"model": "deepseek-flash"})
    assert '    provider: "deepseek-official"\n' in official
    assert "- id: credentials\n  disabled: false\n" in official
    assert "credentials" not in patch_of({"model": "deepseek-flash", "stored_credentials": False})

    def official_default(_executable: str) -> tuple[tuple[str, str], dict[str, str]]:
        return ("deepseek-official", "deepseek-flash"), {}

    monkeypatch.setattr(deepseek_adapter, "profile_routes", official_default)
    bare = adapter.prepare(request(tmp_path, "deepseek"))
    try:
        assert (
            "- id: credentials\n  disabled: false\n"
            in Path(bare.argv[bare.argv.index("--patch") + 1]).read_text()
        )
    finally:
        for path in bare.cleanup_paths:
            path.unlink()

    command = adapter.prepare(
        request(
            tmp_path,
            "deepseek",
            provider_options={
                "model": 'pro"\nx',
                "effort": "max",
                "permission_mode": "danger-full-access",
                "stored_credentials": True,
            },
        )
    )
    try:
        argv = command.argv
        assert argv[-1] == "--json"
        patch = Path(argv[argv.index("--patch") + 1]).read_text()
        assert (
            '    provider: "deepseek-official"\n    model: "pro\\"\\nx"\n'
            '    reasoningEffort: "max"\n'
        ) in patch
        assert "    mode: danger-full-access\n" in patch and "    policy: never\n" in patch
        assert "- id: credentials\n  disabled: false\n" in patch
        assert "tool-bash" not in patch
    finally:
        for path in command.cleanup_paths:
            path.unlink()

    invalid: list[tuple[dict[str, ProviderOption], str]] = [
        ({"provider": "gw"}, "requires model"),
        ({"permission_mode": "yolo"}, "permission_mode must be"),
        ({"stored_credentials": "yes"}, "must be a boolean"),
        ({"agent": "x"}, "unsupported deepseek options: agent"),
    ]
    for options, message in invalid:
        with pytest.raises(ValueError, match=message):
            adapter.prepare(InvocationRequest("deepseek", "x", tmp_path, None, options))

    def unavailable(_executable: str) -> tuple[None, dict[str, str]]:
        return None, {}

    monkeypatch.setattr(deepseek_adapter, "profile_routes", unavailable)
    with pytest.raises(ValueError, match="Unable to read deepseek's default model"):
        adapter.prepare(request(tmp_path, "deepseek", provider_options={"effort": "high"}))


@pytest.mark.skipif(os.name == "nt", reason="POSIX fake executable")
def test_deepseek_profile_routes_read_the_composed_entries(tmp_path: Path) -> None:
    dump = tmp_path / "dsh"
    lines = (
        "# == @deepseek-ai/dsh-base",
        "- id: agent-default-model",
        "  name: x",
        "  config:",
        "    provider: deepseek-official",
        "    model: deepseek-flash",
        "# == user",
        "- id: agent-default-model",
        "  config:",
        "    provider: 'mag''pie'",
        '    model: \\"opencode-go/v4.1\\"',
        "    reasoningEffort: high",
        "- id: jobs",
        "  config:",
        "    model: other",
        "- id: llm-pi-ai",
        "  config:",
        "    providers:",
        "      gw:",
        "        baseURL: x",
        "        models:",
        "          - id: opencode-go/v4.1",
        "            reasoningEfforts:",
        "              high: high",
        "          - id: codex/sol",
        "      gw2:",
        "        models:",
        "          - id: codex/sol",
        "          - id: grok/4",
        "    other:",
        "      skip:",
        "          - id: not-a-model",
    )
    # printf is a shell builtin; these tests run without a PATH.
    dump.write_text("#!/bin/sh\nprintf '%s\\n' " + " ".join(f'"{line}"' for line in lines) + "\n")
    dump.chmod(0o755)
    assert deepseek_adapter.profile_routes(str(dump)) == (
        ("mag'pie", "opencode-go/v4.1"),
        {"opencode-go/v4.1": "gw", "codex/sol": "gw", "grok/4": "gw2"},
    )
    assert deepseek_adapter.profile_routes(str(tmp_path / "missing")) == (None, {})
    # A failed dump must not supply defaults even if it printed a partial config.
    dump.write_text(dump.read_text() + "exit 3\n")
    assert deepseek_adapter.profile_routes(str(dump)) == (None, {})
    dump.write_text(
        "#!/bin/sh\n/bin/cat <<'CONFIG'\n- id: agent-default-model\n  config:\n"
        "    provider: !!js String('deepseek-official')\n"
        "    model: !!js String('deepseek-flash')\nCONFIG\n"
    )
    assert deepseek_adapter.profile_routes(str(dump)) == (None, {})
    adapter = DeepSeekAdapter()
    adapter.executable = lambda: str(dump)
    with pytest.raises(ValueError, match="Unable to read deepseek's default model"):
        adapter.prepare(request(tmp_path, "deepseek", provider_options={"effort": "high"}))


def test_deepseek_parse_maps_turn_end_and_keeps_only_terminal_events(tmp_path: Path) -> None:
    adapter = DeepSeekAdapter()
    capture = adapter.prepare(request(tmp_path, "deepseek")).capture_line
    assert capture is not None
    lines = [
        {"type": "session", "sessionId": "session-1", "cwd": "/w"},
        {"type": "tool_result", "callId": "c1", "status": "completed", "result": "x" * 9000},
        {
            "type": "status",
            "phase": "turn_end",
            "turn": 1,
            "reason": {
                "kind": "error",
                "error": {"code": "MISSING_CREDENTIAL", "message": "no API key"},
            },
        },
        {"type": "final", "text": ""},
    ]
    captured = b"".join(capture(json.dumps(line).encode()) for line in lines)
    assert b"tool_result" not in captured and b'"turn"' not in captured
    result = adapter.parse(1, captured, b"dsh: MISSING_CREDENTIAL: no API key")
    assert result.status is InvocationStatus.BLOCKED
    assert result.session_id == "session-1" and result.error_message == "no API key"

    runner_error = json.dumps({"type": "error", "message": "unknown session id"}).encode()
    failed = adapter.parse(1, runner_error, b"")
    assert failed.status is InvocationStatus.FAILED
    assert failed.error_message == "unknown session id"
    incomplete = adapter.parse(0, json.dumps({"type": "final", "text": "partial"}).encode(), b"")
    assert incomplete.status is InvocationStatus.FAILED and incomplete.output == "partial"


def test_zcode_result_requires_terminal_and_preserves_session(tmp_path: Path) -> None:
    adapter = get_adapter("zcode")
    prepared = adapter.prepare(
        InvocationRequest(
            "zcode", "hello", tmp_path, provider_options={"model": "route/model", "effort": "low"}
        )
    )
    payload = json.loads(prepared.stdin or b"{}")
    assert payload["options"] == {"model": "route/model", "effort": "low"}
    assert prepared.session_id == payload["session_id"]
    assert "-I" in prepared.argv
    events = b'{"type":"session","session_id":"sess-known"}\n'
    assert adapter.parse(0, events, b"").status is InvocationStatus.FAILED
    events += b'{"type":"result","session_id":"sess-known","output":"answer","failed":false}\n'
    result = adapter.parse(0, events, b"")
    assert (result.status, result.output, result.session_id) == (
        InvocationStatus.SUCCEEDED,
        "answer",
        "sess-known",
    )
    assert adapter.parse(1, events, b"transport failed").status is InvocationStatus.FAILED

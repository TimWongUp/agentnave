from __future__ import annotations

import json
import stat
from pathlib import Path

import pytest

from agentnave.adapters import get_adapter
from agentnave.adapters.antigravity import AntigravityAdapter
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codebuddy import CodeBuddyAdapter
from agentnave.adapters.grok import GrokAdapter
from agentnave.models import InvocationRequest, InvocationStatus


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


def test_codebuddy_permission_denials_are_blocked() -> None:
    payload = {
        "type": "result",
        "subtype": "success",
        "result": "partial",
        "session_id": "session-5",
        "is_error": False,
        "permission_denials": [{"tool": "Bash"}],
    }

    result = CodeBuddyAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.error_message == "codebuddy requires permission for one or more tool calls"


def test_codebuddy_authentication_failure_is_blocked() -> None:
    payload = {
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": True,
        "errors": ["Authentication failed. Please use /login command to sign in to your account"],
    }

    result = CodeBuddyAdapter().parse(1, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED


def test_provider_registry_returns_codebuddy_adapter() -> None:
    adapter = get_adapter("codebuddy")

    assert isinstance(adapter, CodeBuddyAdapter)


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
    ],
)
def test_adapters_drop_malformed_usage_without_losing_terminal_result(
    adapter: ClaudeAdapter | CodeBuddyAdapter | AntigravityAdapter | GrokAdapter,
    stdout: bytes,
    expected_output: str,
    expected_session: str,
) -> None:
    result = adapter.parse(0, stdout, b"")

    assert result.status is InvocationStatus.SUCCEEDED
    assert result.output == expected_output
    assert result.session_id == expected_session
    assert result.usage == {}


def test_claude_permission_denials_are_blocked() -> None:
    payload = {
        "type": "result",
        "result": "partial",
        "session_id": "session-2",
        "is_error": False,
        "permission_denials": [{"tool": "Bash"}],
    }

    result = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.BLOCKED
    assert result.output == "partial"


def test_claude_structured_error_preserves_usage_and_message() -> None:
    payload = {
        "type": "result",
        "subtype": "error_max_turns",
        "errors": ["maximum turns reached"],
        "usage": {"output_tokens": 10},
        "num_turns": 4,
    }

    result = ClaudeAdapter().parse(0, json.dumps(payload).encode(), b"")

    assert result.status is InvocationStatus.FAILED
    assert result.error_message is not None
    assert "maximum turns reached" in result.error_message
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


def test_claude_incomplete_stream_preserves_session_without_partial_output() -> None:
    payload = b"\n".join(
        (
            json.dumps(
                {"type": "system", "subtype": "init", "session_id": "session-partial"}
            ).encode(),
            json.dumps({"type": "stream_event", "event": {"type": "content_block_delta"}}).encode(),
        )
    )

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


def test_antigravity_sandbox_option_requires_boolean(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="sandbox must be boolean"):
        AntigravityAdapter().prepare(
            request(tmp_path, "antigravity", provider_options={"sandbox": "true"})
        )


def test_grok_adapter_uses_private_prompt_file_and_explicit_options(tmp_path: Path) -> None:
    prepared = GrokAdapter().prepare(
        request(tmp_path, "grok", provider_options={"effort": "high", "max_turns": 8})
    )
    prompt_path = prepared.cleanup_paths[0]
    try:
        assert prompt_path.read_text() == "do the task"
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


def test_provider_error_message_is_bounded() -> None:
    result = ClaudeAdapter().parse(1, b"", b"secret" * 10_000)

    assert result.error_message == "claude stream ended without a result event"


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
                provider_options={"dangerously_skip_permissions": True},
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

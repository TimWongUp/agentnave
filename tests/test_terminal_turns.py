from __future__ import annotations

from pathlib import Path

import pytest

from agentnave.terminal_turns import antigravity_turn, claude_turn, codex_turn


def test_claude_stays_between_prompt_ids() -> None:
    entries: list[dict[str, object]] = [
        {"type": "user", "promptId": "a", "message": {"content": "first prompt"}},
        # Mid-turn notifications, meta rows, tool_results, and sidechain rows must not truncate turn "a".
        {
            "type": "user",
            "isMeta": True,
            "promptId": "meta-x",
            "message": {"content": "meta"},
        },
        {
            "type": "user",
            "message": {
                "content": "<task-notification><task-id>bg-1</task-id></task-notification>"
            },
        },
        {
            "type": "user",
            "promptId": "tool-other",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]},
        },
        {"type": "system", "subtype": "turn_duration", "isSidechain": True},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "first"}]}},
        {"type": "system", "subtype": "turn_duration"},
        # Background task continuation after turn_duration must not overwrite turn "a".
        {
            "type": "user",
            "message": {
                "content": "<task-notification><task-id>bg-2</task-id></task-notification>"
            },
        },
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "bg followup"}]}},
        {"type": "user", "promptId": "b", "message": {"content": "second prompt"}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "second"}]}},
    ]
    assert claude_turn(entries, "a") == "first"
    assert claude_turn(entries, "b") == "second"
    with pytest.raises(ValueError):
        claude_turn(entries, "missing")


def test_codex_requires_matching_turn() -> None:
    entries: list[dict[str, object]] = [
        {
            "type": "event_msg",
            "payload": {"type": "task_complete", "turn_id": "a", "last_agent_message": "first"},
        },
        {
            "type": "event_msg",
            "payload": {"type": "task_complete", "turn_id": "b", "last_agent_message": "second"},
        },
    ]
    assert codex_turn(entries, "a") == "first"
    with pytest.raises(ValueError):
        codex_turn(entries, "missing")


def test_antigravity_uses_explicit_input_step_and_does_not_cross_next_turn() -> None:
    entries: list[dict[str, object]] = [
        {"type": "USER_INPUT", "source": "USER_EXPLICIT", "step_index": 3},
        {"type": "PLANNER_RESPONSE", "content": "first"},
        {"type": "USER_INPUT", "source": "USER_EXPLICIT", "step_index": 8},
        {"type": "PLANNER_RESPONSE", "content": "second"},
    ]
    assert antigravity_turn(entries, 3) == "first"
    with pytest.raises(ValueError):
        antigravity_turn(entries, 4)


def test_read_turn_uses_codex_client_id(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import json

    from agentnave.terminal_turns import read_turn

    monkeypatch.setenv("CODEX_HOME", str(tmp_path))
    folder = tmp_path / "sessions/2026/01/01"
    folder.mkdir(parents=True)
    native_id = "00000000-0000-0000-0000-000000000001"
    events: list[dict[str, object]] = [
        {
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "turn_id": "turn",
                "item": {"type": "UserMessage", "client_id": "client"},
            },
        },
        {
            "type": "event_msg",
            "payload": {"type": "task_complete", "turn_id": "turn", "last_agent_message": "result"},
        },
    ]
    (folder / f"rollout-a-{native_id}.jsonl").write_text(
        "".join(json.dumps(event) + "\n" for event in events)
    )
    assert (
        read_turn("codex", None, native_id, {"turn_id": "turn", "client_user_message_id": "client"})
        == "result"
    )
    with pytest.raises(ValueError):
        read_turn("codex", None, native_id, {"turn_id": "turn", "client_user_message_id": "wrong"})


def test_read_turn_retries_codex_rollout_flush_and_partial_last_line(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json
    import threading
    import time

    from agentnave.terminal_turns import read_turn

    monkeypatch.setenv("CODEX_HOME", str(tmp_path))
    folder = tmp_path / "sessions/2026/01/01"
    folder.mkdir(parents=True)
    native_id = "00000000-0000-0000-0000-000000000002"
    rollout = folder / f"rollout-b-{native_id}.jsonl"
    user_line = json.dumps(
        {
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "turn_id": "turn-flush",
                "item": {"type": "UserMessage", "client_id": "client-flush"},
            },
        }
    )
    complete_line = json.dumps(
        {
            "type": "event_msg",
            "payload": {
                "type": "task_complete",
                "turn_id": "turn-flush",
                "last_agent_message": "flushed answer",
            },
        }
    )
    # Write a partial trailing line (no newline yet) to simulate in-flight rollout write.
    rollout.write_text(user_line + "\n" + complete_line[:20])

    def finish_flush() -> None:
        time.sleep(0.15)
        with rollout.open("a", encoding="utf-8") as stream:
            stream.write(complete_line[20:] + "\n")

    writer = threading.Thread(target=finish_flush)
    writer.start()
    try:
        assert (
            read_turn(
                "codex",
                None,
                native_id,
                {"turn_id": "turn-flush", "client_user_message_id": "client-flush"},
            )
            == "flushed answer"
        )
    finally:
        writer.join()

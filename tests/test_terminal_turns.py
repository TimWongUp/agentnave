from __future__ import annotations

from pathlib import Path

import pytest

from agentnave.terminal_turns import antigravity_turn, claude_turn, codex_turn


def test_claude_stays_between_prompt_ids() -> None:
    entries: list[dict[str, object]] = [
        {"type": "user", "promptId": "a"},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "first"}]}},
        {"type": "user", "promptId": "b"},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "second"}]}},
    ]
    assert claude_turn(entries, "a") == "first"
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

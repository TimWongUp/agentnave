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
        # Background task continuation after turn_duration belongs to turn "a" until next human prompt.
        {
            "type": "user",
            "promptId": "a-notif",
            "promptSource": "system",
            "turnOrigin": "task_notification",
            "origin": {"kind": "task-notification"},
            "message": {
                "content": "<task-notification><task-id>bg-2</task-id></task-notification>"
            },
        },
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "bg followup"}]}},
        {"type": "system", "subtype": "turn_duration"},
        {"type": "user", "promptId": "b", "message": {"content": "second prompt"}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "second"}]}},
    ]
    assert claude_turn(entries, "a") == "first\n\nbg followup"
    # Settlement anchors (end_prompt_id / end_row_index) bound the read strictly (e.g. bg-only cancel).
    assert claude_turn(entries, "a", end_prompt_id="a") == "first"
    assert claude_turn(entries, "a", end_row_index=7) == "first"
    assert claude_turn(entries, "a", end_prompt_id="a-notif", end_row_index=10) == (
        "first\n\nbg followup"
    )
    assert claude_turn(entries, "b") == "second"
    with pytest.raises(ValueError):
        claude_turn(entries, "missing")


def test_claude_multi_subturn_does_not_truncate_when_end_prompt_id_matches_start() -> None:
    entries: list[dict[str, object]] = [
        {"type": "user", "promptId": "p0", "message": {"content": "start"}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "part 0"}]}},
        {"type": "system", "subtype": "turn_duration"},
        {
            "type": "user",
            "promptSource": "system",
            "turnOrigin": "task_notification",
            "origin": {"kind": "task-notification"},
            "message": {
                "content": "<task-notification><task-id>bg-1</task-id></task-notification>"
            },
        },
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "part 1"}]}},
        {"type": "system", "subtype": "turn_duration"},
    ]
    assert claude_turn(entries, "p0", end_prompt_id="p0", end_row_index=6) == "part 0\n\npart 1"


def test_read_turn_rejects_symlink_to_another_session(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import json
    import os

    from agentnave.terminal_turns import read_turn

    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    proj = tmp_path / "projects/p1"
    proj.mkdir(parents=True)
    target_sid = "11111111-1111-4111-8111-111111111111"
    link_sid = "22222222-2222-4222-8222-222222222222"
    target = proj / f"{target_sid}.jsonl"
    target.write_text(
        json.dumps({"type": "user", "promptId": "p0", "message": {"content": "hi"}})
        + "\n"
        + json.dumps(
            {"type": "assistant", "message": {"content": [{"type": "text", "text": "ok"}]}}
        )
        + "\n",
        encoding="utf-8",
    )
    os.symlink(target, proj / f"{link_sid}.jsonl")
    with pytest.raises(ValueError, match="missing or ambiguous"):
        read_turn("claude", None, link_sid, {"prompt_id": "p0"})

    agy_root = tmp_path / "agy-home"
    monkeypatch.setenv("AGENTNAVE_AGY_HOME", str(agy_root))
    agy_target_logs = agy_root / "brain" / target_sid / ".system_generated/logs"
    agy_target_logs.mkdir(parents=True)
    (agy_target_logs / "transcript_full.jsonl").write_text(
        json.dumps({"type": "USER_INPUT", "source": "USER_EXPLICIT", "step_index": 0})
        + "\n"
        + json.dumps({"type": "PLANNER_RESPONSE", "content": "secret"})
        + "\n",
        encoding="utf-8",
    )
    os.symlink(agy_root / "brain" / target_sid, agy_root / "brain" / link_sid)
    with pytest.raises(ValueError, match="missing or ambiguous"):
        read_turn("antigravity", None, link_sid, {"user_input_index": 0})


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
        {"type": "USER_INPUT", "source": "SYSTEM", "step_index": 5},
        {"type": "PLANNER_RESPONSE", "content": "bg finished"},
        {"type": "USER_INPUT", "source": "USER_EXPLICIT", "step_index": 8},
        {"type": "PLANNER_RESPONSE", "content": "second"},
    ]
    assert antigravity_turn(entries, 3) == "first\n\nbg finished"
    assert antigravity_turn(entries, 3, end_row_index=2) == "first"
    assert antigravity_turn(entries, 8) == "second"
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

"""Read one anchored native CLI turn without falling back to another turn."""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import cast

from agentnave import accounts
from agentnave.adapters import get_adapter
from agentnave.claude_tui.turn import is_user_prompt, transcript_event


def _object(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


def rows(path: Path) -> list[dict[str, object]]:
    result: list[dict[str, object]] = []
    with path.open("rb") as stream:
        for line in stream:
            if not line.endswith(b"\n"):
                break
            try:
                value: object = json.loads(line)
            except ValueError as exc:
                raise ValueError("invalid native transcript line") from exc
            if isinstance(value, dict):
                result.append(cast(dict[str, object], value))
    return result


def read_turn(
    provider: str, account: str | None, session_id: str, anchor: dict[str, object]
) -> str:
    """Locate this account's native transcript and read the exact anchored turn."""
    if not re.fullmatch(r"[0-9a-fA-F-]{36}", session_id):
        raise ValueError("invalid native session ID")
    home = accounts.home_of(get_adapter(provider), account)
    if provider == "claude":
        root = Path(home or Path.home() / ".claude") / "projects"
        matches = list(root.glob(f"*/{session_id}.jsonl"))
        key = anchor.get("prompt_id")
        parser = claude_turn
    elif provider == "codex":
        root = Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex") / "sessions"
        matches = list(root.glob(f"*/*/*/rollout-*-{session_id}.jsonl"))
        key = anchor.get("turn_id")
        client_id = anchor.get("client_user_message_id")
        if not isinstance(client_id, str) or not client_id:
            raise ValueError("missing Codex client_user_message_id")
        matches = [p for p in matches if p.is_file() and p.resolve().is_relative_to(root.resolve())]
        if len(matches) != 1 or not isinstance(key, str) or not key:
            raise ValueError("native transcript missing, ambiguous, or missing turn anchor")
        entries = rows(matches[0])
        if not any(
            row.get("type") == "event_msg"
            and (payload := _object(row.get("payload"))).get("type") == "item_completed"
            and payload.get("turn_id") == key
            and _object(payload.get("item")).get("type") == "UserMessage"
            and _object(payload.get("item")).get("client_id") == client_id
            for row in entries
        ):
            raise ValueError("Codex user message does not match its client ID and turn")
        return codex_turn(entries, key)
    elif provider == "antigravity":
        root = Path.home() / ".gemini/antigravity-cli/brain"
        matches = [root / session_id / ".system_generated/logs/transcript_full.jsonl"]
        index = anchor.get("user_input_index")
        if not isinstance(index, int) or isinstance(index, bool):
            raise ValueError("missing Antigravity user_input_index")
        matches = [p for p in matches if p.is_file() and p.resolve().is_relative_to(root.resolve())]
        if len(matches) != 1:
            raise ValueError("native transcript missing or ambiguous")
        return antigravity_turn(rows(matches[0]), index)
    else:
        raise ValueError("unsupported terminal provider")
    matches = [p for p in matches if p.is_file() and p.resolve().is_relative_to(root.resolve())]
    if len(matches) != 1:
        raise ValueError("native transcript missing or ambiguous")
    if not isinstance(key, str) or not key:
        raise ValueError("missing native turn anchor")
    return parser(rows(matches[0]), key)


def claude_turn(entries: list[dict[str, object]], prompt_id: str) -> str:
    if not prompt_id:
        raise ValueError("missing Claude prompt_id")
    active = False
    answer = ""
    for row in entries:
        if row.get("isSidechain") or row.get("isMeta"):
            continue
        event = transcript_event(row, "")
        if (
            active
            and event is not None
            and is_user_prompt(event)
            and row.get("promptId") != prompt_id
        ):
            break
        if row.get("type") == "user" and row.get("promptId"):
            if active and row["promptId"] != prompt_id:
                break
            if row["promptId"] == prompt_id:
                active = True
        if active and row.get("type") == "assistant":
            content = _object(row.get("message")).get("content")
            if isinstance(content, list):
                text = "".join(
                    str(block.get("text", ""))
                    for item in cast(list[object], content)
                    if (block := _object(item)).get("type") == "text"
                )
                if text:
                    answer = text
    if not active or not answer:
        raise ValueError("Claude anchored turn or its assistant reply was not found")
    return answer


def codex_turn(entries: list[dict[str, object]], turn_id: str) -> str:
    if not turn_id:
        raise ValueError("missing Codex turn_id")
    for row in reversed(entries):
        payload = _object(row.get("payload"))
        if (
            row.get("type") == "event_msg"
            and payload.get("type") == "task_complete"
            and payload.get("turn_id") == turn_id
            and isinstance(payload.get("last_agent_message"), str)
            and payload["last_agent_message"]
        ):
            return cast(str, payload["last_agent_message"])
    raise ValueError("Codex anchored turn or its assistant reply was not found")


def antigravity_turn(entries: list[dict[str, object]], user_input_index: int) -> str:
    if isinstance(user_input_index, bool) or user_input_index < 0:
        raise ValueError("invalid Antigravity user_input_index")
    active = False
    answer = ""
    for row in entries:
        if row.get("type") == "USER_INPUT" and row.get("source") == "USER_EXPLICIT":
            if active:
                break
            if row.get("step_index") == user_input_index:
                active = True
        elif active and row.get("type") == "PLANNER_RESPONSE":
            content = row.get("content")
            if row.get("tool_calls") or not isinstance(content, str) or not content:
                answer = ""
            else:
                answer = content
    if not active or not answer:
        raise ValueError("Antigravity anchored turn or its assistant reply was not found")
    return answer

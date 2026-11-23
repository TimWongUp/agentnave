"""Read one anchored native CLI turn without falling back to another turn."""

from __future__ import annotations

import json
import os
import re
import stat
import time
from pathlib import Path
from typing import cast

from agentnave import accounts
from agentnave.adapters import get_adapter
from agentnave.claude_tui.turn import is_user_prompt, transcript_event

CODEX_READ_RETRY_SECONDS = 3.0
CODEX_READ_POLL_SECONDS = 0.2
# Keep in sync with `MAX_TRANSCRIPT_LINE_BYTES` in `apps/tray/src/terminal/derived/native_logs.rs`.
MAX_TRANSCRIPT_LINE_BYTES = 8 * 1024 * 1024


def _object(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


def _is_regular_in_root(path: Path, root: Path) -> bool:
    """Reject symlinks anywhere under `root`, including links pointing to another session."""
    try:
        rel = path.relative_to(root)
    except ValueError:
        return False
    cur = root
    if cur.is_symlink():
        return False
    for part in rel.parts:
        cur = cur / part
        if cur.is_symlink():
            return False
    try:
        return path.is_file() and path.resolve() == root.resolve() / rel
    except OSError:
        return False


def _optional_end_row_index(anchor: dict[str, object]) -> int | None:
    raw = anchor.get("end_row_index")
    if raw is None:
        return None
    if isinstance(raw, bool) or not isinstance(raw, int) or raw < 0:
        raise ValueError("invalid native turn end_row_index")
    return raw


def _optional_end_prompt_id(anchor: dict[str, object]) -> str | None:
    raw = anchor.get("end_prompt_id")
    if raw is None:
        return None
    if not isinstance(raw, str) or not raw:
        raise ValueError("invalid Claude end_prompt_id")
    return raw


def rows(path: Path, *, max_rows: int | None = None) -> list[dict[str, object]]:
    if max_rows == 0:
        return []
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise ValueError("native transcript missing or inaccessible") from exc
    result: list[dict[str, object]] = []
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("native transcript is not a regular file")
        while True:
            line = stream.readline(MAX_TRANSCRIPT_LINE_BYTES + 1)
            if not line:
                break
            if len(line) > MAX_TRANSCRIPT_LINE_BYTES:
                ended = line.endswith(b"\n")
                while not ended:
                    chunk = stream.readline(64 * 1024)
                    if not chunk:
                        break
                    if chunk.endswith(b"\n"):
                        ended = True
                if not ended:
                    break
                # Count complete oversized lines as a row placeholder to keep end_row_index aligned.
                result.append({})
                if max_rows is not None and len(result) >= max_rows:
                    break
                continue
            if not line.endswith(b"\n"):
                break
            try:
                value: object = json.loads(line)
            except ValueError as exc:
                raise ValueError("invalid native transcript line") from exc
            if isinstance(value, dict):
                result.append(cast(dict[str, object], value))
                if max_rows is not None and len(result) >= max_rows:
                    break
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
        end_prompt_id = _optional_end_prompt_id(anchor)
        end_row_index = _optional_end_row_index(anchor)
        matches = [p for p in matches if _is_regular_in_root(p, root)]
        if len(matches) != 1:
            raise ValueError("native transcript missing or ambiguous")
        if not isinstance(key, str) or not key:
            raise ValueError("missing native turn anchor")
        return claude_turn(
            rows(matches[0], max_rows=end_row_index),
            key,
            end_prompt_id=end_prompt_id,
            end_row_index=end_row_index,
        )
    if provider == "codex":
        root = Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex") / "sessions"
        key = anchor.get("turn_id")
        client_id = anchor.get("client_user_message_id")
        if not isinstance(client_id, str) or not client_id:
            raise ValueError("missing Codex client_user_message_id")
        if not isinstance(key, str) or not key:
            raise ValueError("native transcript missing, ambiguous, or missing turn anchor")
        return _read_codex_turn_with_retry(root, session_id, key, client_id)
    if provider == "antigravity":
        agy_root = Path(
            os.environ.get("AGENTNAVE_AGY_HOME") or Path.home() / ".gemini/antigravity-cli"
        )
        root = agy_root / "brain"
        matches = [root / session_id / ".system_generated/logs/transcript_full.jsonl"]
        index = anchor.get("user_input_index")
        if not isinstance(index, int) or isinstance(index, bool):
            raise ValueError("missing Antigravity user_input_index")
        end_row_index = _optional_end_row_index(anchor)
        matches = [p for p in matches if _is_regular_in_root(p, root)]
        if len(matches) != 1:
            raise ValueError("native transcript missing or ambiguous")
        return antigravity_turn(
            rows(matches[0], max_rows=end_row_index), index, end_row_index=end_row_index
        )
    raise ValueError("unsupported terminal provider")


def _codex_user_message_clients(entries: list[dict[str, object]], turn_id: str) -> list[str]:
    clients: list[str] = []
    for row in entries:
        if row.get("type") != "event_msg":
            continue
        payload = _object(row.get("payload"))
        if payload.get("type") != "item_completed" or payload.get("turn_id") != turn_id:
            continue
        item = _object(payload.get("item"))
        if item.get("type") == "UserMessage":
            cid = item.get("client_id")
            clients.append(cid if isinstance(cid, str) else "")
    return clients


def _read_codex_turn_with_retry(root: Path, session_id: str, turn_id: str, client_id: str) -> str:
    """Retry briefly while Codex flushes the rollout JSONL after turn/completed."""
    deadline = time.monotonic() + CODEX_READ_RETRY_SECONDS
    last_error = "native transcript missing, ambiguous, or missing turn anchor"
    while True:
        matches = [
            p
            for p in root.glob(f"*/*/*/rollout-*-{session_id}.jsonl")
            if _is_regular_in_root(p, root)
        ]
        if len(matches) > 1:
            raise ValueError("native transcript missing, ambiguous, or missing turn anchor")
        if len(matches) == 1:
            entries = rows(matches[0])
            clients = _codex_user_message_clients(entries, turn_id)
            if client_id in clients:
                try:
                    return codex_turn(entries, turn_id)
                except ValueError as exc:
                    last_error = str(exc)
            elif clients:
                raise ValueError("Codex user message does not match its client ID and turn")
            else:
                last_error = "Codex user message does not match its client ID and turn"
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ValueError(last_error)
        time.sleep(min(CODEX_READ_POLL_SECONDS, remaining))


def _claude_user_text(row: dict[str, object]) -> str | None:
    if row.get("type") != "user" or row.get("isSidechain") or row.get("isMeta"):
        return None
    content = _object(row.get("message")).get("content")
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return None
    parts: list[str] = []
    has_prompt_block = False
    for raw in cast(list[object], content):
        block = _object(raw)
        if block.get("type") != "tool_result":
            has_prompt_block = True
            text = block.get("text")
            if isinstance(text, str):
                parts.append(text)
    return "\n".join(parts) if has_prompt_block else None


def _is_claude_task_notification(row: dict[str, object]) -> bool:
    if row.get("type") != "user" or row.get("isSidechain"):
        return False
    origin_kind = _object(row.get("origin")).get("kind")
    if origin_kind in ("task-notification", "task_notification"):
        return True
    if row.get("turnOrigin") in ("task_notification", "task-notification"):
        return True
    text = _claude_user_text(row)
    if text is None:
        return False
    trimmed = text.strip()
    if row.get("promptSource") == "system" and "<task-notification>" in trimmed:
        return True
    return (
        row.get("promptSource") is None
        and trimmed.startswith("<task-notification>")
        and "</task-notification>" in trimmed
    )


def _is_claude_interrupt_row(row: dict[str, object]) -> bool:
    text = _claude_user_text(row)
    if text is None:
        return False
    trimmed = text.strip()
    return trimmed.startswith("[Request interrupted by user") and trimmed.endswith("]")


def _claude_user_prompt_id(row: dict[str, object]) -> str | None:
    """Return promptId only for genuine human non-meta user prompts (not task-notifications or interrupts)."""
    if _is_claude_task_notification(row) or _is_claude_interrupt_row(row):
        return None
    event = transcript_event(row, "")
    if event is None or not is_user_prompt(event):
        return None
    pid = row.get("promptId")
    return pid if isinstance(pid, str) and pid else None


def claude_turn(
    entries: list[dict[str, object]],
    prompt_id: str,
    *,
    end_prompt_id: str | None = None,
    end_row_index: int | None = None,
) -> str:
    if not prompt_id:
        raise ValueError("missing Claude prompt_id")
    scoped = entries[:end_row_index] if end_row_index is not None else entries
    active = False
    subturn_ended = False
    current_prompt_id: str | None = None
    replies: list[str] = []
    current_reply = ""
    for row in scoped:
        if row.get("isSidechain"):
            continue
        if active and row.get("type") == "system" and row.get("subtype") == "turn_duration":
            if current_reply:
                replies.append(current_reply)
                current_reply = ""
            subturn_ended = True
            if (
                end_row_index is None
                and end_prompt_id is not None
                and current_prompt_id == end_prompt_id
            ):
                break
            continue
        if row.get("isMeta"):
            continue
        if active and _is_claude_task_notification(row):
            if current_reply:
                replies.append(current_reply)
                current_reply = ""
            notif_pid = row.get("promptId")
            if isinstance(notif_pid, str) and notif_pid:
                current_prompt_id = notif_pid
            elif subturn_ended:
                current_prompt_id = None
            subturn_ended = False
            continue
        if _is_claude_interrupt_row(row):
            continue
        user_pid = _claude_user_prompt_id(row)
        if user_pid is not None:
            if active and user_pid != prompt_id:
                break
            if user_pid == prompt_id:
                active = True
                subturn_ended = False
                current_prompt_id = prompt_id
                current_reply = ""
                continue
        if active and row.get("type") == "assistant":
            content = _object(row.get("message")).get("content")
            if isinstance(content, list):
                text = "".join(
                    str(block.get("text", ""))
                    for item in cast(list[object], content)
                    if (block := _object(item)).get("type") == "text"
                )
                if text:
                    current_reply = text
    if current_reply:
        replies.append(current_reply)
    if not active or not replies:
        raise ValueError("Claude anchored turn or its assistant reply was not found")
    return "\n\n".join(replies)


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


def antigravity_turn(
    entries: list[dict[str, object]],
    user_input_index: int,
    *,
    end_row_index: int | None = None,
) -> str:
    if isinstance(user_input_index, bool) or user_input_index < 0:
        raise ValueError("invalid Antigravity user_input_index")
    scoped = entries[:end_row_index] if end_row_index is not None else entries
    active = False
    replies: list[str] = []
    current_reply = ""
    for row in scoped:
        if row.get("type") == "USER_INPUT" and row.get("source") == "USER_EXPLICIT":
            if active:
                break
            if row.get("step_index") == user_input_index:
                active = True
                current_reply = ""
        elif active and row.get("type") == "USER_INPUT":
            if current_reply:
                replies.append(current_reply)
                current_reply = ""
        elif active and row.get("type") == "PLANNER_RESPONSE":
            content = row.get("content")
            if row.get("tool_calls"):
                if current_reply:
                    replies.append(current_reply)
                    current_reply = ""
            elif isinstance(content, str) and content:
                current_reply = content
    if current_reply:
        replies.append(current_reply)
    if not active or not replies:
        raise ValueError("Antigravity anchored turn or its assistant reply was not found")
    return "\n\n".join(replies)

"""One turn's state, built from the transcript and hook events, and its stream-json result."""

from __future__ import annotations

import glob
import json
import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast


def object_dict(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


def emit(event: dict[str, object]) -> None:
    print(json.dumps(event, ensure_ascii=False), flush=True)


def transcript_event(row: dict[str, object], session_id: str) -> dict[str, object] | None:
    if row.get("isSidechain") or row.get("isMeta"):
        return None
    if row.get("type") not in ("assistant", "user") or not isinstance(row.get("message"), dict):
        return None
    return {
        "type": row["type"],
        "message": row["message"],
        "session_id": session_id,
        "parent_tool_use_id": None,
    }


# Claude Code's three wordings (timed, a message arrived, server-side task) all open this way.
_MCP_MOVED = re.compile(r'MCP tool "[^"]*" [^\n]*?was moved to the background as task (\w+)')


def _result_text(content: object) -> str:
    if isinstance(content, str):
        return content
    blocks = cast(list[object], content) if isinstance(content, list) else []
    return str(object_dict(blocks[0]).get("text", "")) if blocks else ""


def is_user_prompt(event: dict[str, object]) -> bool:
    if event.get("type") != "user":
        return False
    content = cast(dict[str, object], event["message"]).get("content")
    return isinstance(content, str) or (
        isinstance(content, list)
        and any(
            isinstance(b, dict) and cast(dict[str, object], b).get("type") != "tool_result"
            for b in cast(list[object], content)
        )
    )


@dataclass
class Turn:
    session_id: str
    assistant: dict[str, object] = field(default_factory=dict[str, object])
    denials: list[dict[str, object]] = field(default_factory=list[dict[str, object]])
    warnings: list[str] = field(default_factory=list[str])
    error: str | None = None
    user_seen: bool = False
    done: bool = False
    pending_prompts: set[str] = field(default_factory=set[str])
    # Background tool calls still running: like --print, the turn waits for their notifications.
    background: set[str] = field(default_factory=set[str])
    # MCP tool calls seen, whose results alone may announce a move to the background.
    mcp_calls: set[str] = field(default_factory=set[str])
    # The permission mode Claude Code reported for the prompt, and whether the mode check
    # held the prompt back to restart in the fallback mode.
    mode: str = ""
    mode_blocked: bool = False

    def transcript(self, row: dict[str, object]) -> None:
        event = transcript_event(row, self.session_id)
        if event is not None:
            emit(event)
            self.user_seen |= is_user_prompt(event)
            if event["type"] == "assistant":
                self.assistant = cast(dict[str, object], event["message"])
            self.track_background(event)
            self.track_async_launch(row)
        # A notification arriving while Claude is busy is attached to the running turn
        # instead of starting one; it still settles that background call.
        attachment = object_dict(row.get("attachment"))
        if not row.get("isSidechain") and attachment.get("type") == "queued_command":
            self.settle(str(attachment.get("prompt", "")))
        if (
            not row.get("isSidechain")
            and row.get("type") == "system"
            and row.get("subtype") == "turn_duration"
        ):
            self.done = self.user_seen and not self.background

    def track_background(self, event: dict[str, object]) -> None:
        content = cast(dict[str, object], event["message"]).get("content")
        if event["type"] == "user":
            # Notifications arrive as the message itself or its text blocks; a tool's output
            # may quote one and must not settle anything.
            self.settle(
                _result_text(content)
                if isinstance(content, str)
                else "".join(
                    str(block.get("text", ""))
                    for raw in (cast(list[object], content) if isinstance(content, list) else [])
                    if (block := object_dict(raw)).get("type") == "text"
                )
            )
            for raw in cast(list[object], content) if isinstance(content, list) else []:
                block = object_dict(raw)
                if block.get("type") != "tool_result":
                    continue
                # A denied or failed call never started, so no notification will follow.
                if block.get("is_error"):
                    self.background.discard(str(block.get("tool_use_id")))
                    continue
                # Claude may move a long MCP call to the background itself; only that call's
                # own result opens with the notice, and the notification carries just the task
                # id. Other tools' output may quote the notice and must not count.
                if block.get("tool_use_id") in self.mcp_calls and (
                    moved := _MCP_MOVED.match(_result_text(block.get("content")))
                ):
                    self.background.add(moved.group(1))
            return
        for raw in cast(list[object], content) if isinstance(content, list) else []:
            block = object_dict(raw)
            if block.get("type") != "tool_use":
                continue
            if str(block.get("name", "")).startswith("mcp__"):
                self.mcp_calls.add(str(block.get("id")))
            if object_dict(block.get("input")).get("run_in_background"):
                self.background.add(str(block.get("id")))

    def settle(self, text: str) -> None:
        """Forget the background calls a task notification names."""
        for tool_use in re.findall(r"<tool-use-id>([^<]+)</tool-use-id>", text):
            self.background.discard(tool_use)
        for task in re.findall(r"<task-id>([^<]+)</task-id>", text):
            self.background.discard(task)

    def track_async_launch(self, row: dict[str, object]) -> None:
        """Claude may run an Agent call in the background without `run_in_background`; its
        tool result says so, and the turn still waits for that call's notification."""
        if not object_dict(row.get("toolUseResult")).get("isAsync"):
            return
        content = object_dict(row.get("message")).get("content")
        for raw in cast(list[object], content) if isinstance(content, list) else []:
            block = object_dict(raw)
            if block.get("type") == "tool_result" and isinstance(block.get("tool_use_id"), str):
                self.background.add(str(block["tool_use_id"]))

    def hook(self, event: dict[str, object]) -> None:
        name = event.get("hook_event_name")
        if name == "UserPromptSubmit":
            self.mode = str(event.get("permission_mode") or "")
            self.mode_blocked = bool(event.get("agentnave_mode_blocked"))
        # Only Claude's own refusals count: a boss refusal is the boss's decision, not a
        # permission the tier lacks.
        if name == "PermissionDenied":
            self.denials.append(
                {"tool_name": event.get("tool_name"), "tool_input": event.get("tool_input")}
            )
        if name == "StopFailure":
            self.error = str(event.get("error") or "Claude StopFailure")
            detail = event.get("error_details") or event.get("last_assistant_message")
            if self.error in ("authentication_failed", "invalid_api_key"):
                self.error += ": authentication failed"
            elif detail:
                self.error += f": {str(detail)[:300]}"

    def result(self) -> dict[str, object]:
        content = self.assistant.get("content")
        blocks = cast(list[object], content) if isinstance(content, list) else []
        text = "".join(
            str(object_dict(b).get("text", ""))
            for b in blocks
            if object_dict(b).get("type") == "text"
        )
        return {
            "type": "result",
            "subtype": "error_during_execution" if self.error else "success",
            "is_error": self.error is not None,
            "result": text,
            "session_id": self.session_id or None,
            "stop_reason": self.assistant.get("stop_reason"),
            "permission_denials": self.denials,
            "errors": [self.error] if self.error else [],
            "warnings": self.warnings,
        }


class Transcript:
    def __init__(self, session_id: str) -> None:
        root = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "projects"
        self.pattern = str(root / "*" / (glob.escape(session_id) + ".jsonl"))
        self.path = self.find()
        self.offset = self.path.stat().st_size if self.path is not None else 0

    def find(self) -> Path | None:
        paths = glob.glob(self.pattern)
        return Path(paths[0]) if paths else None

    def read(self) -> list[dict[str, object]]:
        self.path = self.path or self.find()
        if self.path is None:
            return []
        rows, self.offset = self.rows(self.path, self.offset)
        return rows

    @staticmethod
    def rows(path: Path, offset: int) -> tuple[list[dict[str, object]], int]:
        """Complete JSON object lines from offset on, and the offset after them."""
        rows: list[dict[str, object]] = []
        with path.open("rb") as source:
            source.seek(offset)
            while line := source.readline():
                if not line.endswith(b"\n"):
                    break
                offset = source.tell()
                try:
                    value: object = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict):
                    rows.append(cast(dict[str, object], value))
        return rows, offset


def blocked(turn: Turn, reason: str, screen: str = "") -> None:
    excerpt = " ".join(screen.split())[-600:]
    turn.error = f"permission required: Claude interactive runtime blocked: {reason}"
    if excerpt:
        turn.error += f". Screen: {excerpt}"


def drop_empty_session(transcript: Transcript) -> bool:
    """Removes the file of a session this run created without any conversation in it; whether
    the session has no conversation to resume."""
    path = transcript.find()
    if path is None:
        return True
    if any(row.get("type") in ("user", "assistant") for row in Transcript.rows(path, 0)[0]):
        return False
    path.unlink(missing_ok=True)
    return True

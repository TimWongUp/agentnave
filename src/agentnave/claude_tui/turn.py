"""One turn's state, built from the transcript and hook events, and its stream-json result."""

from __future__ import annotations

import glob
import json
import os
import re
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

# Seconds to wait for the second half of a (Stop hook, system/turn_duration) pair before
# falling back to transcript background-task tracking (ADR-0114 section 4).
PAIR_TIMEOUT = 4.0
_MAX_BG_REPLY_CHARS = 512 * 1024

# Background task types reported by Claude Code's Stop.background_tasks that wake the main turn.
# session_crons and internal tasks (dream, auto-mode scan) do not wake the main turn.
# Keep in sync with `WAKING_TASK_TYPES` and ID fallback order (`id` -> `task_id` -> `tool_use_id` ->
# `<kind>:<index>`) in `apps/tray/src/terminal/derived/claude/turn_state.rs`.
_WAKING_TASK_TYPES = frozenset(
    {
        "shell",
        "bash",
        "local_bash",
        "monitor",
        "subagent",
        "agent",
        "local_agent",
        "remote_agent",
        "in_process_teammate",
        "mcp",
        "mcp_task",
        "mcp_tool",
        "remote_mcp",
        "workflow",
        "local_workflow",
    }
)
_TERMINAL_TASK_STATUSES = frozenset(
    {"completed", "done", "failed", "killed", "stopped", "cancelled", "canceled"}
)
_RUNNING_NOTIFICATION_STATUSES = frozenset({"running", "in_progress", "event", "progress"})
_NON_BG_TASK_TOOLS = frozenset(
    {"TaskStop", "TaskOutput", "TaskGet", "TaskList", "TaskCreate", "TaskUpdate"}
)
_INTERRUPTED_TEXTS = frozenset(
    {"[Request interrupted by user]", "[Request interrupted by user for tool use]"}
)
_STATUS_TAG = re.compile(r"<status>\s*([^<\s]+)\s*</status>", re.IGNORECASE)
_STOP_MISSING_WARNING = (
    "Claude Stop hook did not arrive after turn_duration; "
    "fell back to transcript background-task tracking"
)
_DURATION_MISSING_WARNING = (
    "Claude turn_duration did not arrive after Stop hook; "
    "fell back to transcript background-task tracking"
)


def object_dict(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


def emit(event: dict[str, object]) -> None:
    # UTF-8 bytes whatever the console code page, without escaping, which would double CJK output.
    sys.stdout.buffer.write((json.dumps(event, ensure_ascii=False) + "\n").encode())
    sys.stdout.buffer.flush()


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


def _user_text(content: object) -> str:
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "".join(
        str(block.get("text", ""))
        for raw in cast(list[object], content)
        if (block := object_dict(raw)).get("type") == "text"
    )


def _assistant_text(message: dict[str, object]) -> str:
    content = message.get("content")
    blocks = cast(list[object], content) if isinstance(content, list) else []
    return "".join(
        str(object_dict(block).get("text", ""))
        for block in blocks
        if object_dict(block).get("type") == "text"
    )


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


def is_task_notification(row: dict[str, object], event: dict[str, object]) -> bool:
    """Whether a user transcript row is a system-injected <task-notification> wake."""
    if event.get("type") != "user":
        return False
    if row.get("turnOrigin") == "task_notification":
        return True
    if object_dict(row.get("origin")).get("kind") == "task-notification":
        return True
    text = _user_text(cast(dict[str, object], event["message"]).get("content")).lstrip()
    return text.startswith("<task-notification>")


def _waking_background_tasks(event: dict[str, object]) -> tuple[bool, tuple[str, ...]]:
    """Return (has_background_tasks_field, waking_task_ids) from a Stop hook payload."""
    if "background_tasks" not in event or not isinstance(event.get("background_tasks"), list):
        return False, ()
    found: list[str] = []
    seen: set[str] = set()
    for index, raw in enumerate(cast(list[object], event["background_tasks"])):
        item = object_dict(raw)
        if not item:
            continue
        status = str(item.get("status") or "").strip().lower()
        if status in _TERMINAL_TASK_STATUSES:
            continue
        kind = str(item.get("type") or "").strip().lower().replace("-", "_").replace(" ", "_")
        if kind not in _WAKING_TASK_TYPES:
            continue
        task_id = next(
            (
                val
                for key in ("id", "task_id", "tool_use_id")
                if isinstance((val := item.get(key)), str) and val
            ),
            f"{kind}:{index}",
        )
        if task_id not in seen:
            seen.add(task_id)
            found.append(task_id)
    return True, tuple(found)


@dataclass(frozen=True, slots=True)
class _StopSignal:
    prompt_id: str | None
    has_bg_field: bool
    waking_tasks: tuple[str, ...]
    received_at: float


@dataclass(frozen=True, slots=True)
class _DurationSignal:
    prompt_id: str | None
    received_at: float


def _match_signal[T: (_StopSignal, _DurationSignal)](
    queue: list[T], prompt_id: str | None
) -> int | None:
    if not queue:
        return None
    if prompt_id:
        for idx, item in enumerate(queue):
            if item.prompt_id == prompt_id:
                return idx
        for idx, item in enumerate(queue):
            if item.prompt_id is None:
                return idx
        return None
    return 0


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
    # Background tool calls tracked from the transcript (used for older Claude versions
    # without Stop.background_tasks, or when Stop/turn_duration pairing times out).
    background: set[str] = field(default_factory=set[str])
    # MCP tool calls seen, whose results alone may announce a move to the background.
    mcp_calls: set[str] = field(default_factory=set[str])
    # The permission mode Claude Code reported for the prompt, and whether the mode check
    # held the prompt back to restart in the fallback mode.
    mode: str = ""
    mode_blocked: bool = False
    # Replies committed at the end of each sub-turn within this dispatch, concatenated in order.
    subturn_replies: list[str] = field(default_factory=list[str])
    _current_reply: str = ""
    _bg_aliases: dict[str, str] = field(default_factory=dict[str, str])
    _tool_names: dict[str, str] = field(default_factory=dict[str, str])
    _task_stops: dict[str, str] = field(default_factory=dict[str, str])
    _parents: dict[str, str | None] = field(default_factory=dict[str, str | None])
    _prompt_ids: dict[str, str] = field(default_factory=dict[str, str])
    _pending_stops: list[_StopSignal] = field(default_factory=list[_StopSignal])
    _pending_durations: list[_DurationSignal] = field(default_factory=list[_DurationSignal])
    _stop_bg_active: tuple[str, ...] | None = None
    _bg_wait_active: bool = False
    _subturn_idle: bool = False

    def _note_warning(self, message: str) -> None:
        if message not in self.warnings:
            self.warnings.append(message)

    def _index_row(self, row: dict[str, object]) -> None:
        if row.get("isSidechain"):
            return
        uid = row.get("uuid")
        if isinstance(uid, str) and uid:
            parent = row.get("parentUuid")
            self._parents[uid] = parent if isinstance(parent, str) and parent else None
            pid = row.get("promptId")
            if isinstance(pid, str) and pid:
                self._prompt_ids[uid] = pid

    def _resolve_prompt_id(self, row: dict[str, object]) -> str | None:
        pid = row.get("promptId")
        if isinstance(pid, str) and pid:
            return pid
        cursor = row.get("parentUuid")
        seen: set[str] = set()
        while isinstance(cursor, str) and cursor and cursor not in seen and len(seen) < 512:
            seen.add(cursor)
            if cursor in self._prompt_ids:
                return self._prompt_ids[cursor]
            cursor = self._parents.get(cursor)
        return None

    def _commit_subturn_reply(self) -> None:
        if self._current_reply:
            self.subturn_replies.append(self._current_reply)
            self._current_reply = ""

    def reply_text(self) -> str:
        parts = list(self.subturn_replies)
        if self._current_reply:
            parts.append(self._current_reply)
        elif not parts and self.assistant:
            fallback = _assistant_text(self.assistant)
            if fallback:
                parts.append(fallback)
        return "\n\n".join(parts)

    def has_reply(self) -> bool:
        return bool(self.reply_text().strip()) or bool(self.assistant)

    def _register_bg(self, key: str, *aliases: str) -> None:
        if not key:
            return
        canonical = self._bg_aliases.get(key)
        if canonical is None:
            for alias in aliases:
                if alias and alias in self._bg_aliases:
                    canonical = self._bg_aliases[alias]
                    break
        if canonical is None:
            canonical = key
            self.background.add(canonical)
        self._bg_aliases[canonical] = canonical
        self._bg_aliases[key] = canonical
        for alias in aliases:
            if alias:
                self._bg_aliases[alias] = canonical

    def _settle_bg(self, identifier: str) -> None:
        if not identifier:
            return
        canonical = self._bg_aliases.get(identifier, identifier)
        self.background.discard(canonical)
        for alias in [k for k, v in self._bg_aliases.items() if v == canonical]:
            del self._bg_aliases[alias]

    def transcript(self, row: dict[str, object], now: float | None = None) -> None:
        self._index_row(row)
        event = transcript_event(row, self.session_id)
        if event is not None:
            emit(event)
            if event["type"] == "user":
                user_text = _user_text(cast(dict[str, object], event["message"]).get("content"))
                if self.user_seen and user_text.strip() in _INTERRUPTED_TEXTS:
                    self.error = "Claude turn was interrupted"
                    return
                if is_task_notification(row, event):
                    self._commit_subturn_reply()
                    self._subturn_idle = False
                elif is_user_prompt(event):
                    self.user_seen = True
                else:
                    self._subturn_idle = False
            elif event["type"] == "assistant":
                self.assistant = cast(dict[str, object], event["message"])
                self._subturn_idle = False
                if text := _assistant_text(self.assistant):
                    self._current_reply = text
            self.track_background(event, row)
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
            self._on_turn_duration(self._resolve_prompt_id(row), now)

    def track_background(
        self, event: dict[str, object], row: dict[str, object] | None = None
    ) -> None:
        content = cast(dict[str, object], event["message"]).get("content")
        if event["type"] == "user":
            # Notifications arrive as the message itself or its text blocks; a tool's output
            # may quote one and must not settle anything.
            self.settle(_user_text(content))
            ok_ids = self._track_tool_results(content)
            if row is not None:
                self._track_tool_use_result(row, ok_ids)
            return
        for raw in cast(list[object], content) if isinstance(content, list) else []:
            block = object_dict(raw)
            if block.get("type") != "tool_use":
                continue
            call_id = block.get("id")
            if not isinstance(call_id, str) or not call_id:
                continue
            tool_name = str(block.get("name", ""))
            self._tool_names[call_id] = tool_name
            tool_input = object_dict(block.get("input"))
            if tool_name.startswith("mcp__"):
                self.mcp_calls.add(call_id)
            if tool_name == "TaskStop":
                target = tool_input.get("task_id") or tool_input.get("shell_id")
                if isinstance(target, str) and target:
                    self._task_stops[call_id] = target
            if tool_input.get("run_in_background"):
                self._register_bg(call_id)

    def _track_tool_results(self, content: object) -> list[str]:
        ok_ids: list[str] = []
        for raw in cast(list[object], content) if isinstance(content, list) else []:
            block = object_dict(raw)
            if block.get("type") != "tool_result":
                continue
            call_id = str(block.get("tool_use_id") or "")
            # A denied or failed call never started, so no notification will follow.
            if block.get("is_error"):
                self._settle_bg(call_id)
                self._task_stops.pop(call_id, None)
                continue
            if call_id:
                ok_ids.append(call_id)
            if call_id in self._task_stops:
                self._settle_bg(self._task_stops.pop(call_id))
                continue
            # Claude may move a long MCP call to the background itself; only that call's
            # own result opens with the notice, and the notification carries just the task
            # id. Other tools' output may quote the notice and must not count.
            if call_id in self.mcp_calls and (
                moved := _MCP_MOVED.match(_result_text(block.get("content")))
            ):
                self._register_bg(moved.group(1), call_id)
        return ok_ids

    def _track_tool_use_result(self, row: dict[str, object], ok_ids: list[str]) -> None:
        """Track background launches announced in toolUseResult (#504): async Agent calls,
        backgroundTaskId (including timedOutAfterMs auto-backgrounding), and Monitor taskId."""
        result = object_dict(row.get("toolUseResult"))
        if not result or not ok_ids:
            return
        names = {self._tool_names.get(call_id, "") for call_id in ok_ids}
        if result.get("isAsync"):
            extra = [
                str(value)
                for key in ("backgroundTaskId", "taskId", "agentId")
                if isinstance(value := result.get(key), str) and value
            ]
            for call_id in ok_ids:
                self._register_bg(call_id, *extra)
        bg_id = result.get("backgroundTaskId")
        if isinstance(bg_id, str) and bg_id and not (names & _NON_BG_TASK_TOOLS):
            self._register_bg(bg_id, *ok_ids)
        task_id = result.get("taskId")
        status = str(result.get("status") or "").strip().lower()
        if (
            isinstance(task_id, str)
            and task_id
            and status not in _TERMINAL_TASK_STATUSES
            and not (names & _NON_BG_TASK_TOOLS)
        ):
            self._register_bg(task_id, *ok_ids)

    def settle(self, text: str) -> None:
        """Forget the background calls a terminal task notification names."""
        if not text:
            return
        status_match = _STATUS_TAG.search(text)
        if status_match and status_match.group(1).lower() in _RUNNING_NOTIFICATION_STATUSES:
            return
        settled_ids: set[str] = set()
        for tool_use in re.findall(r"<tool-use-id>([^<]+)</tool-use-id>", text):
            settled_ids.add(tool_use)
            self._settle_bg(tool_use)
        for task in re.findall(r"<task-id>([^<]+)</task-id>", text):
            settled_ids.add(task)
            self._settle_bg(task)
        if settled_ids and self._stop_bg_active is not None:
            remaining = tuple(tid for tid in self._stop_bg_active if tid not in settled_ids)
            self._stop_bg_active = remaining
            if not remaining:
                self._bg_wait_active = False

    def _settle_subturn(self, stop: _StopSignal | None) -> None:
        if not self.user_seen:
            return
        self._commit_subturn_reply()
        if stop is not None and stop.has_bg_field:
            self._stop_bg_active = stop.waking_tasks
            active = stop.waking_tasks
        else:
            self._stop_bg_active = None
            active = tuple(sorted(self.background))
        if not active:
            self._bg_wait_active = False
            self._subturn_idle = False
            self.done = True
            return
        self.done = False
        self._subturn_idle = True
        if self.reply_text().strip():
            self._bg_wait_active = True

    def _on_stop(self, signal: _StopSignal) -> None:
        idx = _match_signal(self._pending_durations, signal.prompt_id)
        if idx is not None:
            self._pending_durations.pop(idx)
            self._settle_subturn(signal)
            return
        self._pending_stops.append(signal)

    def _on_turn_duration(self, prompt_id: str | None, now: float | None = None) -> None:
        idx = _match_signal(self._pending_stops, prompt_id)
        if idx is not None:
            stop = self._pending_stops.pop(idx)
            self._settle_subturn(stop)
            return
        current = time.monotonic() if now is None else now
        self._pending_durations.append(_DurationSignal(prompt_id, current))

    def poll_timeout(self, now: float | None = None) -> None:
        """Fall back to transcript tracking when one half of Stop/turn_duration never arrives."""
        current = time.monotonic() if now is None else now
        while self._pending_durations and current - self._pending_durations[0].received_at >= (
            PAIR_TIMEOUT
        ):
            self._pending_durations.pop(0)
            if self.user_seen:
                self._note_warning(_STOP_MISSING_WARNING)
                self._settle_subturn(None)
        while self._pending_stops and current - self._pending_stops[0].received_at >= PAIR_TIMEOUT:
            self._pending_stops.pop(0)
            if self.user_seen:
                self._note_warning(_DURATION_MISSING_WARNING)
                self._settle_subturn(None)

    def active_background_ids(self) -> tuple[str, ...]:
        if self._stop_bg_active is not None:
            return self._stop_bg_active
        return tuple(sorted(self.background))

    def background_state_event(self) -> dict[str, object]:
        active_ids = self.active_background_ids()
        reply = self.reply_text()
        waiting = (
            not self.done
            and self.error is None
            and self._bg_wait_active
            and bool(active_ids)
            and bool(reply.strip())
        )
        only_background = waiting and self._subturn_idle and not self.pending_prompts
        return {
            "type": "agentnave_background_tasks",
            "waiting": waiting,
            "only_background": only_background,
            "task_ids": list(active_ids) if waiting else [],
            "reply": reply[:_MAX_BG_REPLY_CHARS] if only_background else None,
            "session_id": self.session_id or None,
        }

    def hook(self, event: dict[str, object], now: float | None = None) -> None:
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
        if name == "Stop":
            raw_pid = event.get("prompt_id")
            prompt_id = raw_pid if isinstance(raw_pid, str) and raw_pid else None
            has_bg, waking = _waking_background_tasks(event)
            current = time.monotonic() if now is None else now
            self._on_stop(_StopSignal(prompt_id, has_bg, waking, current))
        if name == "StopFailure":
            self.error = str(event.get("error") or "Claude StopFailure")
            detail = event.get("error_details") or event.get("last_assistant_message")
            if self.error in ("authentication_failed", "invalid_api_key"):
                self.error += ": authentication failed"
            elif detail:
                self.error += f": {str(detail)[:300]}"

    def result(self) -> dict[str, object]:
        return {
            "type": "result",
            "subtype": "error_during_execution" if self.error else "success",
            "is_error": self.error is not None,
            "result": self.reply_text(),
            "session_id": self.session_id or None,
            "stop_reason": self.assistant.get("stop_reason"),
            "permission_denials": self.denials,
            "errors": [self.error] if self.error else [],
            "warnings": self.warnings,
        }


class Transcript:
    def __init__(self, session_id: str) -> None:
        # This process is one task's own child: its environment carries that task's account
        # (CLAUDE_CONFIG_DIR) or, for the default account, what the service inherited.
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


def unauthenticated(turn: Turn) -> None:
    """The login screen: this account has no stored login. Worded so that the run is blocked on
    authentication and the account cools down, as a `-p` run that reports it would."""
    turn.error = (
        "authentication required: Claude Code is not logged in for this account (the login "
        "screen was shown); sign in with `claude auth login` using this account's "
        "configuration directory"
    )


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

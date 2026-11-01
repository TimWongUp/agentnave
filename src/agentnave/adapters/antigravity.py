"""Transparent Google Antigravity CLI adapter."""

from __future__ import annotations

import json
import re
from collections.abc import Callable
from datetime import datetime
from pathlib import Path

from agentnave.adapters.base import (
    AdapterHooks,
    ParsedProviderResult,
    PreparedCommand,
    brief,
    error_summary,
    failure_status,
    normalized_usage,
    object_dict,
    parse_json_lines,
    parse_json_object,
    resolve_executable,
    restricted_options,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity

_LABEL = re.compile(rb'Propagating selected model override to backend: label="([^"\n]{1,80})"')
_LABEL_TEXT = re.compile(r"([A-Za-z0-9.]+(?: [A-Za-z0-9.]+)*)(?: \(([A-Za-z0-9 ]+)\))?")


def session_model(conversation_id: str | None, *, since: datetime) -> str | None:
    """The model agy used for this conversation, from its own run log; agy streams none.

    The log names the selected model by label ("Gemini 3.8 Flash (High)"), which maps to the
    model ID agy accepts ("gemini-3.8-flash-high"). Only one consistent label counts.
    """
    if not conversation_id:
        return None
    marker = f"Print mode: conversation={conversation_id},".encode()
    root = Path.home() / ".gemini" / "antigravity-cli" / "log"
    try:
        logs = [
            (stat.st_mtime, path)
            for path in root.glob("cli-*.log")
            if (stat := path.stat()).st_mtime >= since.timestamp() - 5 and stat.st_size < 16 << 20
        ]
        for _, path in sorted(logs, reverse=True)[:20]:
            data = path.read_bytes()
            if marker not in data:
                continue
            labels = {label.decode("utf-8", "replace") for label in _LABEL.findall(data)}
            if len(labels) != 1 or not (match := _LABEL_TEXT.fullmatch(labels.pop())):
                return None
            parts = [match.group(1), *([match.group(2)] if match.group(2) else [])]
            return "-".join(" ".join(parts).lower().split())
    except OSError:
        return None
    return None


_DISCUSSION_AGENT = """---
name: agentnave-discussion
description: Dialogue-only participant for AgentNave discussion rooms.
tools: []
mcpServers: []
skills: []
plugins: []
mainAgent: true
subagent: false
commandExecutionPolicy: off
---
You are a conversation participant. Answer the current user request.
"""


def _compact(value: dict[str, object]) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def _result_capture() -> Callable[[bytes], bytes]:
    """Retain result envelopes, session changes and step boundaries, not tool payloads."""
    last_session: str | None = None
    last_step: bytes | None = None

    def capture(line: bytes) -> bytes:
        nonlocal last_session, last_step
        event = parse_json_object(line.decode(errors="replace"))
        if event is None:
            return b""
        if event.get("event") == "result":
            # Keep dedup in step with the session id parse() may read from this envelope.
            if isinstance(session_id := event.get("conversation_id"), str) and session_id:
                last_session = session_id
            return line + b"\n"
        retained = b""
        session_id = event.get("conversation_id")
        if isinstance(session_id, str) and session_id and session_id != last_session:
            last_session = session_id
            retained += _compact({"conversation_id": session_id}) + b"\n"
        if event.get("event") == "step_update":
            step = object_dict(event.get("step_update"))
            compact = {"step_type": step.get("step_type"), "state": step.get("state")}
            # parse() reads only the last step's type and state.
            record = _compact({"event": "step_update", "step_update": compact})
            if record != last_step:
                last_step = record
                retained += record + b"\n"
        return retained

    return capture


def _restricted_profile(directory: Path) -> Path:
    return directory / ".agents" / "agents" / "agentnave-discussion.md"


def write_restricted_profile(directory: Path) -> None:
    """Prepare a dedicated seat directory; callers own that directory and its cleanup."""
    profile = _restricted_profile(directory)
    profile.parent.mkdir(parents=True, exist_ok=True)
    if profile.exists():
        if profile.read_text(encoding="utf-8") != _DISCUSSION_AGENT:
            raise ValueError("Restricted agent profile already exists with different contents")
        return
    with profile.open("x", encoding="utf-8") as stream:
        stream.write(_DISCUSSION_AGENT)


class AntigravityAdapter(AdapterHooks):
    name = "antigravity"
    _options = {
        "model": "--model",
        "effort": "--effort",
        "agent": "--agent",
        "mode": "--mode",
        "project": "--project",
        "print_timeout": "--print-timeout",
        "sandbox": "--sandbox",
        "dangerously_skip_permissions": "--dangerously-skip-permissions",
        "disable_slash_commands": "--disable-slash-commands",
    }
    _boolean_options = {"disable_slash_commands", "sandbox", "dangerously_skip_permissions"}

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = restricted_options(request)
        message = {"event": "user", "message": {"content": request.prompt}}
        prompt = (json.dumps(message, ensure_ascii=False) + "\n").encode()
        argv = [
            resolve_executable("agy"),
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
        ]
        if request.session_id is not None:
            argv.extend(("--conversation", request.session_id))
        argv.extend(self.option_args(request))
        if discussion:
            # agy loads agents only from cwd; never write into the caller's project here.
            profile = _restricted_profile(request.cwd)
            if not profile.is_file() or profile.read_text(encoding="utf-8") != _DISCUSSION_AGENT:
                raise ValueError(
                    "antigravity restricted_tools needs a dedicated cwd prepared with "
                    "write_restricted_profile"
                )
            argv.extend(
                (
                    "--agent",
                    "agentnave-discussion",
                    "--sandbox=true",
                    "--disable-slash-commands=true",
                )
            )
        return PreparedCommand(tuple(argv), request.cwd, prompt, capture_line=_result_capture())

    def session_model(self, session_id: str | None, *, since: datetime) -> str | None:
        return session_model(session_id, since=since)

    def prepare_seat(self, directory: Path) -> None:
        write_restricted_profile(directory)

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        event_type = event.get("event")
        if event_type == "init":
            return ProviderActivity("lifecycle", "init", "initialized")
        if event_type == "result":
            return ProviderActivity(
                "lifecycle",
                "result",
                brief(object_dict(event.get("result")).get("status")),
                # Only WAITING can leave agy running; other result states end the process.
                blocking_error=(
                    "provider_blocked"
                    if object_dict(event.get("result")).get("status") == "WAITING"
                    else None
                ),
            )
        if event_type != "step_update":
            return None
        step = object_dict(event.get("step_update"))
        step_type = step.get("step_type")
        state = brief(step.get("state"))
        if step_type == "tool":
            index = step.get("step_index")
            return ProviderActivity(
                "tool",
                "step_update.tool",
                state,
                brief(step.get("tool_name")),
                tool_call_id=str(index) if isinstance(index, int) else None,
            )
        if step_type == "agent_response":
            return ProviderActivity(
                "message",
                "step_update.agent_response",
                state,
                message=brief(step.get("text_delta")),
                public_output=brief(step.get("text_delta"), 1000),
                message_delta=True,
            )
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        stdout_text = stdout.decode(errors="replace").strip()
        stderr_text = stderr.decode(errors="replace").strip()
        events = parse_json_lines(stdout_text)
        stream_session_id = next(
            (
                conversation_id
                for event in reversed(events)
                if isinstance((conversation_id := event.get("conversation_id")), str)
                and conversation_id
            ),
            None,
        )
        envelope = next(
            (event for event in reversed(events) if event.get("event") == "result"), None
        )
        raw_payload: object = envelope.get("result") if envelope is not None else None
        payload: dict[str, object] | None = object_dict(raw_payload)
        if not isinstance(raw_payload, dict):
            payload = None

        if not payload:
            message = error_summary(
                stderr_text, "antigravity stream ended without a valid result event"
            )
            return ParsedProviderResult(
                failure_status(message),
                "",
                stream_session_id,
                error_message=message,
            )

        raw_output = payload.get("response", "")
        output = raw_output if isinstance(raw_output, str) else str(raw_output)
        session_id = payload.get("conversation_id")
        usage = normalized_usage(payload)
        provider_status = payload.get("status")
        explicit_error = payload.get("error")

        if not isinstance(session_id, str) or not session_id:
            session_id = stream_session_id
        normalized_status = provider_status.upper() if isinstance(provider_status, str) else None
        lowered_stderr = stderr_text.lower()
        blocked_by_stderr = bool(stderr_text) and (
            failure_status(stderr_text) is InvocationStatus.BLOCKED
            or "auto-denied" in lowered_stderr
            or "automatically denied" in lowered_stderr
        )
        last_step = object_dict(
            next(
                (
                    event.get("step_update")
                    for event in reversed(events)
                    if event.get("event") == "step_update"
                ),
                None,
            )
        )
        # agy reports ERROR with an error already recovered by its own retry, including one
        # left in the conversation by an earlier turn; a completed final reply as this
        # invocation's last step shows the turn itself finished.
        turn_answered = (
            normalized_status == "ERROR"
            and bool(output.strip())
            and last_step.get("step_type") == "agent_response"
            and last_step.get("state") == "DONE"
        )
        if (
            returncode == 0
            and ((normalized_status == "SUCCESS" and not explicit_error) or turn_answered)
            and not blocked_by_stderr
        ):
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id, usage)

        if normalized_status is None:
            status_message = "antigravity result omitted status"
        elif normalized_status != "SUCCESS":
            status_message = f"antigravity stopped with status {provider_status}"
        else:
            status_message = ""
        raw_message = str(
            explicit_error
            or stderr_text
            or status_message
            or (output if returncode == 0 else "")
            or f"antigravity exited with status {returncode}"
        )
        message = error_summary(raw_message, f"antigravity exited with status {returncode}")
        if normalized_status in {"CANCELED", "INTERRUPTED"}:
            status = InvocationStatus.CANCELLED
        elif normalized_status == "WAITING" or blocked_by_stderr:
            status = InvocationStatus.BLOCKED
        else:
            status = failure_status(message)
        return ParsedProviderResult(status, output, session_id, usage, message)

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        unknown = sorted(set(request.provider_options) - set(self._options))
        if unknown:
            raise ValueError(f"unsupported antigravity options: {', '.join(unknown)}")
        args: list[str] = []
        for key, value in request.provider_options.items():
            flag = self._options[key]
            if key in self._boolean_options:
                if not isinstance(value, bool):
                    raise ValueError(f"antigravity option {key} must be boolean")
                args.append(f"{flag}={str(value).lower()}")
            else:
                args.extend((flag, str(value).lower() if isinstance(value, bool) else str(value)))
        return tuple(args)

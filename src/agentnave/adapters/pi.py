"""Pi Coding Agent print/JSON adapter."""

from __future__ import annotations

import json
from typing import cast

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    brief,
    discussion_options,
    error_summary,
    failure_status,
    object_dict,
    option_args,
    parse_json_lines,
    parse_json_object,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity


def _text(message: dict[str, object]) -> str:
    content = message.get("content")
    if not isinstance(content, list):
        return ""
    return "".join(
        str(block["text"])
        for item in cast(list[object], content)
        if (block := object_dict(item)).get("type") == "text" and isinstance(block.get("text"), str)
    )


def _capture_line(line: bytes) -> bytes:
    event = parse_json_object(line.decode(errors="replace"))
    if event is None:
        return b""
    kind = event.get("type")
    if kind == "session":
        kept = {"type": kind, "id": event.get("id")}
    elif kind == "message_end" and object_dict(event.get("message")).get("role") == "assistant":
        message = object_dict(event.get("message"))
        kept = {
            "type": kind,
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": _text(message)}],
                "stopReason": message.get("stopReason"),
                "errorMessage": message.get("errorMessage"),
            },
        }
    elif kind == "agent_end":
        kept = {"type": kind}
    else:
        return b""
    return json.dumps(kept, ensure_ascii=False).encode() + b"\n"


class PiAdapter:
    name = "pi"
    _options = {
        "model": "--model",
        "effort": "--thinking",
        "provider": "--provider",
        "tools": "--tools",
    }

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = discussion_options(request)
        argv = ["pi", "--print", "--mode", "json"]
        if request.session_id is not None:
            argv.extend(("--session", request.session_id))
        argv.extend(option_args(request, self._options))
        if discussion:
            argv.extend(
                (
                    "--no-tools",
                    "--no-extensions",
                    "--no-mcp",
                    "--no-skills",
                    "--no-prompt-templates",
                )
            )
        return PreparedCommand(
            tuple(argv), request.cwd, stdin=request.prompt.encode(), capture_line=_capture_line
        )

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        kind = event.get("type")
        if kind == "message_update":
            delta = object_dict(event.get("assistantMessageEvent"))
            if delta.get("type") == "text_delta":
                return ProviderActivity(
                    "message",
                    "text_delta",
                    message=brief(delta.get("delta")),
                    message_delta=True,
                    public_output=brief(delta.get("delta"), 1000),
                )
        if kind in ("tool_execution_start", "tool_execution_end"):
            return ProviderActivity(
                "tool",
                str(kind),
                "completed" if kind == "tool_execution_end" else "running",
                brief(event.get("toolName")),
                tool_call_id=brief(event.get("toolCallId")),
            )
        if kind == "message_end":
            message = object_dict(event.get("message"))
            if message.get("role") == "assistant" and message.get("stopReason") in (
                "error",
                "aborted",
            ):
                return ProviderActivity("lifecycle", "message_end", "error")
        if kind == "agent_end":
            return ProviderActivity("lifecycle", "agent_end", "completed")
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        events = parse_json_lines(stdout.decode(errors="replace"))
        session_id = next(
            (
                value
                for event in events
                if event.get("type") == "session" and isinstance(value := event.get("id"), str)
            ),
            None,
        )
        message = next(
            (
                object_dict(event.get("message"))
                for event in reversed(events)
                if event.get("type") == "message_end"
                and object_dict(event.get("message")).get("role") == "assistant"
            ),
            object_dict(None),
        )
        output = _text(message)
        stop_reason = message.get("stopReason")
        complete = bool(events) and events[-1].get("type") == "agent_end"
        if returncode == 0 and complete and stop_reason == "stop":
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)
        raw_error = message.get("errorMessage")
        fallback = (
            f"pi exited with status {returncode}"
            if returncode != 0
            else f"pi stopped with reason {stop_reason}"
            if complete and stop_reason != "stop"
            else "pi stream ended without a completed assistant reply"
        )
        error = error_summary(
            raw_error if isinstance(raw_error, str) else stderr.decode(errors="replace"), fallback
        )
        status = (
            InvocationStatus.BLOCKED if "no api key" in error.lower() else failure_status(error)
        )
        return ParsedProviderResult(status, output, session_id, error_message=error)

"""Pi Coding Agent print/JSON adapter."""

from __future__ import annotations

import json
import re
from typing import cast

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    brief,
    discussion_options,
    error_summary,
    failure_status,
    normalized_usage,
    object_dict,
    option_args,
    parse_json_lines,
    parse_json_object,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity

# Pi reports this on stderr before asking on stdin whether to fork another project's session.
_FOREIGN_SESSION = re.compile(r"Session found in different project: ([^\n\x1b]*)")
# These events repeat whole-run messages or tool results and never decide the final reply.
_SKIPPABLE_OVERSIZED = (
    b'{"type":"agent_end"',
    b'{"type":"turn_end"',
    b'{"type":"tool_execution_',
)


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
                "cost": object_dict(object_dict(message.get("usage")).get("cost")).get("total"),
            },
        }
    elif kind == "agent_end":
        kept = {"type": kind}
    elif kind == "agent_settled":
        kept = {"type": kind, "aborted": event.get("aborted")}
    else:
        return b""
    return json.dumps(kept).encode() + b"\n"


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
        # Pi trims stdin, but answers its cross-project fork prompt with the first stdin line;
        # a leading blank line makes that prompt abort instead of consuming the user's prompt.
        stdin = request.prompt.encode()
        if request.session_id is not None:
            stdin = b"\n" + stdin
        return PreparedCommand(
            tuple(argv),
            request.cwd,
            stdin=stdin,
            capture_line=_capture_line,
            skip_oversized_line=lambda head: head.startswith(_SKIPPABLE_OVERSIZED),
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
                "running"
                if kind == "tool_execution_start"
                else "error"
                if event.get("isError") is True
                else "completed",
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
            state = "retrying" if event.get("willRetry") is True else "completed"
            return ProviderActivity("lifecycle", "agent_end", state)
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
        costs = [
            cost
            for event in events
            if event.get("type") == "message_end"
            and isinstance(cost := object_dict(event.get("message")).get("cost"), (int, float))
            and not isinstance(cost, bool)
        ]
        usage = normalized_usage({"total_cost_usd": sum(costs)}) if costs else {}
        last = events[-1] if events else {}
        # agent_settled follows agent_end; either may be the last retained completion marker.
        complete = last.get("type") == "agent_end" or (
            last.get("type") == "agent_settled" and last.get("aborted") is not True
        )
        foreign = _FOREIGN_SESSION.search(stderr.decode(errors="replace"))
        if foreign is not None:
            error = error_summary(
                f"pi session belongs to another working directory ({foreign.group(1).strip()}); "
                "continue it from that cwd",
                "pi session belongs to another working directory",
            )
            return ParsedProviderResult(InvocationStatus.FAILED, output, session_id, usage, error)
        if returncode == 0 and complete and stop_reason == "stop":
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id, usage)
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
        lowered = error.lower()
        status = (
            InvocationStatus.BLOCKED
            if "no api key" in lowered or "/login" in lowered
            else failure_status(error)
        )
        return ParsedProviderResult(status, output, session_id, usage, error)

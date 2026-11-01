"""Transparent Claude Code CLI adapter."""

from __future__ import annotations

import json
import re
from collections.abc import Callable
from typing import cast

from agentnave.adapters.base import (
    AdapterHooks,
    ParsedProviderResult,
    PreparedCommand,
    brief,
    clean_model,
    error_summary,
    failure_status,
    normalized_usage,
    object_dict,
    option_args,
    parse_json_lines,
    parse_json_object,
    resolve_executable,
    restricted_options,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity

# CodeBuddy leaves permission_denials empty and reports a denial only in the tool result.
_DENIED = re.compile(r"Error: Permission to use (\S{1,64}) has been denied")


def _denied_tools(event: dict[str, object]) -> list[str]:
    if event.get("type") != "user" or event.get("parent_tool_use_id") is not None:
        return []
    content = object_dict(event.get("message")).get("content")
    names: list[str] = []
    for raw in cast(list[object], content) if isinstance(content, list) else []:
        block = object_dict(raw)
        if block.get("type") != "tool_result":
            continue
        parts = block.get("content")
        texts = (
            [object_dict(part).get("text") for part in cast(list[object], parts)]
            if isinstance(parts, list)
            else [parts]
        )
        for text in texts:
            if isinstance(text, str) and (match := _DENIED.match(text)):
                names.append(match.group(1))
    return names


def _result_capture() -> Callable[[bytes], bytes]:
    last_session: str | None = None

    def capture(line: bytes) -> bytes:
        nonlocal last_session
        event = parse_json_object(line.decode(errors="replace"))
        if event is None:
            return b""
        if event.get("type") == "result":
            return line + b"\n"
        denied = b"".join(
            json.dumps({"permission_denial": {"tool_name": name}}).encode() + b"\n"
            for name in _denied_tools(event)
        )
        if denied:
            return denied
        # Keep only session changes for parse's fallback, not deltas or tool payloads.
        session_id = event.get("session_id")
        if not isinstance(session_id, str) or session_id == last_session:
            return b""
        last_session = session_id
        return json.dumps({"session_id": session_id}).encode() + b"\n"

    return capture


def assistant_model(event: dict[str, object]) -> str | None:
    """The main assistant's model; a subagent's messages carry parent_tool_use_id."""
    if event.get("parent_tool_use_id") is not None:
        return None
    kind = event.get("type")
    if kind == "assistant":
        return clean_model(object_dict(event.get("message")).get("model"))
    if kind == "stream_event":
        nested = object_dict(event.get("event"))
        if nested.get("type") == "message_start":
            return clean_model(object_dict(nested.get("message")).get("model"))
    return None


class ClaudeAdapter(AdapterHooks):
    name = "claude"
    executable = "claude"
    _options = {
        "model": "--model",
        "effort": "--effort",
        "permission_mode": "--permission-mode",
        "agent": "--agent",
        "fallback_model": "--fallback-model",
        "max_budget_usd": "--max-budget-usd",
    }

    _discussion_args = (
        "--safe-mode",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
    )

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        return option_args(request, self._options)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = restricted_options(request)
        argv = [
            resolve_executable(self.executable),
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
        ]
        if request.session_id is not None:
            argv.append(f"--resume={request.session_id}")
        argv.extend(self.option_args(request))
        if discussion:
            argv.extend(self._discussion_args)
        return PreparedCommand(
            tuple(argv), request.cwd, request.prompt.encode(), capture_line=_result_capture()
        )

    def event_model(self, event: dict[str, object]) -> str | None:
        return assistant_model(event)

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        event_type = event.get("type")
        if event_type == "system":
            subtype = event.get("subtype")
            if subtype == "api_retry":
                # Only the native error category, never the full error payload.
                return ProviderActivity(
                    "retry",
                    "system.api_retry",
                    "retrying",
                    message=brief(event.get("error")),
                    blocking_error=(
                        str(event["error"])
                        if event.get("error")
                        in (
                            "authentication_failed",
                            "permission_denied",
                            "insufficient_quota",
                            "billing_error",
                            "invalid_api_key",
                        )
                        else None
                    ),
                )
            if subtype == "status":
                return ProviderActivity("lifecycle", "system.status", brief(event.get("status")))
            if subtype in ("init", "hook_started", "hook_response"):
                return ProviderActivity("lifecycle", f"system.{subtype}", str(subtype))
        if event_type == "stream_event":
            nested = object_dict(event.get("event"))
            block = object_dict(nested.get("content_block"))
            delta = object_dict(nested.get("delta"))
            if nested.get("type") == "content_block_start":
                if block.get("type") == "tool_use":
                    return ProviderActivity(
                        "tool",
                        "stream_event.content_block_start",
                        "started",
                        brief(block.get("name")),
                        tool_call_id=brief(block.get("id")),
                    )
                if block.get("type") == "text":
                    return ProviderActivity(
                        "message",
                        "stream_event.text",
                        message=brief(block.get("text")),
                        public_output=brief(block.get("text"), 1000),
                        message_delta=True,
                    )
                if block.get("type") == "thinking":
                    return ProviderActivity("lifecycle", "stream_event.thinking", "reasoning")
            if delta.get("type") == "text_delta":
                return ProviderActivity(
                    "message",
                    "stream_event.text",
                    message=brief(delta.get("text")),
                    public_output=brief(delta.get("text"), 1000),
                    message_delta=True,
                )
        if event_type in ("assistant", "user"):
            content = object_dict(event.get("message")).get("content")
            if isinstance(content, list):
                for raw in reversed(cast(list[object], content)):
                    block = object_dict(raw)
                    block_type = block.get("type")
                    if block_type == "tool_use":
                        return ProviderActivity(
                            "tool",
                            "assistant.tool_use",
                            "started",
                            brief(block.get("name")),
                            tool_call_id=brief(block.get("id")),
                        )
                    if block_type == "tool_result":
                        state = "failed" if block.get("is_error") is True else "completed"
                        # A tool denied in non-interactive mode only fails that call; the
                        # CLI usually recovers with another tool, so it never wakes the boss.
                        return ProviderActivity(
                            "tool",
                            "user.tool_result",
                            state,
                            tool_call_id=brief(block.get("tool_use_id")),
                        )
                    if block_type == "text" and event_type == "assistant":
                        return ProviderActivity(
                            "message",
                            "assistant.text",
                            message=brief(block.get("text")),
                            public_output=brief(block.get("text"), 1000),
                        )
        if event_type == "result":
            # Denied or failed tool calls do not block; only a failed run does.
            failed = event.get("is_error") is True or (
                isinstance(event.get("subtype"), str) and str(event["subtype"]).startswith("error")
            )
            return ProviderActivity(
                "lifecycle",
                "result",
                brief(event.get("subtype")),
                blocking_error="provider_failed" if failed else None,
            )
        return None

    def _denials_warning(self, denials: list[object]) -> str:
        tools = {
            name
            for denial in denials
            if isinstance(name := object_dict(denial).get("tool_name"), str) and name
        }
        return (
            f"{self.name} denied {len(denials)} tool call(s)"
            + (f" ({', '.join(sorted(tools)[:5])})" if tools else "")
            + " but reported success; the task may be incomplete. Choose a permission tier "
            "for this CLI in the AgentNave App, or pass permission options explicitly."
        )

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        stdout_text = stdout.decode(errors="replace").strip()
        events = parse_json_lines(stdout_text)
        payload = next(
            (event for event in reversed(events) if event.get("type") == "result"),
            None,
        )

        if payload is None:
            session_id = next(
                (
                    value
                    for event in reversed(events)
                    if isinstance(value := event.get("session_id"), str)
                ),
                None,
            )
            return ParsedProviderResult(
                InvocationStatus.FAILED,
                "",
                session_id,
                error_message=f"{self.name} stream ended without a result event",
            )

        raw_output = payload.get("result", "")
        output = raw_output if isinstance(raw_output, str) else str(raw_output)
        session_id = payload.get("session_id")
        usage = normalized_usage(payload)
        is_error = bool(payload.get("is_error", False))
        subtype = payload.get("subtype")
        stop_reason = payload.get("stop_reason")
        errors = payload.get("errors", payload.get("error"))
        permission_denials = payload.get("permission_denials")

        if not isinstance(session_id, str):
            session_id = None
        has_permission_denials = False
        if isinstance(permission_denials, list):
            has_permission_denials = len(cast(list[object], permission_denials)) > 0
        structured_error = isinstance(subtype, str) and subtype.startswith("error")
        refused = isinstance(stop_reason, str) and stop_reason.lower() == "refusal"
        # A run the CLI reports as successful stays successful even if some tool calls were
        # denied on the way: the model may have finished the task another way.
        if returncode == 0 and not is_error and not structured_error and not refused:
            denials = (
                cast(list[object], permission_denials)
                if has_permission_denials
                else [
                    event["permission_denial"] for event in events if "permission_denial" in event
                ]
            )
            warnings = (self._denials_warning(denials),) if denials else ()
            return ParsedProviderResult(
                InvocationStatus.SUCCEEDED, output, session_id, usage, warnings=warnings
            )

        if refused:
            message = error_summary(output, f"{self.name} refused the request")
            status = InvocationStatus.BLOCKED
        elif has_permission_denials:
            message = f"{self.name} requires permission for one or more tool calls"
            status = InvocationStatus.BLOCKED
        else:
            raw_error = errors or subtype or output
            if raw_error == "success" and output:
                raw_error = output
            message = error_summary(
                str(raw_error or ""), f"{self.name} exited with status {returncode}"
            )
            status = (
                InvocationStatus.BLOCKED
                if message.lower().startswith("not logged in")
                else failure_status(message)
            )
        return ParsedProviderResult(status, output, session_id, usage, message)

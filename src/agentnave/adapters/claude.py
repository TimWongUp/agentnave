"""Transparent Claude Code CLI adapter."""

from __future__ import annotations

from typing import cast

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    brief,
    error_summary,
    failure_status,
    normalized_usage,
    object_dict,
    option_args,
    parse_json_lines,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity


class ClaudeAdapter:
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

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        argv = [
            self.executable,
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
        ]
        if request.session_id is not None:
            argv.append(f"--resume={request.session_id}")
        argv.extend(option_args(request, self._options))
        return PreparedCommand(tuple(argv), request.cwd, request.prompt.encode())

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
                        content = block.get("content")
                        texts = (
                            [content]
                            if isinstance(content, str)
                            else [
                                text
                                for part in cast(list[object], content)
                                if isinstance((text := object_dict(part).get("text")), str)
                                and object_dict(part).get("type") == "text"
                            ]
                            if isinstance(content, list)
                            else []
                        )
                        denied = any(
                            text.startswith("Error: Permission to use ")
                            and "permission prompts are not available in non-interactive mode"
                            in text
                            for text in texts
                        )
                        return ProviderActivity(
                            "tool",
                            "user.tool_result",
                            state,
                            tool_call_id=brief(block.get("tool_use_id")),
                            blocking_error="permission_denied" if denied else None,
                        )
                    if block_type == "text" and event_type == "assistant":
                        return ProviderActivity(
                            "message",
                            "assistant.text",
                            message=brief(block.get("text")),
                            public_output=brief(block.get("text"), 1000),
                        )
        if event_type == "result":
            denied = event.get("permission_denials")
            blocked = isinstance(denied, list) and len(cast(list[object], denied)) > 0
            failed = event.get("is_error") is True or (
                isinstance(event.get("subtype"), str) and str(event["subtype"]).startswith("error")
            )
            return ProviderActivity(
                "lifecycle",
                "result",
                brief(event.get("subtype")),
                blocking_error="permission_denied"
                if blocked
                else "provider_failed"
                if failed
                else None,
            )
        return None

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
        if (
            returncode == 0
            and not is_error
            and not structured_error
            and not has_permission_denials
            and not refused
        ):
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id, usage)

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
            status = failure_status(message)
        return ParsedProviderResult(status, output, session_id, usage, message)

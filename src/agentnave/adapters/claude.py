"""Transparent Claude Code CLI adapter."""

from __future__ import annotations

from typing import cast

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    error_summary,
    failure_status,
    normalized_usage,
    option_args,
    parse_json_lines,
)
from agentnave.models import InvocationRequest, InvocationStatus


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

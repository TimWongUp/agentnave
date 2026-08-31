"""Transparent Google Antigravity CLI adapter."""

from __future__ import annotations

import json

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    error_summary,
    failure_status,
    normalized_usage,
    object_dict,
    parse_json_lines,
)
from agentnave.models import InvocationRequest, InvocationStatus


class AntigravityAdapter:
    name = "antigravity"
    _options = {
        "model": "--model",
        "effort": "--effort",
        "agent": "--agent",
        "mode": "--mode",
        "project": "--project",
        "print_timeout": "--print-timeout",
        "sandbox": "--sandbox",
        "disable_slash_commands": "--disable-slash-commands",
    }
    _boolean_options = {"disable_slash_commands", "sandbox"}

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        message = {"event": "user", "message": {"content": request.prompt}}
        prompt = (json.dumps(message, ensure_ascii=False) + "\n").encode()
        argv = ["agy", "--input-format", "stream-json", "--output-format", "stream-json"]
        if request.session_id is not None:
            argv.extend(("--conversation", request.session_id))
        argv.extend(self._option_args(request))
        return PreparedCommand(tuple(argv), request.cwd, prompt)

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
        if (
            returncode == 0
            and normalized_status == "SUCCESS"
            and not explicit_error
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

    def _option_args(self, request: InvocationRequest) -> tuple[str, ...]:
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

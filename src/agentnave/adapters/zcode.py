"""ZCode's native app-server protocol via a supervised stdio bridge."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from uuid import uuid4

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    brief,
    error_summary,
    failure_status,
    normalized_usage,
    option_args,
    parse_json_lines,
    parse_json_object,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity


def _capture_line(line: bytes) -> bytes:
    event = parse_json_object(line.decode(errors="replace"))
    return line + b"\n" if event and event.get("type") in {"session", "result", "error"} else b""


class ZcodeAdapter:
    name = "zcode"

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        args = option_args(
            request, {"model": "model", "effort": "effort", "permission_mode": "mode"}
        )
        if "permission_mode" in request.provider_options and request.provider_options[
            "permission_mode"
        ] not in {"build", "edit", "plan", "yolo", "guarded", "auto"}:
            raise ValueError("unsupported zcode permission_mode")
        return args

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        self.option_args(request)
        session_id = request.session_id or f"sess_{uuid4()}"
        payload = {
            "prompt": request.prompt,
            "cwd": str(request.cwd),
            "session_id": session_id,
            "resume": request.session_id is not None,
            "options": dict(request.provider_options),
        }
        return PreparedCommand(
            (sys.executable, "-I", str(Path(__file__).parents[1] / "zcode_protocol.py")),
            request.cwd,
            json.dumps(payload).encode(),
            session_id=session_id,
            capture_line=_capture_line,
        )

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        kind = event.get("type")
        if kind == "text":
            return ProviderActivity(
                "message",
                "text",
                message=brief(event.get("text")),
                message_delta=True,
                public_output=brief(event.get("text"), 1000),
            )
        if kind == "tool":
            return ProviderActivity(
                "tool", "tool", brief(event.get("status")), brief(event.get("name"))
            )
        if kind in {"session", "turn.started", "turn.completed", "turn.failed"}:
            return ProviderActivity("lifecycle", str(kind), str(kind))
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        events = parse_json_lines(stdout.decode(errors="replace"))
        session_id = next(
            (str(e["session_id"]) for e in reversed(events) if e.get("session_id")), None
        )
        result = next((e for e in reversed(events) if e.get("type") == "result"), None)
        output = str(result.get("output", "")) if result else ""
        if returncode == 0 and result is not None and result.get("failed") is False:
            return ParsedProviderResult(
                InvocationStatus.SUCCEEDED, output, session_id, normalized_usage(result)
            )
        error = next((str(e["message"]) for e in reversed(events) if e.get("type") == "error"), "")
        error = (
            error
            or (str(result.get("error", "")) if result else "")
            or error_summary(stderr.decode(errors="replace"), "ZCode did not complete")
        )
        return ParsedProviderResult(failure_status(error), output, session_id, error_message=error)

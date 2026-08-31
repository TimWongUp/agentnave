"""Transparent Grok CLI adapter."""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

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


def _last_text_block(events: list[dict[str, object]]) -> str:
    last = ""
    current: list[str] = []
    for event in events:
        data = event.get("data")
        if event.get("type") == "text" and isinstance(data, str):
            current.append(data)
        elif current:
            last = "".join(current)
            current.clear()
    return "".join(current) or last


class GrokAdapter:
    name = "grok"
    _options = {
        "model": "--model",
        "effort": "--reasoning-effort",
        "permission_mode": "--permission-mode",
        "agent": "--agent",
        "max_turns": "--max-turns",
        "sandbox": "--sandbox",
    }

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        prompt = request.prompt.encode()
        fd, raw_path = tempfile.mkstemp(prefix="agentnave-prompt-", suffix=".txt")
        path = Path(raw_path)
        try:
            with os.fdopen(fd, "wb") as prompt_file:
                prompt_file.write(prompt)
        except Exception:
            path.unlink(missing_ok=True)
            raise
        argv = [
            "grok",
            "--prompt-file",
            str(path),
            "--output-format",
            "streaming-json",
            "--verbatim",
            "--cwd",
            str(request.cwd),
        ]
        if request.session_id is not None:
            argv.append(f"--resume={request.session_id}")
        try:
            argv.extend(option_args(request, self._options))
        except Exception:
            path.unlink(missing_ok=True)
            raise
        return PreparedCommand(tuple(argv), request.cwd, cleanup_paths=(path,))

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        stdout_text = stdout.decode(errors="replace").strip()
        events = parse_json_lines(stdout_text)
        output = _last_text_block(events)
        payload = next(
            (event for event in reversed(events) if event.get("type") in {"end", "error"}),
            None,
        )

        if payload is None:
            return ParsedProviderResult(
                InvocationStatus.FAILED,
                "",
                None,
                {},
                "grok stream ended without a terminal event",
            )

        session_id = payload.get("session_id", payload.get("sessionId"))
        usage = normalized_usage(payload)
        explicit_error = payload.get("error")
        stop_reason = payload.get("stopReason", payload.get("stop_reason"))
        if payload.get("type") == "error":
            explicit_error = payload.get("message", explicit_error or "grok reported an error")

        if not isinstance(session_id, str):
            session_id = None
        normal_stop = stop_reason is None or (
            isinstance(stop_reason, str) and stop_reason.replace("_", "").lower() == "endturn"
        )
        if returncode == 0 and not explicit_error and normal_stop:
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id, usage)

        raw_message = str(
            explicit_error
            or (f"grok stopped with reason {stop_reason}" if not normal_stop else "")
            or output
            or f"grok exited with status {returncode}"
        )
        message = error_summary(raw_message, f"grok exited with status {returncode}")
        status = (
            InvocationStatus.BLOCKED
            if isinstance(stop_reason, str) and stop_reason.lower() == "refusal"
            else failure_status(message)
        )
        return ParsedProviderResult(status, output, session_id, usage, message)

"""Transparent Devin CLI adapter over its ACP server (ADR-0070)."""

from __future__ import annotations

import json
import sys
from datetime import datetime
from pathlib import Path

from agentnave.adapters.base import (
    AdapterHooks,
    ParsedProviderResult,
    PreparedCommand,
    brief,
    clean_model,
    error_summary,
    failure_status,
    parse_json_lines,
    parse_json_object,
    restricted_options,
)
from agentnave.adapters.devin_acp import MODES
from agentnave.models import Host, InvocationRequest, InvocationStatus, ProviderActivity
from agentnave.native_prompts import NativePrompt, encode_answer, prompt_from_event
from agentnave.native_sessions import session_model as native_session_model

_RETAINED = frozenset({"session", "result", "error", "model"})


def _capture_line(line: bytes) -> bytes:
    """Retain only the events parse() reads, never replies or tool payloads."""
    event = parse_json_object(line.decode(errors="replace"))
    return line + b"\n" if event and event.get("type") in _RETAINED else b""


class DevinAdapter(AdapterHooks):
    name = "devin"
    options_description = (
        "permission_mode (accept-edits, smart, ask, plan, bypass, dangerous = bypass), "
        "sandbox (boolean); restricted_tools is not supported"
    )
    supports_restricted_tools = False
    _options = frozenset({"model", "permission_mode", "sandbox"})

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        """Validate the options the helper applies over ACP; there are no CLI flags."""
        options = request.provider_options
        unknown = sorted(set(options) - self._options)
        if unknown:
            raise ValueError(f"unsupported devin options: {', '.join(unknown)}")
        mode = options.get("permission_mode")
        if mode is not None and (not isinstance(mode, str) or mode not in MODES):
            raise ValueError("devin permission_mode must be one of: " + ", ".join(MODES))
        if "model" in options and (not isinstance(options["model"], str) or not options["model"]):
            raise ValueError("devin model must be a non-empty string")
        if "sandbox" in options and not isinstance(options["sandbox"], bool):
            raise ValueError("devin option sandbox must be a boolean")
        return ()

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, _ = restricted_options(request, self)
        self.option_args(request)
        payload = {
            "prompt": request.prompt,
            "cwd": str(request.cwd),
            "session_id": request.session_id,
            "options": dict(request.provider_options),
        }
        return PreparedCommand(
            (sys.executable, "-I", str(Path(__file__).with_name("devin_acp.py"))),
            request.cwd,
            (json.dumps(payload, ensure_ascii=False) + "\n").encode(),
            capture_line=_capture_line,
            # Permission requests are answered over the helper's stdin (ADR-0057).
            keep_stdin_open=True,
        )

    def session_model(self, session_id: str | None, *, since: datetime) -> str | None:
        return native_session_model(Host("devin"), session_id, since=since)

    def event_model(self, event: dict[str, object]) -> str | None:
        return clean_model(event.get("model")) if event.get("type") == "model" else None

    def prompt_event(self, event: dict[str, object]) -> NativePrompt | None:
        return prompt_from_event(event)

    def encode_prompt_answer(self, answer: dict[str, object]) -> bytes:
        return encode_answer(answer)

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        kind = event.get("type")
        if kind in ("session", "thinking"):
            # The model works from the session's start; thinking text is never forwarded.
            return ProviderActivity("lifecycle", str(kind), "reasoning")
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
                "tool",
                "tool",
                brief(event.get("status")),
                brief(event.get("name")),
                tool_call_id=brief(event.get("id")),
            )
        if kind == "result":
            return ProviderActivity("lifecycle", "result", brief(event.get("stop_reason")))
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        events = parse_json_lines(stdout.decode(errors="replace"))
        session_id = next(
            (
                value
                for event in reversed(events)
                if event.get("type") in ("session", "result")
                and isinstance(value := event.get("session_id"), str)
                and value
            ),
            None,
        )
        result = next((e for e in reversed(events) if e.get("type") == "result"), None)
        raw_output = result.get("output") if result is not None else None
        output = raw_output if isinstance(raw_output, str) else ""
        if returncode == 0 and result is not None and result.get("failed") is False:
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)

        stop = result.get("stop_reason") if result is not None else None
        raw_error = next(
            (str(e["message"]) for e in reversed(events) if e.get("type") == "error"), ""
        ) or (str(result.get("error") or "") if result is not None else "")
        message = error_summary(
            raw_error or stderr.decode(errors="replace"),
            f"devin exited with status {returncode}",
        )
        lowered = message.lower()
        if stop == "cancelled":
            status = InvocationStatus.CANCELLED
        elif stop == "refusal" or "not logged in" in lowered or "auth login" in lowered:
            status = InvocationStatus.BLOCKED
        else:
            status = failure_status(message)
        return ParsedProviderResult(status, output, session_id, error_message=message)

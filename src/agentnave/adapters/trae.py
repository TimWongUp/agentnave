"""TRAE CLI exec JSONL, independently decoded from its native ThreadEvent contract.

Evidence: public v0.213.1's exec/resume help and bundled serde ThreadEvent identifiers
(ThreadStartedEvent.thread_id, item.completed/agent_message/text, TurnCompletedEvent.usage,
TurnFailedEvent/error). These are not the app-server JSON-RPC or ACP events.
"""

from __future__ import annotations

import json

from agentnave.adapters.base import (
    AdapterHooks,
    ParsedProviderResult,
    PreparedCommand,
    brief,
    error_summary,
    failure_status,
    object_dict,
    parse_json_lines,
    parse_json_object,
    restricted_options,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity


def _capture_line(line: bytes) -> bytes:
    event = parse_json_object(line.decode(errors="replace"))
    if event is None:
        return b""
    kind = event.get("type")
    if kind == "thread.started":
        kept = {"type": kind, "thread_id": event.get("thread_id")}
    elif kind == "item.completed":
        item = object_dict(event.get("item"))
        if item.get("type") != "agent_message":
            return b""
        kept = {"type": kind, "item": {"type": "agent_message", "text": item.get("text")}}
    elif kind == "turn.completed":
        kept = {"type": kind, "usage": event.get("usage")}
    elif kind in ("turn.failed", "error"):
        kept = {key: event[key] for key in ("type", "error", "message") if key in event}
    else:
        return b""
    return json.dumps(kept, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"


class TraeAdapter(AdapterHooks):
    name = "trae"
    executable_name = "traecli"
    supports_restricted_tools = False
    options_description = (
        "model (native model ID), permission_mode (default, auto or bypass_permissions), "
        "sandbox (read-only, workspace-write or danger-full-access; new sessions only), "
        "skip_git_repo_check (boolean; always enabled for dispatched tasks)"
    )
    _options = {"model", "permission_mode", "sandbox", "skip_git_repo_check"}

    def supported_options(self) -> str:
        return self.options_description

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        unknown = sorted(set(request.provider_options) - self._options)
        if unknown:
            raise ValueError(f"unsupported trae options: {', '.join(unknown)}")
        argv: list[str] = []
        for key, value in request.provider_options.items():
            if key == "skip_git_repo_check":
                if not isinstance(value, bool):
                    raise ValueError("trae skip_git_repo_check must be a boolean")
                continue
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"trae {key} must be a nonempty string")
            if key == "permission_mode" and value not in {"default", "auto", "bypass_permissions"}:
                raise ValueError("trae permission_mode must be default, auto or bypass_permissions")
            if key == "sandbox":
                if value not in {"read-only", "workspace-write", "danger-full-access"}:
                    raise ValueError(
                        "trae sandbox must be read-only, workspace-write or danger-full-access"
                    )
                # Resume's help lists no --sandbox. Do not guess a config-key equivalent.
                if request.session_id is not None:
                    raise ValueError(
                        "trae exec resume does not support sandbox; use permission_mode"
                    )
            argv.extend(("--" + key.replace("_", "-"), value))
        return tuple(argv)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, _ = restricted_options(request, self)
        # v0.213.1 rejects --no-daemon for exec. Its native run stayed inside the
        # supervised process group in a local fake-endpoint probe; cloud auth is separate.
        argv = [self.executable_path(), "exec"]
        if request.session_id is not None:
            argv.append("resume")
        argv.extend(("--json", "--skip-git-repo-check", *self.option_args(request)))
        if request.session_id is not None:
            argv.append(request.session_id)
        argv.append("-")
        return PreparedCommand(
            tuple(argv),
            request.cwd,
            request.prompt.encode(),
            capture_line=_capture_line,
            # A failed resume may exit before thread.started; retain only the supplied ID.
            session_id=request.session_id,
        )

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        kind = event.get("type")
        if kind in ("thread.started", "turn.started", "turn.completed", "turn.failed"):
            return ProviderActivity(
                "lifecycle",
                str(kind),
                str(kind),
                blocking_error="provider_failed" if kind == "turn.failed" else None,
            )
        if kind == "error":
            return ProviderActivity("retry", "error", "retrying")
        if kind not in ("item.started", "item.updated", "item.completed"):
            return None
        item = object_dict(event.get("item"))
        item_type = item.get("type")
        if item_type == "agent_message":
            return ProviderActivity(
                "message",
                str(kind),
                message=brief(item.get("text")),
                public_output=brief(item.get("text"), 1000),
            )
        if item_type in ("command_execution", "mcp_tool_call", "web_search", "file_change"):
            return ProviderActivity(
                "tool",
                str(kind),
                brief(item.get("status")),
                brief(item.get("tool")) or str(item_type),
                tool_call_id=brief(item.get("id")),
            )
        if item_type == "reasoning":
            return ProviderActivity("lifecycle", str(kind), "reasoning")
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        events = parse_json_lines(stdout.decode(errors="replace"))
        session_id = None
        output = ""
        terminal: dict[str, object] | None = None
        raw_error = ""
        for event in events:
            kind = event.get("type")
            if kind == "thread.started" and isinstance(native_id := event.get("thread_id"), str):
                session_id = native_id or session_id
            elif kind == "item.completed":
                item = object_dict(event.get("item"))
                if item.get("type") == "agent_message" and isinstance(
                    text := item.get("text"), str
                ):
                    output = text
            elif kind in ("turn.completed", "turn.failed"):
                terminal = event
            if kind in ("turn.failed", "error"):
                error = event.get("error")
                raw_error = str(
                    object_dict(error).get("message") or event.get("message") or error or ""
                )
        if returncode == 0 and terminal is not None and terminal.get("type") == "turn.completed":
            if output or any(object_dict(terminal.get("usage")).values()):
                return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)
            raw_error = "trae turn completed without a reply or token usage"
        fallback = (
            "trae stream ended without a completion event"
            if terminal is None
            else f"trae exited with status {returncode}"
        )
        message = error_summary(raw_error or stderr.decode(errors="replace"), fallback)
        status = (
            InvocationStatus.BLOCKED
            if "not logged in" in message.lower()
            else failure_status(message)
        )
        return ParsedProviderResult(status, output, session_id, error_message=message)

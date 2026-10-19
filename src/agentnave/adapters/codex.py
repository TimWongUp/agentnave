"""Transparent Codex CLI adapter."""

from __future__ import annotations

import json
import re
import subprocess
import sys

from agentnave import __version__
from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    brief,
    error_summary,
    failure_status,
    object_dict,
    parse_json_lines,
    parse_json_object,
    resolve_executable,
    restricted_options,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity
from agentnave.processes import provider_environment

_DESKTOP_CODEX_PATHS = (
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "~/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "/Applications/Codex.app/Contents/Resources/codex",
    "~/Applications/Codex.app/Contents/Resources/codex",
)

# Tool items carry full command/tool output; activity sees them, the retained result does not.
_TOOL_ITEM = re.compile(
    rb'"type"\s*:\s*"(?:command_execution|mcp_tool_call|web_search|file_change|reasoning|todo_list)"'
)
_ITEM_EVENT = re.compile(rb'"type"\s*:\s*"item\.(?:started|updated|completed)"')
# Anything that may be a reply or a terminal event is never skipped.
_KEPT_EVENT = re.compile(rb'"type"\s*:\s*"(?:agent_message|turn\.|thread\.|error)')


def _capture_line(line: bytes) -> bytes:
    """Retain only the fields parse() reads, never tool item payloads."""
    event = parse_json_object(line.decode(errors="replace"))
    if event is None:
        return b""
    kind = event.get("type")
    if kind == "thread.started":
        kept: dict[str, object] = {"type": kind, "thread_id": event.get("thread_id")}
    elif kind == "item.completed":
        item = object_dict(event.get("item"))
        if item.get("type") != "agent_message":
            return b""
        kept = {"type": kind, "item": {"type": "agent_message", "text": item.get("text")}}
    elif kind == "turn.completed":
        kept = {"type": kind, "usage": event.get("usage")}
    elif kind in ("turn.failed", "error"):
        # parse() falls back from "error" to "message"; an absent key must stay absent.
        kept = {key: event[key] for key in ("type", "error", "message") if key in event}
    else:
        return b""
    return json.dumps(kept, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"


def _skip_oversized_line(head: bytes) -> bool:
    # Only a positively identified tool item event may be dropped; anything else fails.
    return (
        _ITEM_EVENT.search(head) is not None
        and _TOOL_ITEM.search(head) is not None
        and _KEPT_EVENT.search(head) is None
    )


def _codex_executable() -> str:
    # Desktop bundles stay ahead of PATH and the common install directories.
    return resolve_executable("codex", _DESKTOP_CODEX_PATHS if sys.platform == "darwin" else ())


def _capture_rpc_line(line: bytes) -> bytes:
    from agentnave.messaging import codex_event, rpc_line

    event = parse_json_object(line.decode(errors="replace"))
    return _capture_line(rpc_line(codex_event(event))) if event is not None else b""


def _require_rpc_version(executable: str) -> None:
    try:
        version = subprocess.run(
            (executable, "--version"),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=5,
            check=False,
            env=provider_environment((executable,)),
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ValueError("Codex messages require readable app-server version >= 0.99.0") from exc
    match = re.search(rb"(?:codex-cli\s+)?(\d+)\.(\d+)\.(\d+)", version.stdout)
    if (
        version.returncode != 0
        or match is None
        or tuple(int(part) for part in match.groups()) < (0, 99, 0)
    ):
        raise ValueError("Codex messages require app-server >= 0.99.0")


class CodexAdapter:
    name = "codex"
    _options = {
        "model",
        "effort",
        "skip_git_repo_check",
        "dangerously_bypass_approvals_and_sandbox",
    }

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        unknown = sorted(set(request.provider_options) - self._options)
        if unknown:
            raise ValueError(f"unsupported codex options: {', '.join(unknown)}")

        argv: list[str] = []
        for key, value in request.provider_options.items():
            if key == "model":
                argv.extend(("--model", str(value)))
            elif key == "effort":
                argv.extend(("--config", f"model_reasoning_effort={value}"))
            elif not isinstance(value, bool):
                raise ValueError(f"codex option {key} must be a boolean")
            elif value:
                argv.append("--" + key.replace("_", "-"))

        return tuple(argv)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = restricted_options(request)
        if request.enable_messages:
            from agentnave.messaging import rpc_line

            self.option_args(request)
            executable = _codex_executable()
            _require_rpc_version(executable)
            argv = [executable, "app-server"]
            if discussion:
                raise ValueError(
                    "Codex messages with restricted_tools are not yet supported; omit enable_messages"
                )
            return PreparedCommand(
                tuple(argv),
                request.cwd,
                stdin=rpc_line(
                    {
                        "id": "agentnave-initialize",
                        "method": "initialize",
                        "params": {
                            "clientInfo": {
                                "name": "agentnave",
                                "title": "AgentNave",
                                "version": __version__,
                            }
                        },
                    }
                ),
                capture_line=_capture_rpc_line,
                input_protocol="codex_rpc",
            )
        argv = [_codex_executable(), "exec"]
        if request.session_id is not None:
            argv.append("resume")
        argv.append("--json")
        if discussion:
            # Keep native auth and persistence, but omit user MCP/config and local read tools.
            argv.extend(("--ignore-user-config", "--skip-git-repo-check"))
            for feature in ("shell_tool", "unified_exec", "view_image", "apps", "multi_agent"):
                argv.extend(("--disable", feature))
            for setting in (
                'web_search="disabled"',
                'sandbox_mode="read-only"',
                'approval_policy="never"',
            ):
                argv.extend(("--config", setting))

        argv.extend(self.option_args(request))

        if request.session_id is not None:
            argv.append(request.session_id)
        argv.append("-")
        return PreparedCommand(
            tuple(argv),
            request.cwd,
            request.prompt.encode(),
            capture_line=_capture_line,
            skip_oversized_line=_skip_oversized_line,
        )

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        event_type = event.get("type")
        if event_type == "agentnave.approval_required":
            return ProviderActivity(
                "lifecycle", "approval_required", "blocked", blocking_error="permission_required"
            )
        if event_type in ("thread.started", "turn.started", "turn.completed", "turn.failed"):
            return ProviderActivity(
                "lifecycle",
                str(event_type),
                str(event_type),
                blocking_error="provider_failed" if event_type == "turn.failed" else None,
            )
        if event_type == "error":
            # Retries may still recover (for example via the HTTPS fallback), so never a blocker.
            return ProviderActivity("retry", "error", "retrying")
        if event_type not in ("item.started", "item.updated", "item.completed"):
            return None
        item = object_dict(event.get("item"))
        item_type = item.get("type")
        if item_type == "agent_message":
            return ProviderActivity(
                "message",
                str(event_type),
                message=brief(item.get("text")),
                public_output=brief(item.get("text"), 1000),
            )
        if item_type in ("command_execution", "mcp_tool_call", "web_search", "file_change"):
            return ProviderActivity(
                "tool",
                str(event_type),
                brief(item.get("status")),
                brief(item.get("tool")) or str(item_type),
                tool_call_id=brief(item.get("id")),
            )
        if item_type == "reasoning":
            return ProviderActivity("lifecycle", str(event_type), "reasoning")
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        events = parse_json_lines(stdout.decode(errors="replace").strip())
        session_id = next(
            (
                value
                for event in reversed(events)
                if event.get("type") == "thread.started"
                and isinstance(value := event.get("thread_id"), str)
            ),
            None,
        )
        output = next(
            (
                text
                for event in reversed(events)
                if event.get("type") == "item.completed"
                and (item := object_dict(event.get("item"))).get("type") == "agent_message"
                and isinstance(text := item.get("text"), str)
            ),
            "",
        )
        terminal = next(
            (
                event
                for event in reversed(events)
                if event.get("type") in ("turn.completed", "turn.failed")
            ),
            None,
        )

        if returncode == 0 and terminal is not None and terminal.get("type") == "turn.completed":
            # Codex can close a turn whose model response never arrived: no reply, zero usage.
            if not output and not any(object_dict(terminal.get("usage")).values()):
                return ParsedProviderResult(
                    InvocationStatus.FAILED,
                    output,
                    session_id,
                    error_message="codex turn completed without a reply or token usage",
                )
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)

        error_event = next(
            (event for event in reversed(events) if event.get("type") in ("turn.failed", "error")),
            None,
        )
        raw_error = ""
        if error_event is not None:
            error = error_event.get("error")
            error_mapping = object_dict(error)
            if error_mapping:
                raw_error = str(error_mapping.get("message", error_mapping))
            else:
                raw_error = str(error_event.get("message", error or ""))
        raw_error = raw_error or stderr.decode(errors="replace")
        fallback = (
            "codex stream ended without a completion event"
            if terminal is None
            else f"codex exited with status {returncode}"
        )
        message = error_summary(raw_error, fallback)
        # A turn that failed on a rejected credential needs the user to sign in again.
        status = (
            InvocationStatus.BLOCKED
            if "401 unauthorized" in message.lower()
            else failure_status(message)
        )
        return ParsedProviderResult(status, output, session_id, error_message=message)

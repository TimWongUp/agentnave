"""Transparent Codex CLI adapter."""

from __future__ import annotations

import glob
import json
import os
import re
import sys

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

_DESKTOP_CODEX_PATHS = (
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "~/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "/Applications/Codex.app/Contents/Resources/codex",
    "~/Applications/Codex.app/Contents/Resources/codex",
)

# Windows keeps the CLI in a plugin app-server bundle, and copies it under a content hash.
_WINDOWS_DESKTOP_CODEX_PATHS = (
    "~/.codex/plugins/.plugin-appserver/codex.exe",
    "~/AppData/Local/OpenAI/Codex/bin/*/codex.exe",
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
    return resolve_executable("codex", _bundled_paths())


def _bundled_paths() -> tuple[str, ...]:
    """The CLI a Codex desktop app ships, where the app keeps it instead of on PATH."""
    by_platform = {"darwin": _DESKTOP_CODEX_PATHS, "win32": _WINDOWS_DESKTOP_CODEX_PATHS}
    patterns = by_platform.get(sys.platform, ())
    paths: list[str] = []
    for pattern in patterns:
        expanded = os.path.expanduser(pattern)
        # A content-hash directory has no fixed name; anything else is a literal path.
        paths.extend(sorted(glob.glob(expanded)) if "*" in pattern else [expanded])
    return tuple(paths)


class CodexAdapter:
    name = "codex"
    _options = {
        "model",
        "effort",
        "skip_git_repo_check",
        "approve_for_me",
        "dangerously_bypass_approvals_and_sandbox",
        "sandbox",
    }
    _sandboxes = ("read-only", "workspace-write", "danger-full-access")

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        unknown = sorted(set(request.provider_options) - self._options)
        if unknown:
            raise ValueError(f"unsupported codex options: {', '.join(unknown)}")
        # approve_for_me brings its own approvals and sandbox; mixing would depend on order.
        if request.provider_options.get("approve_for_me") is True and (
            "sandbox" in request.provider_options
            or request.provider_options.get("dangerously_bypass_approvals_and_sandbox") is True
        ):
            raise ValueError(
                "codex approve_for_me cannot be combined with sandbox or "
                "dangerously_bypass_approvals_and_sandbox"
            )

        argv: list[str] = []
        for key, value in request.provider_options.items():
            if key == "model":
                argv.extend(("--model", str(value)))
            elif key == "effort":
                argv.extend(("--config", f"model_reasoning_effort={value}"))
            elif key == "sandbox":
                if value not in self._sandboxes:
                    raise ValueError("codex sandbox must be one of: " + ", ".join(self._sandboxes))
                # `exec resume` has no --sandbox flag; the config key works for both.
                argv.extend(("--config", f'sandbox_mode="{value}"'))
            elif not isinstance(value, bool):
                raise ValueError(f"codex option {key} must be a boolean")
            elif value and key == "approve_for_me" and request.session_id is not None:
                # `exec resume` has no --approve-for-me; these are the settings it applies.
                for setting in (
                    'approval_policy="on-request"',
                    'approvals_reviewer="auto_review"',
                    'sandbox_mode="workspace-write"',
                ):
                    argv.extend(("--config", setting))
            elif value:
                argv.append("--" + key.replace("_", "-"))

        return tuple(argv)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = restricted_options(request)
        argv = [_codex_executable(), "exec"]
        if request.session_id is not None:
            argv.append("resume")
        argv.append("--json")
        # Always allowed outside a Git repository: a dispatched task's cwd is chosen by the boss
        # and may not be a repo, and Codex otherwise refuses to run at all.
        argv.append("--skip-git-repo-check")
        if discussion:
            # Keep native auth and persistence, but omit user MCP/config and local read tools.
            argv.append("--ignore-user-config")
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

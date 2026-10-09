"""Transparent Codex CLI adapter."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    brief,
    discussion_options,
    error_summary,
    failure_status,
    object_dict,
    parse_json_lines,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity

_DESKTOP_CODEX_PATHS = (
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "~/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
    "/Applications/Codex.app/Contents/Resources/codex",
    "~/Applications/Codex.app/Contents/Resources/codex",
)


def _codex_executable() -> str:
    if sys.platform == "darwin":
        for candidate in _DESKTOP_CODEX_PATHS:
            executable = shutil.which(str(Path(candidate).expanduser()))
            if executable is not None:
                return executable
    return "codex"


class CodexAdapter:
    name = "codex"
    _options = {
        "model",
        "effort",
        "skip_git_repo_check",
        "dangerously_bypass_approvals_and_sandbox",
    }

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = discussion_options(request)
        unknown = sorted(set(request.provider_options) - self._options)
        if unknown:
            raise ValueError(f"unsupported codex options: {', '.join(unknown)}")

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

        for key, value in request.provider_options.items():
            if key == "model":
                argv.extend(("--model", str(value)))
            elif key == "effort":
                argv.extend(("--config", f"model_reasoning_effort={value}"))
            elif not isinstance(value, bool):
                raise ValueError(f"codex option {key} must be a boolean")
            elif value:
                argv.append("--" + key.replace("_", "-"))

        if request.session_id is not None:
            argv.append(request.session_id)
        argv.append("-")
        return PreparedCommand(tuple(argv), request.cwd, request.prompt.encode())

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        event_type = event.get("type")
        if event_type in ("thread.started", "turn.started", "turn.completed", "turn.failed"):
            return ProviderActivity(
                "lifecycle",
                str(event_type),
                str(event_type),
                blocking_error="provider_failed" if event_type == "turn.failed" else None,
            )
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
        return ParsedProviderResult(
            failure_status(message), output, session_id, error_message=message
        )

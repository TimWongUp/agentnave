"""Transparent Codex CLI adapter."""

from __future__ import annotations

from agentnave.adapters.base import (
    ParsedProviderResult,
    PreparedCommand,
    error_summary,
    failure_status,
    object_dict,
    parse_json_lines,
)
from agentnave.models import InvocationRequest, InvocationStatus


class CodexAdapter:
    name = "codex"
    executable = "codex"
    _options = {"model", "effort", "skip_git_repo_check"}

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        unknown = sorted(set(request.provider_options) - self._options)
        if unknown:
            raise ValueError(f"unsupported codex options: {', '.join(unknown)}")

        argv = [self.executable, "exec"]
        if request.session_id is not None:
            argv.append("resume")
        argv.append("--json")

        for key, value in request.provider_options.items():
            if key == "model":
                argv.extend(("--model", str(value)))
            elif key == "effort":
                argv.extend(("--config", f"model_reasoning_effort={value}"))
            elif not isinstance(value, bool):
                raise ValueError("codex option skip_git_repo_check must be a boolean")
            elif value:
                argv.append("--skip-git-repo-check")

        if request.session_id is not None:
            argv.append(request.session_id)
        argv.append("-")
        return PreparedCommand(tuple(argv), request.cwd, request.prompt.encode())

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
                if event.get("type") in {"turn.completed", "turn.failed"}
            ),
            None,
        )

        if returncode == 0 and terminal is not None and terminal.get("type") == "turn.completed":
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)

        error_event = next(
            (event for event in reversed(events) if event.get("type") in {"turn.failed", "error"}),
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

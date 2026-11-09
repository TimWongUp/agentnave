"""Transparent Claude Code CLI adapter."""

from __future__ import annotations

import json
import os
import re
import sys
from collections.abc import Callable
from dataclasses import replace
from typing import cast

from agentnave.adapters.base import (
    AdapterHooks,
    ParsedProviderResult,
    PreparedCommand,
    brief,
    error_summary,
    failure_status,
    object_dict,
    option_args,
    parse_json_lines,
    parse_json_object,
    resolve_executable,
    restricted_options,
)
from agentnave.models import InvocationRequest, InvocationStatus, ProviderActivity
from agentnave.native_prompts import NativePrompt, encode_answer, prompt_from_event
from agentnave.native_sessions import assistant_model

# CodeBuddy leaves permission_denials empty and reports a denial only in the tool result.
_DENIED = re.compile(r"Error: Permission to use (\S{1,64}) has been denied")


def _denied_tools(event: dict[str, object]) -> list[str]:
    if event.get("type") != "user" or event.get("parent_tool_use_id") is not None:
        return []
    content = object_dict(event.get("message")).get("content")
    names: list[str] = []
    for raw in cast(list[object], content) if isinstance(content, list) else []:
        block = object_dict(raw)
        if block.get("type") != "tool_result":
            continue
        parts = block.get("content")
        texts = (
            [object_dict(part).get("text") for part in cast(list[object], parts)]
            if isinstance(parts, list)
            else [parts]
        )
        for text in texts:
            if isinstance(text, str) and (match := _DENIED.match(text)):
                names.append(match.group(1))
    return names


_MODES = frozenset({"auto", "bypassPermissions"})


def _result_capture() -> Callable[[bytes], bytes]:
    last_session: str | None = None

    def capture(line: bytes) -> bytes:
        nonlocal last_session
        event = parse_json_object(line.decode(errors="replace"))
        if event is None:
            return b""
        if event.get("type") == "result":
            return line + b"\n"
        denied = b"".join(
            json.dumps({"permission_denial": {"tool_name": name}}).encode() + b"\n"
            for name in _denied_tools(event)
        )
        if denied:
            return denied
        # Keep only session changes for parse's fallback, not deltas or tool payloads.
        session_id = event.get("session_id")
        if not isinstance(session_id, str) or session_id == last_session:
            return b""
        last_session = session_id
        return json.dumps({"session_id": session_id}).encode() + b"\n"

    return capture


class ClaudeAdapter(AdapterHooks):
    name = "claude"
    options_description = "permission_mode, permission_fallback (auto or bypassPermissions; set by the permission tier), agent, fallback_model, max_budget_usd, interactive (boolean; interactive Claude runtime), restricted_tools (boolean; restricted tools)"
    executable = "claude"
    # Claude-only extras: the interactive runtime and the permission tier's fallback mode.
    _supports_interactive = True
    restricted_extra_options = frozenset({"interactive"})
    _options = {
        "model": "--model",
        "effort": "--effort",
        "permission_mode": "--permission-mode",
        "agent": "--agent",
        "fallback_model": "--fallback-model",
        "max_budget_usd": "--max-budget-usd",
    }

    _discussion_args = (
        "--safe-mode",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
    )

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        if self._supports_interactive:
            options = dict(request.provider_options)
            interactive = options.pop("interactive", False)
            if not isinstance(interactive, bool):
                raise ValueError("interactive must be a boolean")
            fallback = options.pop("permission_fallback", None)
            if fallback is not None and (
                fallback not in _MODES or options.get("permission_mode") not in _MODES - {fallback}
            ):
                raise ValueError(
                    "permission_fallback must be auto or bypassPermissions, "
                    "with permission_mode set to the other one"
                )
            request = replace(request, provider_options=options)
        return option_args(request, self._options)

    def executable_path(self) -> str:
        return resolve_executable(self.executable)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = restricted_options(request, self)
        options = self.option_args(request)
        fallback = request.provider_options.get("permission_fallback")
        if (
            self._supports_interactive
            and request.provider_options.get("interactive") is True
            and not discussion
            and os.name != "nt"
        ):
            argv = [
                sys.executable,
                "-I",
                "-m",
                "agentnave.claude_tui",
                "run",
                "--claude",
                self.executable_path(),
            ]
            if request.session_id is not None:
                argv.extend(("--resume", request.session_id))
            if isinstance(fallback, str):
                argv.extend(("--fallback", fallback))
            argv.extend(("--", *options))
            return PreparedCommand(
                tuple(argv),
                request.cwd,
                (json.dumps({"prompt": request.prompt}, ensure_ascii=False) + "\n").encode(),
                capture_line=_result_capture(),
                keep_stdin_open=True,
            )
        argv = [
            self.executable_path(),
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
        ]
        if request.session_id is not None:
            argv.append(f"--resume={request.session_id}")
        argv.extend(options)
        if discussion:
            argv.extend(self._discussion_args)
        elif isinstance(fallback, str) and os.path.isabs(argv[0]):
            # A missing CLI keeps the direct command, so its launch error stays a launch error.
            claude = argv.pop(0)
            argv = [
                sys.executable,
                "-I",
                "-m",
                "agentnave.claude_tui",
                "print",
                "--claude",
                claude,
                "--fallback",
                fallback,
                "--",
                *argv,
            ]
        return PreparedCommand(
            tuple(argv), request.cwd, request.prompt.encode(), capture_line=_result_capture()
        )

    def prompt_event(self, event: dict[str, object]) -> NativePrompt | None:
        return prompt_from_event(event)

    def encode_prompt_answer(self, answer: dict[str, object]) -> bytes:
        return encode_answer(answer)

    def event_model(self, event: dict[str, object]) -> str | None:
        return assistant_model(event)

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        event_type = event.get("type")
        if event_type == "system":
            return self._system_activity(event)
        if event_type == "stream_event":
            nested = object_dict(event.get("event"))
            block = object_dict(nested.get("content_block"))
            delta = object_dict(nested.get("delta"))
            if nested.get("type") == "message_start":
                return ProviderActivity("lifecycle", "stream_event.message_start", "reasoning")
            if nested.get("type") == "content_block_start":
                if block.get("type") == "tool_use":
                    return ProviderActivity(
                        "tool",
                        "stream_event.content_block_start",
                        "started",
                        brief(block.get("name")),
                        tool_call_id=brief(block.get("id")),
                    )
                if block.get("type") == "text":
                    return ProviderActivity(
                        "message",
                        "stream_event.text",
                        message=brief(block.get("text")),
                        public_output=brief(block.get("text"), 1000),
                        message_delta=True,
                    )
                if block.get("type") == "thinking":
                    return ProviderActivity("lifecycle", "stream_event.thinking", "reasoning")
            if delta.get("type") == "text_delta":
                return ProviderActivity(
                    "message",
                    "stream_event.text",
                    message=brief(delta.get("text")),
                    public_output=brief(delta.get("text"), 1000),
                    message_delta=True,
                )
        if event_type in ("assistant", "user"):
            content = object_dict(event.get("message")).get("content")
            if isinstance(content, list):
                for raw in reversed(cast(list[object], content)):
                    block = object_dict(raw)
                    block_type = block.get("type")
                    if block_type == "tool_use":
                        return ProviderActivity(
                            "tool",
                            "assistant.tool_use",
                            "started",
                            brief(block.get("name")),
                            tool_call_id=brief(block.get("id")),
                        )
                    if block_type == "tool_result":
                        state = "failed" if block.get("is_error") is True else "completed"
                        # A tool denied in non-interactive mode only fails that call; the
                        # CLI usually recovers with another tool, so it never wakes the boss.
                        return ProviderActivity(
                            "tool",
                            "user.tool_result",
                            state,
                            tool_call_id=brief(block.get("tool_use_id")),
                        )
                    if block_type == "text" and event_type == "assistant":
                        return ProviderActivity(
                            "message",
                            "assistant.text",
                            message=brief(block.get("text")),
                            public_output=brief(block.get("text"), 1000),
                        )
                    if block_type == "thinking" and event_type == "assistant":
                        return ProviderActivity("lifecycle", "assistant.thinking", "reasoning")
        if event_type == "result":
            # Denied or failed tool calls do not block; only a failed run does.
            failed = event.get("is_error") is True or (
                isinstance(event.get("subtype"), str) and str(event["subtype"]).startswith("error")
            )
            raw_error = event.get("errors") or event.get("error") or event.get("result")
            return ProviderActivity(
                "lifecycle",
                "result",
                brief(event.get("subtype")),
                message=brief(str(raw_error)) if failed and raw_error else None,
                blocking_error="provider_failed" if failed else None,
            )
        return None

    def _system_activity(self, event: dict[str, object]) -> ProviderActivity | None:
        subtype = event.get("subtype")
        if subtype == "api_retry":
            # Only the native error category, never the full error payload.
            return ProviderActivity(
                "retry",
                "system.api_retry",
                "retrying",
                message=brief(event.get("error")),
                blocking_error=(
                    str(event["error"])
                    if event.get("error")
                    in (
                        "authentication_failed",
                        "permission_denied",
                        "insufficient_quota",
                        "billing_error",
                        "invalid_api_key",
                    )
                    else None
                ),
            )
        if subtype == "status":
            return ProviderActivity("lifecycle", "system.status", brief(event.get("status")))
        if subtype in ("init", "hook_started", "hook_response"):
            return ProviderActivity("lifecycle", f"system.{subtype}", str(subtype))
        return None

    def _denials_warning(self, denials: list[object]) -> str:
        tools = {
            name
            for denial in denials
            if isinstance(name := object_dict(denial).get("tool_name"), str) and name
        }
        return (
            f"{self.name} denied {len(denials)} tool call(s)"
            + (f" ({', '.join(sorted(tools)[:5])})" if tools else "")
            + " but reported success; the task may be incomplete. Choose a permission tier "
            "for this CLI in the AgentNave App, or pass permission options explicitly."
        )

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
        is_error = bool(payload.get("is_error", False))
        subtype = payload.get("subtype")
        stop_reason = payload.get("stop_reason")
        errors = payload.get("errors", payload.get("error"))
        permission_denials = payload.get("permission_denials")
        raw_warnings = payload.get("warnings")
        runtime_warnings = (
            tuple(w for w in cast(list[object], raw_warnings) if isinstance(w, str))
            if isinstance(raw_warnings, list)
            else ()
        )

        if not isinstance(session_id, str):
            session_id = None
        has_permission_denials = False
        if isinstance(permission_denials, list):
            has_permission_denials = len(cast(list[object], permission_denials)) > 0
        structured_error = isinstance(subtype, str) and subtype.startswith("error")
        refused = isinstance(stop_reason, str) and stop_reason.lower() == "refusal"
        # A run the CLI reports as successful stays successful even if some tool calls were
        # denied on the way: the model may have finished the task another way.
        if returncode == 0 and not is_error and not structured_error and not refused:
            denials = (
                cast(list[object], permission_denials)
                if has_permission_denials
                else [
                    event["permission_denial"] for event in events if "permission_denial" in event
                ]
            )
            warnings = runtime_warnings + ((self._denials_warning(denials),) if denials else ())
            return ParsedProviderResult(
                InvocationStatus.SUCCEEDED, output, session_id, warnings=warnings
            )

        if refused:
            message = f"{self.name} refused the request"
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
            status = (
                InvocationStatus.BLOCKED
                if message.lower().startswith("not logged in")
                else failure_status(message)
            )
        return ParsedProviderResult(status, output, session_id, message, runtime_warnings)

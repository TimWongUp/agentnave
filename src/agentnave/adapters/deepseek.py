"""DeepSeek Harness headless adapter."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from dataclasses import replace
from pathlib import Path

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

_DESKTOP_DSH_PATHS = (
    "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh",
    "~/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh",
)
_PERMISSION_MODES = ("read-only", "workspace-write", "danger-full-access")
# Headless disables the GUI credential store; these rows are the documented restore.
_STORED_CREDENTIAL_ROWS = ("credentials", "authorization", "deepseek-account")
# Tool plugin rows of the shipped base profile, verified against dsh 0.2.1.
_TOOL_ROWS = (
    "tool-bash",
    "tool-pwsh",
    "tool-jobs",
    "tool-fs",
    "tool-fs-search",
    "tool-skill",
    "tool-subagent-control",
    "tool-subagent-list-agents",
    "tool-subagent",
    "tool-subagent-fork",
    "tool-workflow",
    "tool-todo",
    "tool-goal",
    "tool-web",
    "tool-working-directory",
    "mcp-resources",
    "plan-mode",
)
_BLOCKED_CODES = {"MISSING_CREDENTIAL", "INVALID_CREDENTIAL", "AUTH"}
_OFFICIAL_ROUTE = "deepseek-official"


def _scalar(raw: str) -> str:
    """Unquote one plain, single- or double-quoted YAML scalar from --dump-config."""
    raw = raw.strip()
    if raw.startswith('"'):
        try:
            value = json.loads(raw)
        except json.JSONDecodeError:
            return ""
        return value if isinstance(value, str) else ""
    if raw.startswith("'") and raw.endswith("'") and len(raw) > 1:
        return raw[1:-1].replace("''", "'")
    return raw


def profile_routes(executable: str) -> tuple[tuple[str, str] | None, dict[str, str]]:
    """The headless profile's composed agent-default-model (route, model), if readable, and
    the route of each model its configured providers declare (first declaration wins).

    dsh replaces that entry's config as a whole, so a model override must restate a route.
    """
    try:
        dump = subprocess.run(
            [executable, "--profile", "headless", "--dump-config"],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None, {}
    found: dict[str, str] = {}
    routes: dict[str, str] = {}
    inside = providers = False
    route = ""
    for line in dump.splitlines():
        if line.startswith("- ") or line.startswith("#"):
            inside = line.strip() == "- id: agent-default-model"
            if inside:
                found = {}
            providers = False
        elif line.startswith("    ") and not line.startswith("     "):
            key, _, value = line.strip().partition(":")
            providers, route = key == "providers", ""
            if inside and key in ("provider", "model"):
                found[key] = _scalar(value)
        elif not providers:
            continue
        elif line.startswith("      ") and not line.startswith("       "):
            key = line.strip()
            route = _scalar(key[:-1]) if key.endswith(":") else ""
        elif route and line.startswith("          - id:"):
            model = _scalar(line.removeprefix("          - id:"))
            if model:
                routes.setdefault(model, route)
    default_route, default_model = found.get("provider"), found.get("model")
    default = (default_route, default_model) if default_route and default_model else None
    return default, routes


def _quote(value: object) -> str:
    # A JSON string is a valid YAML double-quoted scalar.
    return json.dumps(str(value), ensure_ascii=False)


def _capture_line(line: bytes) -> bytes:
    """Retain only the events parse() reads, never tool payloads."""
    event = parse_json_object(line.decode(errors="replace"))
    if event is None:
        return b""
    kind = event.get("type")
    if kind in ("session", "final", "error"):
        return line + b"\n"
    if kind == "status" and event.get("phase") == "turn_end":
        kept = {"type": kind, "phase": "turn_end", "reason": event.get("reason")}
        return json.dumps(kept, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"
    return b""


class DeepSeekAdapter:
    name = "deepseek"
    _options = ("model", "effort", "provider", "permission_mode", "stored_credentials")

    def executable(self) -> str:
        return resolve_executable("dsh", _DESKTOP_DSH_PATHS if sys.platform == "darwin" else ())

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        """Return the --patch overlay rows; dsh exposes these settings only as profile rows."""
        options = dict(request.provider_options)
        unknown = sorted(set(options) - set(self._options))
        if unknown:
            raise ValueError(f"unsupported deepseek options: {', '.join(unknown)}")
        rows: list[str] = []
        if "model" in options:
            rows.append(
                "- id: agent-default-model\n  config:\n"
                f"    provider: {_quote(options.get('provider', _OFFICIAL_ROUTE))}\n"
                f"    model: {_quote(options['model'])}\n"
                + (
                    f"    reasoningEffort: {_quote(options['effort'])}\n"
                    if "effort" in options
                    else ""
                )
            )
        elif "effort" in options or "provider" in options:
            raise ValueError("deepseek effort and provider require model")
        if "permission_mode" in options:
            mode = options["permission_mode"]
            if mode not in _PERMISSION_MODES:
                raise ValueError(
                    "deepseek permission_mode must be one of " + ", ".join(_PERMISSION_MODES)
                )
            # Same sandbox and approval pair the shipped profile derives from DSH_PERMISSION_MODE.
            rows.append(
                "- id: sandbox-policy\n  config:\n"
                f"    mode: {mode}\n    workspaceRoot: !!js process.cwd()\n"
                "- id: approval\n  config:\n"
                f"    policy: {'never' if mode == 'danger-full-access' else 'ask'}\n"
            )
        stored = options.get("stored_credentials", False)
        if not isinstance(stored, bool):
            raise ValueError("deepseek stored_credentials must be a boolean")
        if stored:
            rows.extend(f"- id: {row}\n  disabled: false\n" for row in _STORED_CREDENTIAL_ROWS)
        return tuple(rows)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, discussion = restricted_options(request)
        options = dict(request.provider_options)
        route = options.get("provider")
        if route is None:
            # The profile's default model keeps its own route (a gateway, say), a model a
            # configured provider declares takes that provider's route, and any other bare
            # model is one of dsh's bundled official models.
            default, routes = profile_routes(self.executable())
            model = options.get("model")
            if model is None:
                route = default[0] if default is not None else None
            else:
                route = (
                    default[0]
                    if default is not None and default[1] == model
                    else routes.get(str(model), _OFFICIAL_ROUTE)
                )
                options["provider"] = route
        if route == _OFFICIAL_ROUTE and "stored_credentials" not in options:
            # Headless leaves the app's saved keys off; the official route needs them unless
            # the caller decides otherwise (ADR-0027).
            options["stored_credentials"] = True
        request = replace(request, provider_options=options)
        rows = list(self.option_args(request))
        if discussion:
            rows.extend(f"- id: {row}\n  disabled: true\n" for row in _TOOL_ROWS)
            rows.append(
                "- id: sandbox-policy\n  config:\n"
                "    mode: read-only\n    workspaceRoot: !!js process.cwd()\n"
            )
        argv = [
            self.executable(),
            "--profile",
            "headless",
        ]
        cleanup: tuple[Path, ...] = ()
        if rows:
            fd, raw_path = tempfile.mkstemp(prefix="agentnave-dsh-", suffix=".yml")
            path = Path(raw_path)
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as patch:
                    patch.write("".join(rows))
            except Exception:
                path.unlink(missing_ok=True)
                raise
            argv.extend(("--patch", str(path)))
            cleanup = (path,)
        argv.append("--json")
        if request.session_id is not None:
            argv.extend(("--session-id", request.session_id))
        # Without a positional task, headless reads the whole task verbatim from stdin.
        return PreparedCommand(
            tuple(argv),
            request.cwd,
            stdin=request.prompt.encode(),
            cleanup_paths=cleanup,
            capture_line=_capture_line,
        )

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        kind = event.get("type")
        if kind == "tool_call":
            return ProviderActivity(
                "tool",
                "tool_call",
                "running",
                brief(event.get("tool")),
                tool_call_id=brief(event.get("callId")),
            )
        if kind == "tool_result":
            return ProviderActivity(
                "tool",
                "tool_result",
                brief(event.get("status")),
                tool_call_id=brief(event.get("callId")),
            )
        if kind == "text":
            # Each text event is one committed assistant message, not a token delta.
            return ProviderActivity(
                "message",
                "text",
                message=brief(event.get("text")),
                public_output=brief(event.get("text"), 1000),
            )
        if kind == "thinking":
            return ProviderActivity("lifecycle", "thinking", "reasoning")
        if kind == "status":
            phase = brief(event.get("phase"))
            if phase == "turn_end":
                return ProviderActivity(
                    "lifecycle", "turn_end", brief(object_dict(event.get("reason")).get("kind"))
                )
            return ProviderActivity("lifecycle", "status", phase) if phase else None
        if kind == "error":
            return ProviderActivity("lifecycle", "error", "error", blocking_error="provider_failed")
        return None

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        events = parse_json_lines(stdout.decode(errors="replace"))
        stderr_text = stderr.decode(errors="replace").strip()
        session_id = next(
            (
                value
                for event in events
                if event.get("type") == "session"
                and isinstance(value := event.get("sessionId"), str)
                and value
            ),
            None,
        )
        final = next((event for event in reversed(events) if event.get("type") == "final"), None)
        raw_output = final.get("text") if final is not None else None
        output = raw_output if isinstance(raw_output, str) else ""
        # Later turns may follow child settlement; the last turn_end decides the run.
        reason = object_dict(
            next(
                (
                    event.get("reason")
                    for event in reversed(events)
                    if event.get("type") == "status" and event.get("phase") == "turn_end"
                ),
                None,
            )
        )
        kind = reason.get("kind")
        if returncode == 0 and final is not None and kind == "completed":
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)

        error = object_dict(reason.get("error"))
        runner_error = next(
            (event for event in reversed(events) if event.get("type") == "error"), None
        )
        if not error and runner_error is not None:
            error = runner_error
        code = error.get("code")
        raw_message = error.get("message")
        fallback = (
            f"deepseek turn ended with reason {kind}"
            if isinstance(kind, str) and kind not in ("completed", "error")
            else "deepseek stream ended without a completed turn"
            if returncode == 0
            else f"deepseek exited with status {returncode}"
        )
        message = error_summary(
            raw_message if isinstance(raw_message, str) else stderr_text, fallback
        )
        if kind == "aborted":
            status = InvocationStatus.CANCELLED
        elif code in _BLOCKED_CODES:
            status = InvocationStatus.BLOCKED
        else:
            status = failure_status(message)
        return ParsedProviderResult(status, output, session_id, error_message=message)

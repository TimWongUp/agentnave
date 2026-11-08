"""Transparent Devin CLI adapter."""

from __future__ import annotations

import os
import tempfile
from datetime import datetime
from pathlib import Path

from agentnave.adapters.base import (
    AdapterHooks,
    ParsedProviderResult,
    PreparedCommand,
    error_summary,
    failure_status,
    parse_json_object,
    restricted_options,
)
from agentnave.models import Host, InvocationRequest, InvocationStatus, ProviderActivity
from agentnave.native_sessions import extract_devin_turn
from agentnave.native_sessions import session_model as native_session_model

_PERMISSION_MODES = frozenset(
    {
        "normal",
        "auto",
        "accept-edits",
        "smart",
        "dangerous",
        "yolo",
        "bypass",
        "autonomous",
    }
)


def _looks_like_atif(payload: dict[str, object] | None) -> bool:
    if not payload:
        return False
    return isinstance(payload.get("steps"), list) or (
        isinstance(payload.get("schema_version"), str)
        and str(payload.get("schema_version")).startswith("ATIF")
    )


class DevinAdapter(AdapterHooks):
    name = "devin"
    options_description = (
        "permission_mode (normal, auto, accept-edits, smart, dangerous, yolo, bypass, "
        "autonomous), sandbox (boolean), respect_workspace_trust (boolean); "
        "restricted_tools is not supported"
    )
    supports_restricted_tools = False
    _options = frozenset({"model", "permission_mode", "sandbox", "respect_workspace_trust"})

    def __init__(self) -> None:
        self._reported_model: str | None = None

    def option_args(self, request: InvocationRequest) -> tuple[str, ...]:
        unknown = sorted(set(request.provider_options) - self._options)
        if unknown:
            raise ValueError(f"unsupported devin options: {', '.join(unknown)}")
        mode = request.provider_options.get("permission_mode")
        if mode is not None:
            if not isinstance(mode, str) or mode not in _PERMISSION_MODES:
                raise ValueError(
                    "devin permission_mode must be one of: " + ", ".join(sorted(_PERMISSION_MODES))
                )
            if mode == "autonomous" and request.provider_options.get("sandbox") is not True:
                raise ValueError("devin autonomous permission_mode requires sandbox=true")
        args: list[str] = []
        for key, value in request.provider_options.items():
            if key == "model":
                args.extend(("--model", str(value)))
            elif key == "permission_mode":
                args.extend(("--permission-mode", str(value)))
            elif key == "sandbox":
                if not isinstance(value, bool):
                    raise ValueError("devin option sandbox must be a boolean")
                if value:
                    args.append("--sandbox")
            elif key == "respect_workspace_trust":
                if not isinstance(value, bool):
                    raise ValueError("devin option respect_workspace_trust must be a boolean")
                args.extend(("--respect-workspace-trust", "true" if value else "false"))
        return tuple(args)

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        request, _ = restricted_options(request, self)
        options = self.option_args(request)
        prompt_fd, raw_prompt = tempfile.mkstemp(prefix="agentnave-devin-prompt-", suffix=".txt")
        prompt_path = Path(raw_prompt)
        try:
            with os.fdopen(prompt_fd, "wb") as stream:
                stream.write(request.prompt.encode("utf-8"))
        except Exception:
            prompt_path.unlink(missing_ok=True)
            raise
        export_fd, raw_export = tempfile.mkstemp(prefix="agentnave-devin-export-", suffix=".json")
        os.close(export_fd)
        export_path = Path(raw_export)
        argv = [
            self.executable_path(),
            "-p",
            "--prompt-file",
            str(prompt_path),
            "--export",
            str(export_path),
        ]
        if request.session_id is not None:
            argv.extend(("--resume", request.session_id))
        argv.extend(options)
        return PreparedCommand(
            tuple(argv),
            request.cwd,
            cleanup_paths=(prompt_path, export_path),
        )

    def session_model(self, session_id: str | None, *, since: datetime) -> str | None:
        return self._reported_model or native_session_model(Host("devin"), session_id, since=since)

    def activity(self, event: dict[str, object]) -> ProviderActivity | None:
        return None

    def parse_prepared(
        self, prepared: PreparedCommand, returncode: int, stdout: bytes, stderr: bytes
    ) -> ParsedProviderResult:
        try:
            export = parse_json_object(
                prepared.cleanup_paths[-1].read_text(encoding="utf-8", errors="replace")
            )
        except (OSError, IndexError):
            export = None
        return self._parse_result(returncode, stdout, stderr, export)

    def parse(self, returncode: int, stdout: bytes, stderr: bytes) -> ParsedProviderResult:
        return self._parse_result(returncode, stdout, stderr, None)

    def _parse_result(
        self, returncode: int, stdout: bytes, stderr: bytes, export: dict[str, object] | None
    ) -> ParsedProviderResult:
        stdout_text = stdout.decode("utf-8", errors="replace").strip()
        stderr_text = stderr.decode("utf-8", errors="replace").strip()
        stdout_json = parse_json_object(stdout_text) if stdout_text.startswith("{") else None
        if export is None and _looks_like_atif(stdout_json):
            export = stdout_json
            stdout_text = ""
        session_id, export_output, model = extract_devin_turn(export)
        self._reported_model = model
        output = stdout_text or export_output
        lowered_stderr = stderr_text.lower()
        rejected_tool = "rejected a tool call that requires confirmation" in lowered_stderr
        if (
            returncode == 0
            and (output or session_id is not None)
            and not (rejected_tool and not output)
        ):
            return ParsedProviderResult(InvocationStatus.SUCCEEDED, output, session_id)

        raw_message = (
            stderr_text
            or (output if returncode != 0 else "")
            or (
                "devin output was empty"
                if returncode == 0
                else f"devin exited with status {returncode}"
            )
        )
        message = error_summary(raw_message, f"devin exited with status {returncode}")
        lowered = f"{stderr_text}\n{message}".lower()
        blocked = (
            rejected_tool
            or "not logged in" in lowered
            or "auth login" in lowered
            or "workspace trust" in lowered
            or failure_status(lowered) is InvocationStatus.BLOCKED
        )
        status = InvocationStatus.BLOCKED if blocked else failure_status(message)
        return ParsedProviderResult(status, output, session_id, error_message=message)

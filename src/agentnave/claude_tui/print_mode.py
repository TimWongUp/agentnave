"""`claude --print` with the permission tier's fallback mode (ADR-0059)."""

from __future__ import annotations

import contextlib
import json
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

from agentnave.claude_tui.hooks import (
    mode_hook,
    mode_of,
    mode_warning,
    modes_unavailable,
    with_mode,
)
from agentnave.claude_tui.turn import Transcript, drop_empty_session, object_dict


def write_line(event: dict[str, object]) -> None:
    # Bytes, so a Windows console encoding never fails on the CLI's UTF-8 output.
    sys.stdout.buffer.write((json.dumps(event, ensure_ascii=False) + "\n").encode())
    sys.stdout.buffer.flush()


def print_attempt(
    executable: str, args: list[str], prompt: bytes
) -> tuple[int, dict[str, object] | None, str, bool]:
    """Streams one -p run through, holding back its result line. Returns the exit status,
    the result, and the permission mode with whether the mode check held the prompt."""
    result: dict[str, object] | None = None
    with tempfile.TemporaryDirectory(prefix="agentnave-claude-") as tmp:
        directory = Path(tmp)
        settings = directory / "settings.json"
        settings.write_text(json.dumps({"hooks": mode_hook(directory)}), encoding="utf-8")
        process = subprocess.Popen(
            [executable, *args, "--settings", str(settings)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )
        try:
            assert process.stdin is not None and process.stdout is not None
            with contextlib.suppress(BrokenPipeError):
                process.stdin.write(prompt)
                process.stdin.close()
            for line in process.stdout:
                try:
                    event = object_dict(json.loads(line))
                except ValueError:
                    event = {}
                if event.get("type") == "result":
                    result = event
                else:
                    sys.stdout.buffer.write(line)
                    sys.stdout.buffer.flush()
            returncode = process.wait()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        mode, held = "", False
        for path in directory.glob("event-*.json"):
            event = object_dict(json.loads(path.read_text(encoding="utf-8")))
            mode = str(event.get("permission_mode") or "")
            held = bool(event.get("agentnave_mode_blocked"))
    return returncode, result, mode, held


def print_main(executable: str, fallback: str, args: list[str], prompt: bytes) -> int:
    """`claude --print` with the permission tier's fallback mode (ADR-0059)."""
    resume = next((a.split("=", 1)[1] for a in args if a.startswith("--resume=")), None)
    session_id = resume or str(uuid.uuid4())
    base = [a for a in args if not a.startswith("--resume=")]
    requested = mode_of(base)
    for index, mode in enumerate((requested, fallback)):
        session = ["--resume", session_id] if resume or index else ["--session-id", session_id]
        attempt = with_mode(base, mode) if index else base
        returncode, result, actual, held = print_attempt(executable, [*attempt, *session], prompt)
        if not held:
            break
    else:
        if resume is None:
            drop_empty_session(Transcript(session_id))
        write_line(
            {
                "type": "result",
                "subtype": "error_during_execution",
                "is_error": True,
                "result": "",
                "session_id": resume,
                "errors": [f"permission required: {modes_unavailable(requested, fallback)}"],
            }
        )
        return 1
    if result is not None:
        if warning := mode_warning(requested, fallback, actual):
            warnings = result.get("warnings")
            result["warnings"] = [*(warnings if isinstance(warnings, list) else []), warning]
        write_line(result)
    return returncode

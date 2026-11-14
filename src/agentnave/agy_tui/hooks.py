"""The env-gated entry in Antigravity's global hooks.json, and the hook files it leaves.

agy loads hooks only from a workspace's `.agents/hooks.json` or the global file; `--settings`
rejects them, and an added workspace can take the project's place as the first workspace. The
entry does nothing unless AgentNave's helper set ENV for that agy process (ADR-0072).
"""

from __future__ import annotations

import contextlib
import json
import os
import shlex
import stat
from pathlib import Path
from typing import cast

NAME = "agentnave-interactive"
ENV = "AGENTNAVE_AGY_HOOKS"

# `exec` keeps agy as the hook shell's parent, so $PPID names the agy that ran it.
_RECORD = (
    'if [ -n "$AGENTNAVE_AGY_HOOKS" ]; then f="$AGENTNAVE_AGY_HOOKS/$PPID.$$.$1"; '
    'cat > "$f.tmp" && mv "$f.tmp" "$f.json"; else cat > /dev/null; fi; echo "{}"'
)
_QUESTION_DENIED = (
    "No one can answer questions during this run. Continue with your best judgment and "
    "list open questions in your final answer."
)
_ASK = (
    'cat > /dev/null; if [ -n "$AGENTNAVE_AGY_HOOKS" ]; then echo '
    + shlex.quote(json.dumps({"decision": "deny", "reason": _QUESTION_DENIED}))
    + "; else echo "
    + shlex.quote(json.dumps({"decision": "allow"}))
    + "; fi"
)


def _command(script: str, *args: str) -> dict[str, object]:
    line = shlex.join(("exec", "/bin/sh", "-c", script, "agentnave", *args))
    return {"type": "command", "command": line, "timeout": 10}


def entry() -> dict[str, object]:
    return {
        "SessionStart": [_command(_RECORD, "SessionStart")],
        "PreInvocation": [_command(_RECORD, "PreInvocation")],
        "Stop": [_command(_RECORD, "Stop")],
        # Print mode cannot ask either; the dispatch prompt already asks for questions in the
        # final answer.
        "PreToolUse": [{"matcher": "ask_question", "hooks": [_command(_ASK)]}],
    }


def hooks_path() -> Path:
    return Path.home() / ".gemini" / "config" / "hooks.json"


def _read(path: Path) -> bytes | None:
    try:
        return path.read_bytes()
    except FileNotFoundError:
        return None


def ensure_entry() -> str | None:
    """Adds or restores the entry, keeping every other hook; a warning when it wrote one.

    Writes through a symlink to its target, and starts over when the file changed while it
    was being rewritten, so a concurrent save by the user or Antigravity is not lost.
    """
    path = Path(os.path.realpath(hooks_path()))
    for _ in range(3):
        original = _read(path)
        try:
            document: object = json.loads(original) if original is not None else {}
        except ValueError as exc:
            raise ValueError(f"{path} is not valid JSON ({exc}); fix it in Antigravity") from exc
        if not isinstance(document, dict):
            raise ValueError(f"{path} must hold a JSON object; fix it in Antigravity")
        hooks = cast(dict[str, object], document)
        current = hooks.get(NAME)
        if isinstance(current, dict) and cast(dict[str, object], current).get("enabled") is False:
            raise ValueError(
                f"the {NAME} hook is disabled in {path}; enable it, or turn off interactive "
                "mode for Antigravity in the AgentNave App"
            )
        if current == entry():
            return None
        hooks[NAME] = entry()
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(f".{path.name}.agentnave-{os.getpid()}")
        text = json.dumps(hooks, indent=2, ensure_ascii=False) + "\n"
        temporary.write_text(text, encoding="utf-8")
        with contextlib.suppress(FileNotFoundError):
            os.chmod(temporary, stat.S_IMODE(path.stat().st_mode))
        if _read(path) != original:
            temporary.unlink()
            continue
        os.replace(temporary, path)
        return f"AgentNave added the {NAME} hook to {path}; it acts only in runs AgentNave starts"
    raise ValueError(f"{path} kept changing while AgentNave added its hook; try again")


def read_events(
    directory: Path, agy_pid: int, seen: set[str]
) -> list[tuple[str, dict[str, object]]]:
    """New hook payloads this agy wrote, oldest first. A nested agy has another pid."""
    events: list[tuple[int, str, str, dict[str, object]]] = []
    for path in directory.glob(f"{agy_pid}.*.json"):
        if path.name in seen:
            continue
        try:
            value: object = json.loads(path.read_text(encoding="utf-8"))
            written = path.stat().st_mtime_ns
        except (OSError, ValueError):
            continue
        seen.add(path.name)
        if isinstance(value, dict):
            name = path.name.split(".")[2]
            events.append((written, path.name, name, cast(dict[str, object], value)))
    return [(name, value) for _, _, name, value in sorted(events, key=lambda e: e[:2])]

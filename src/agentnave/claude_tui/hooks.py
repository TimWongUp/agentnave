"""The hook channel: settings that register the hooks, the hook process, and boss answers."""

from __future__ import annotations

import json
import os
import select
import sys
import time
import uuid
from pathlib import Path

from agentnave.claude_tui.turn import Turn, emit, object_dict
from agentnave.native_prompts import mask_secrets

# The helper's interpreter arguments: isolated mode keeps the working directory off sys.path.
HELPER = ("-I", "-m", "agentnave.claude_tui")


def mode_of(args: list[str]) -> str:
    return args[args.index("--permission-mode") + 1] if "--permission-mode" in args[:-1] else ""


def with_mode(args: list[str], mode: str) -> list[str]:
    index = args.index("--permission-mode") + 1
    return [*args[:index], mode, *args[index + 1 :]]


def mode_hook(directory: Path) -> dict[str, object]:
    """Holds back a prompt Claude Code would run in manual mode (ADR-0059): an unavailable
    auto or bypassPermissions mode starts as `default` without an error."""
    command = {
        "type": "command",
        "command": sys.executable,
        "args": [*HELPER, "hook", str(directory)],
    }
    return {"UserPromptSubmit": [{"hooks": [command]}]}


MODE_HELD = "AgentNave restarts Claude Code in its fallback permission mode for this prompt"


def mode_warning(requested: str, fallback: str, actual: str) -> str | None:
    if actual == requested or not actual:
        return None
    if actual == fallback:
        access = " (full access)" if fallback == "bypassPermissions" else ""
        return (
            f"Claude Code {requested} mode is unavailable here; per the App permission "
            f"tier's fallback, AgentNave ran it in {fallback} mode{access} instead"
        )
    return f"Claude Code ran in {actual} mode instead of the requested {requested} mode"


def modes_unavailable(requested: str, fallback: str) -> str:
    return (
        f"Claude Code started in manual mode because neither {requested} nor {fallback} mode "
        "is available here; enable one of them in Claude Code's settings"
    )


def tier_consent(extra: list[str]) -> dict[str, object]:
    """The App permission tier is the user's consent; its confirmation dialog would block."""
    mode = mode_of(extra)
    if mode == "bypassPermissions":
        return {"skipDangerousModePermissionPrompt": True}
    if mode == "auto":
        return {"skipAutoPermissionPrompt": True}
    return {}


# What -p already does, where the interactive screen would ask instead: it starts the project's
# .mcp.json servers in a trusted folder, and cancels MCP elicitation forms (Elicitation hook).
PRINT_PARITY: dict[str, object] = {"enableAllProjectMcpServers": True}


# Tools that need a user's answer: Claude Code takes a PreToolUse "allow" for them only with
# updatedInput, and shows its own menu otherwise (hooks reference, "Tools that require user
# interaction").
INTERACTION_TOOLS = ("AskUserQuestion", "ExitPlanMode")


def write_settings(
    directory: Path, extra: list[str] | None = None, check_mode: bool = False
) -> Path:
    hooks: dict[str, object] = mode_hook(directory) if check_mode else {}
    for name in (
        "PermissionRequest",
        "PreToolUse",
        "PermissionDenied",
        "StopFailure",
        "Notification",
        "Elicitation",
    ):
        command: dict[str, object] = {
            "type": "command",
            "command": sys.executable,
            "args": [*HELPER, "hook", str(directory)],
        }
        if name in ("PermissionRequest", "PreToolUse"):
            command.update(timeout=604800, onFailure="block")
        rule: dict[str, object] = {"hooks": [command]}
        if name in ("PermissionRequest", "PermissionDenied", "PreToolUse"):
            rule["matcher"] = "|".join(INTERACTION_TOOLS) if name == "PreToolUse" else "*"
        hooks[name] = [rule]
    path = directory / "settings.json"
    settings = {"hooks": hooks, **PRINT_PARITY, **tier_consent(extra or [])}
    path.write_text(json.dumps(settings), encoding="utf-8")
    return path


def atomic_json(path: Path, value: dict[str, object]) -> None:
    staging = path.with_suffix(".tmp")
    staging.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")
    staging.replace(path)


def hook_reply(event: dict[str, object], answer: dict[str, object]) -> dict[str, object]:
    """The only encoding of boss decisions into Claude hook output; never persists permissions."""
    name = event.get("hook_event_name")
    allowed = answer.get("decision") == "allow"
    if name == "PermissionRequest":
        decision: dict[str, object] = {"behavior": "allow" if allowed else "deny"}
        if not allowed:
            decision["message"] = str(answer.get("message") or "Denied by the boss")
        output: dict[str, object] = {"hookEventName": name, "decision": decision}
    else:
        output = {"hookEventName": name, "permissionDecision": "allow" if allowed else "deny"}
        if allowed:
            # Echo the input: approving a plan passes it through, a question adds its answers.
            updated = dict(object_dict(event.get("tool_input")))
            if event.get("tool_name") == "AskUserQuestion":
                updated["answers"] = object_dict(answer.get("answers"))
            output["updatedInput"] = updated
        else:
            output["permissionDecisionReason"] = str(answer.get("message") or "Denied by the boss")
    return {"hookSpecificOutput": output}


def hook_main(directory: Path) -> int:
    # Bytes: Windows' locale stdin encoding would garble the UTF-8 hook input.
    event = object_dict(json.loads(sys.stdin.buffer.read()))
    name = event.get("hook_event_name")
    if name == "Elicitation":
        emit({"hookSpecificOutput": {"hookEventName": name, "action": "cancel"}})
        return 0
    if name == "UserPromptSubmit":
        held = event.get("permission_mode") == "default"
        event = {"hook_event_name": name, "permission_mode": event.get("permission_mode")}
        atomic_json(
            directory / f"event-{uuid.uuid4().hex}.json", {**event, "agentnave_mode_blocked": held}
        )
        if held:
            emit({"decision": "block", "reason": MODE_HELD})
        return 0
    gated = name == "PermissionRequest" or (
        name == "PreToolUse" and event.get("tool_name") in INTERACTION_TOOLS
    )
    prompt_id = uuid.uuid4().hex
    if gated:
        path = directory / f"prompt-{prompt_id}.json"
        atomic_json(path, {**event, "hook_pid": os.getpid()})
        answer_path = directory / f"answer-{prompt_id}.json"
        deadline = time.monotonic() + 604790
        while directory.is_dir() and time.monotonic() < deadline:
            if answer_path.exists():
                answer = object_dict(json.loads(answer_path.read_text(encoding="utf-8")))
                emit(hook_reply(event, answer))
                path.unlink(missing_ok=True)
                answer_path.unlink(missing_ok=True)
                return 0
            time.sleep(0.05)
        return 1  # Claude's onFailure=block closes a timed-out or disconnected hook.
    atomic_json(directory / f"event-{prompt_id}.json", event)
    return 0


def prompt_event(prompt_id: str, event: dict[str, object]) -> dict[str, object]:
    question = (
        event.get("hook_event_name") == "PreToolUse" and event.get("tool_name") == "AskUserQuestion"
    )
    result: dict[str, object] = {
        "type": "agentnave_prompt",
        "prompt_id": prompt_id,
        "kind": "question" if question else "permission",
        "tool_name": event.get("tool_name"),
        # Masked whole before any cut: a cut secret is no longer recognizable.
        "tool_input": mask_secrets(json.dumps(event.get("tool_input"), ensure_ascii=False))[:8000],
    }
    if question:
        questions = object_dict(event.get("tool_input")).get("questions", [])
        # Preserve question text for the answer mapping; reject unbounded payloads.
        if not isinstance(questions, list) or len(json.dumps(questions)) > 16000:
            raise ValueError("AskUserQuestion questions exceeded the native prompt limit")
        result["questions"] = questions
    return result


class Answers:
    """Read complete JSON lines without blocking PTY/transcript polling."""

    def __init__(self) -> None:
        self.pending = bytearray()
        self.open = True

    def poll(self, directory: Path, turn: Turn) -> None:
        if not self.open or not select.select([0], [], [], 0)[0]:
            return
        chunk = os.read(0, 65536)
        if not chunk:
            self.open = False
            return
        self.pending.extend(chunk)
        if len(self.pending) > 1024 * 1024:
            raise ValueError("native prompt answer exceeded the channel limit")
        while b"\n" in self.pending:
            line, _, remainder = self.pending.partition(b"\n")
            self.pending = bytearray(remainder)
            answer = object_dict(json.loads(line))
            prompt_id = answer.get("prompt_id")
            if (
                answer.get("type") == "agentnave_answer"
                and isinstance(prompt_id, str)
                and prompt_id in turn.pending_prompts
                and answer.get("decision") in ("allow", "deny")
            ):
                atomic_json(directory / f"answer-{prompt_id}.json", answer)


def read_hooks(directory: Path, turn: Turn) -> None:
    for path in sorted(directory.glob("prompt-*.json")):
        prompt_id = path.stem.removeprefix("prompt-")
        if prompt_id not in turn.pending_prompts:
            try:
                event = object_dict(json.loads(path.read_text(encoding="utf-8")))
            except FileNotFoundError:
                continue
            emit(prompt_event(prompt_id, event))
            turn.pending_prompts.add(prompt_id)
    for path in sorted(directory.glob("event-*.json")):
        turn.hook(object_dict(json.loads(path.read_text(encoding="utf-8"))))
        path.unlink()
    for prompt_id in list(turn.pending_prompts):
        path = directory / f"prompt-{prompt_id}.json"
        try:
            event = object_dict(json.loads(path.read_text(encoding="utf-8")))
        except FileNotFoundError:
            event = None  # Answered: the hook removed its prompt before replying.
        if event is not None:
            pid = event.get("hook_pid")
            try:
                if isinstance(pid, int):
                    os.kill(pid, 0)
                continue
            except ProcessLookupError:
                # Gone unanswered (timeout or killed): Claude's onFailure=block refused the call.
                turn.denials.append(
                    {"tool_name": event.get("tool_name"), "tool_input": event.get("tool_input")}
                )
        turn.pending_prompts.remove(prompt_id)
        emit({"type": "agentnave_prompt_closed", "prompt_id": prompt_id})
        path.unlink(missing_ok=True)

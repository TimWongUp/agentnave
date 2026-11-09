"""One interactive Antigravity CLI turn, normalized to agy's print stream-json. Stdlib only.

Launched with ``python -I -m agentnave.agy_tui`` inside the invocation's supervisor group, only
with permissions skipped (ADR-0072). No process here creates a session or process group.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

from agentnave.agy_tui.hooks import ENV, ensure_entry, read_events
from agentnave.agy_tui.screen import AgyScreen, input_box, state
from agentnave.claude_tui.screen import ANSI, clean_paste
from agentnave.claude_tui.terminal import Terminal

STARTUP_TIMEOUT = 60.0
DELIVERY_TIMEOUT = 30.0
# A menu drawn this long is a real dialog, not a frame in the middle of a redraw.
DIALOG_SETTLE = 2.0
# How long an idle footer may stand without agy's Stop hook: with this turn's reply written,
# and without one. An unrecognized screen gets the longer wait too.
STOP_GRACE = 15.0
STALL_LIMIT = 60.0
# A call agy moves to the background names its task here, and ends with that task's message.
BACKGROUND = re.compile(r"running as a background task with task id: (\S+)")
TASK_ENDED = re.compile(r'Task id "([^"]+)" (?:finished|was canceled|failed)\b')


def emit(event: dict[str, object]) -> None:
    print(json.dumps(event, ensure_ascii=False), flush=True)


def object_dict(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


def transcript_path(conversation_id: str) -> Path:
    # transcript.jsonl truncates long fields, the final answer among them.
    return (
        Path.home()
        / ".gemini/antigravity-cli/brain"
        / conversation_id
        / ".system_generated/logs/transcript_full.jsonl"
    )


def flat(text: str) -> str:
    return " ".join(text.split())


def excerpt(screen: str) -> str:
    return " ".join(ANSI.sub("", screen).split())[-600:]


@dataclass
class Turn:
    conversation_id: str | None
    # The start of the task as pasted: only a user row carrying it is this delivery.
    expected: str = ""
    transcript: Path | None = None
    offset: int = 0
    user_seen: bool = False
    response: str = ""
    answered: bool = False
    model: str | None = None
    stop_seen: bool = False
    stopped: bool = False
    error: str | None = None
    warnings: list[str] = field(default_factory=list[str])
    open_tools: list[int] = field(default_factory=list[int])
    tool_names: dict[int, str] = field(default_factory=dict[int, str])
    background: dict[str, int] = field(default_factory=dict[str, int])
    shown: str = ""

    def block(self, reason: str, screen: str = "") -> None:
        self.error = f"permission required: Antigravity interactive runtime blocked: {reason}"
        if screen:
            self.error += f". Screen: {excerpt(screen)}"

    def start(self, conversation_id: str) -> None:
        self.conversation_id = conversation_id
        self.transcript = transcript_path(conversation_id)
        emit({"event": "init", "conversation_id": conversation_id})

    def step(self, index: int, step_type: str, state: str, **fields: object) -> None:
        update: dict[str, object] = {"conversation_id": self.conversation_id, "step_index": index}
        update.update(step_type=step_type, state=state, **fields)
        emit({"event": "step_update", "step_update": update})

    def close_tool(self) -> None:
        if self.open_tools:
            self.step(self.open_tools.pop(0), "tool", "DONE")

    def row(self, row: dict[str, object]) -> None:
        index = row.get("step_index")
        index = index if isinstance(index, int) else 0
        kind, content = row.get("type"), row.get("content")
        if (
            kind == "USER_INPUT"
            and row.get("source") == "USER_EXPLICIT"
            and isinstance(content, str)
            and self.expected in flat(content)
        ):
            self.user_seen, self.response, self.answered = True, "", False
            self.step(index, "user_input", "DONE")
        elif kind == "PLANNER_RESPONSE" and self.user_seen:
            while self.open_tools:
                self.close_tool()
            if isinstance(content, str) and content:
                self.response = content
                self.step(index, "agent_response", "DONE", text_delta=content)
                self.shown = ""
            calls = row.get("tool_calls")
            for number, raw in enumerate(
                cast(list[object], calls) if isinstance(calls, list) else []
            ):
                # Tool ids stay distinct when one response makes several calls.
                tool = index * 100 + number
                name = object_dict(raw).get("name")
                if isinstance(name, str):
                    self.tool_names[tool] = name
                self.open_tools.append(tool)
                self.step(tool, "tool", "ACTIVE", tool_name=name)
            self.answered = not calls
        elif (
            kind == "GENERIC"
            and row.get("status") == "RUNNING"
            and self.user_seen
            and self.open_tools
            and isinstance(content, str)
            and (moved := BACKGROUND.search(content))
        ):
            self.background[moved.group(1)] = self.open_tools.pop(0)
            self.shown = ""
        elif row.get("source") == "MODEL" and self.user_seen:
            self.close_tool()  # A tool's output step follows its call in order.
        elif (
            kind == "SYSTEM_MESSAGE"
            and isinstance(content, str)
            and (ended := TASK_ENDED.search(content))
            and ended.group(1) in self.background
        ):
            self.step(self.background.pop(ended.group(1)), "tool", "DONE")
            self.shown = ""

    def watch(self, shown: str) -> None:
        """While a background call runs, the footer tells who works: idle is agy waiting on
        the call, busy is the model."""
        if self.stopped:
            return  # Stop owns turn completion; do not emit further progress.
        if self.open_tools:
            # Transcript calls take precedence; reconsider even an unchanged footer after
            # their output arrives and hands control back to the model/background calls.
            self.shown = ""
            return
        if shown == self.shown:
            return
        self.shown = shown
        if not self.background:
            return
        if shown == "idle":
            tool = next(reversed(self.background.values()))
            self.step(tool, "tool", "ACTIVE", tool_name=self.tool_names.get(tool))
        elif shown == "busy":
            self.step(0, "thinking", "ACTIVE")

    def read(self) -> None:
        if self.transcript is None:
            return
        try:
            with self.transcript.open("rb") as source:
                source.seek(self.offset)
                while (line := source.readline()).endswith(b"\n"):
                    self.offset = source.tell()
                    try:
                        value: object = json.loads(line)
                    except ValueError:
                        continue
                    self.row(object_dict(value))
        except FileNotFoundError:
            return

    def hook(self, name: str, event: dict[str, object]) -> None:
        conversation_id = event.get("conversationId")
        if not isinstance(conversation_id, str) or not conversation_id:
            return
        if name == "SessionStart" and self.conversation_id is None:
            self.start(conversation_id)
        # Subagent conversations run in the same agy and stop on their own.
        if name != "Stop" or conversation_id != self.conversation_id:
            return
        self.stop_seen = True
        if isinstance(model := event.get("modelName"), str) and model:
            self.model = model
        if error := event.get("error"):
            self.error = f"{event.get('terminationReason') or 'error'}: {error}"
        # Like print mode, the turn waits for its background commands.
        self.stopped = event.get("fullyIdle") is True or self.error is not None

    def result(self) -> dict[str, object]:
        if self.error is None and not self.response:
            self.error = "Antigravity turn ended without a reply"
        result: dict[str, object] = {
            "conversation_id": self.conversation_id,
            "status": "ERROR" if self.error else "SUCCESS",
            "response": self.response,
            "warnings": self.warnings,
        }
        if self.error:
            result["error"] = self.error
        if self.model:
            result["model"] = self.model
        return {"event": "result", "result": result}


@dataclass
class Run:
    terminal: Terminal
    turn: Turn
    hooks: Path
    seen: set[str] = field(default_factory=set[str])

    def poll(self) -> str:
        screen = self.terminal.poll()
        for name, event in read_events(self.hooks, self.terminal.process.pid, self.seen):
            self.turn.hook(name, event)
        self.turn.read()
        return screen

    def exited(self) -> bool:
        return self.terminal.process.poll() is not None

    def await_ready(self) -> bool:
        deadline = time.monotonic() + STARTUP_TIMEOUT
        previous, since = "", time.monotonic()
        while time.monotonic() < deadline:
            screen = self.poll()
            now = time.monotonic()
            if screen != previous:
                previous, since = screen, now
            if self.exited():
                self.turn.block("agy exited before becoming ready", previous)
                return False
            if now - since < 1:
                continue
            if state(screen) == "menu":
                self.turn.block("unexpected startup dialog; handle it in agy", screen)
                return False
            if state(screen) == "idle":
                return True
        self.turn.block("startup timed out without becoming ready; handle it in agy", previous)
        return False

    def deliver(self, prompt: str) -> bool:
        paste = clean_paste(prompt)
        self.terminal.outgoing.extend(b"\x1b[200~" + paste.encode() + b"\x1b[201~")
        evidence = flat(paste)[:40]
        self.turn.expected = evidence
        started = time.monotonic()
        submitted: float | None = None
        retried = False
        while not self.turn.user_seen:
            screen = self.poll()
            now = time.monotonic()
            if self.exited() or self.turn.error:
                return False
            # agy folds a long paste into "[Pasted text #1 +121 lines]".
            box = input_box(screen)
            echoed = evidence in box or box.lower().startswith("> [pasted text #")
            # Enter never answers a menu that came up after the paste.
            if state(screen) == "menu":
                self.turn.block("agy opened a dialog while taking the task", screen)
                return False
            if submitted is None and not self.terminal.outgoing and echoed:
                time.sleep(0.5)  # Let agy finish taking the paste before Enter.
                if state(self.poll()) == "menu":
                    continue
                self.terminal.outgoing.extend(b"\r")
                submitted = now
            elif submitted is not None and now - submitted >= 3 and not retried:
                self.terminal.outgoing.extend(b"\r")
                retried = True
            if now - started > DELIVERY_TIMEOUT:
                self.turn.block("task delivery was not confirmed", screen)
                return False
        return True

    def await_stop(self) -> None:
        previous, since = "", time.monotonic()
        while not self.turn.stopped:
            offset = self.turn.offset
            screen = self.poll()
            self.turn.watch(state(screen))
            now = time.monotonic()
            if self.exited():
                self.poll()
                if not self.turn.stopped:
                    status = self.terminal.process.returncode
                    self.turn.error = f"agy exited unexpectedly with status {status}"
                return
            if screen != previous or self.turn.offset != offset:
                previous, since = screen, now
            if self.stalled(state(screen), now - since, screen):
                return
        # The reply row can land just after the Stop hook.
        deadline = time.monotonic() + 3
        while not self.turn.answered and time.monotonic() < deadline:
            self.poll()
        if not self.turn.answered and self.turn.error is None:
            self.turn.error = "agy stopped without a final reply after this task"

    def stalled(self, shown: str, steady: float, screen: str) -> bool:
        """Blocks a turn whose screen has stood still past its limit. A busy footer has none,
        nor an idle one after a Stop that left background commands running: agy stops again
        once they finish."""
        # With permissions skipped, a menu mid-turn is one no hook can answer.
        if shown == "menu" and steady > DIALOG_SETTLE:
            reason = "agy opened a dialog during the turn"
        elif shown == "idle" and not self.turn.stop_seen and self.turn.answered:
            if steady < STOP_GRACE:
                return False
            reason = (
                "agy's reply is written but its Stop hook never arrived; check the "
                "agentnave-interactive entry in ~/.gemini/config/hooks.json"
            )
        elif shown == "unknown" or (shown == "idle" and not self.turn.stop_seen):
            if steady < STALL_LIMIT:
                return False
            reason = "agy stopped changing without a reply or Stop hook"
        else:
            return False
        self.turn.block(reason, screen)
        return True


def settings(cwd: str) -> str:
    # Never saved by agy: trusting the folder here leaves the user's settings unchanged.
    return json.dumps({"trustedWorkspaces": sorted({cwd, os.path.realpath(cwd)})})


def close(terminal: Terminal) -> None:
    # agy finishes its own conversation writes on SIGTERM; give it time before the kill.
    if terminal.process.poll() is None:
        terminal.process.terminate()
        deadline = time.monotonic() + 5
        while terminal.process.poll() is None and time.monotonic() < deadline:
            terminal.poll()
    terminal.close()


def run_main(executable: str, conversation_id: str | None, extra: list[str], prompt: str) -> int:
    turn = Turn(conversation_id)
    try:
        if not os.path.isabs(executable):
            raise ValueError("agy executable must be an absolute path")
        if conversation_id is not None:
            if not re.fullmatch(r"[A-Za-z0-9-]{1,80}", conversation_id):
                raise ValueError("invalid conversation id")
            turn.start(conversation_id)
            transcript = cast(Path, turn.transcript)
            turn.offset = transcript.stat().st_size if transcript.exists() else 0
        if warning := ensure_entry():
            turn.warnings.append(warning)
        with tempfile.TemporaryDirectory(prefix="agentnave-agy-") as tmp:
            args = [*extra, "--settings", settings(os.getcwd())]
            if conversation_id is not None:
                args[:0] = ["--conversation", conversation_id]
            terminal = Terminal(executable, args, {ENV: tmp}, AgyScreen)
            try:
                run = Run(terminal, turn, Path(tmp))
                if run.await_ready() and run.deliver(prompt):
                    run.await_stop()
            finally:
                close(terminal)
    except (OSError, ValueError) as exc:
        turn.block(str(exc))
    emit(turn.result())
    return 1 if turn.error else 0


def read_prompt() -> str:
    value = object_dict(json.loads(sys.stdin.buffer.readline()))
    prompt = value.get("prompt")
    if not isinstance(prompt, str):
        raise ValueError("first stdin line must contain a prompt string")
    return prompt


def terminate(signum: int, _frame: object) -> None:
    # Let finally blocks stop agy and remove the hook directory on cancellation.
    raise SystemExit(128 + signum)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agy", required=True)
    parser.add_argument("--conversation")
    parser.add_argument("extra", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, terminate)
    extra = cast(list[str], args.extra)
    extra = extra[1:] if extra[:1] == ["--"] else extra
    return run_main(args.agy, args.conversation, extra, read_prompt())

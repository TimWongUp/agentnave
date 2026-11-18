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
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

from agentnave.agy_tui.screen import (
    AgyScreen,
    busy_activity,
    input_box,
    state,
    trust_menu,
)
from agentnave.claude_tui.screen import ANSI, clean_paste, workspace_matches
from agentnave.claude_tui.terminal import Terminal

STARTUP_TIMEOUT = 60.0
# Give the post-login experiment refresh time to finish before sending the task.
EXPERIMENT_WAIT = 9.0
DELIVERY_TIMEOUT = 30.0
# A menu drawn this long is a real dialog, not a frame in the middle of a redraw.
DIALOG_SETTLE = 2.0
STALL_LIMIT = 60.0
TRANSCRIPT_GRACE = 5.0
# A call agy moves to the background names its task here, and ends with that task's message.
BACKGROUND = re.compile(r"running as a background task with task id: (\S+)")
TASK_ENDED = re.compile(r'Task id "([^"]+)" (?:finished|was canceled|failed)\b')
_EXPERIMENTS_READY = b"Experiments refreshed after login"
_CREATED_CONVERSATION = re.compile(rb"Created conversation ([0-9a-fA-F-]{36})\b")
_REQUEST_OPEN, _REQUEST_CLOSE = "<USER_REQUEST>", "</USER_REQUEST>"


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


def compact(text: str) -> str:
    return "".join(text.split())


def delivered(row: dict[str, object], expected: str) -> bool:
    """Whether a user row's request is exactly this task: one task may quote another whole."""
    content = row.get("content")
    if not isinstance(content, str) or not expected:
        return False
    # agy wraps the typed request and follows it with metadata of its own.
    start, end = content.find(_REQUEST_OPEN), content.rfind(_REQUEST_CLOSE)
    if start >= 0 and end > start:
        content = content[start + len(_REQUEST_OPEN) : end]
    return flat(content) == expected


def first_request_matches(conversation_id: str, expected: str) -> bool:
    """Confirm the session before publishing its ID."""
    try:
        with transcript_path(conversation_id).open("rb") as source:
            while (line := source.readline()).endswith(b"\n"):
                try:
                    row = object_dict(json.loads(line))
                except ValueError:
                    continue
                if row.get("type") == "USER_INPUT" and row.get("source") == "USER_EXPLICIT":
                    return delivered(row, expected)
    except OSError:
        pass
    return False


def excerpt(screen: str) -> str:
    return " ".join(ANSI.sub("", screen).split())[-600:]


@dataclass
class Turn:
    conversation_id: str | None
    # The whole task as pasted, set as Enter submits it: only a user row requesting exactly it
    # is this delivery. Concurrent tasks share the service's opening instructions, so no
    # excerpt tells them apart.
    expected: str = ""
    transcript: Path | None = None
    offset: int = 0
    user_seen: bool = False
    response: str = ""
    answered: bool = False
    model: str | None = None
    active: bool = False
    done_at: float | None = None
    stopped: bool = False
    error: str | None = None
    warnings: list[str] = field(default_factory=list[str])
    open_tools: list[int] = field(default_factory=list[int])
    tool_names: dict[int, str] = field(default_factory=dict[int, str])
    background: dict[str, int] = field(default_factory=dict[str, int])
    shown: str = ""
    screen_activity: str | None = None

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
            if self.open_tools:
                tool = self.open_tools[0]
                self.step(tool, "tool", "ACTIVE", tool_name=self.tool_names.get(tool))

    def row(self, row: dict[str, object]) -> None:
        index = row.get("step_index")
        index = index if isinstance(index, int) else 0
        kind, content = row.get("type"), row.get("content")
        if (
            kind == "USER_INPUT"
            and row.get("source") == "USER_EXPLICIT"
            and delivered(row, self.expected)
        ):
            self.user_seen, self.response, self.answered = True, "", False
            self.step(index, "user_input", "DONE")
        elif kind == "PLANNER_RESPONSE" and self.user_seen:
            while self.open_tools:
                self.close_tool()
            if isinstance(content, str) and content:
                self.response = content
                self.shown = ""
            elif not row.get("tool_calls"):
                self.response = ""
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
            if self.answered:
                model = row.get("model")
                self.model = model if isinstance(model, str) and model else None
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
            self.answered = False
            self.shown = ""

    def watch(self, shown: str, activity: str | None = None) -> None:
        """While a background call runs, the footer tells who works: idle is agy waiting on
        the call, busy is the model."""
        if self.stopped:
            return  # The turn is complete; do not emit further progress.
        if self.open_tools:
            # Transcript calls take precedence; reconsider even an unchanged footer after their
            # output arrives. Remember the status line so its lag after a call ends is no news.
            self.shown, self.screen_activity = "", activity
            return
        if activity is None:
            if shown != "busy":
                self.screen_activity = None
        elif activity != self.screen_activity:
            # agy writes nothing while the model thinks or a call runs before its response
            # lands; its status line does (ADR-0079).
            self.screen_activity = activity
            if self.user_seen:
                self.shown = shown
                if activity == "thinking":
                    self.step(0, "thinking", "ACTIVE")
                else:
                    self.step(0, "tool", "ACTIVE", tool_name=activity)
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

    def native(self, event: str, screen: str = "") -> None:
        if not self.expected:
            return
        if event == "working":
            self.active, self.done_at = True, None
        elif event == "done" and self.active:
            self.done_at = time.monotonic()
        elif event in ("blocked", "waiting", "waiting_for_input") and self.active:
            self.block("agy requires input during the turn", screen)
        elif event in ("failed", "error"):
            self.error = "agy reported a failed turn"
            if screen:
                self.error += f". Screen: {excerpt(screen)}"

    def complete(self, now: float | None = None) -> bool:
        if (
            self.error is None
            and self.done_at is not None
            and (
                not self.background
                or (now if now is not None else time.monotonic()) - self.done_at >= TRANSCRIPT_GRACE
            )
            and self.user_seen
            and self.answered
            and self.response.strip()
            and not self.open_tools
        ):
            self.stopped = True
            self.step(0, "agent_response", "DONE", text_delta=self.response)
            return True
        return False

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
    run_log: Path

    def log_bytes(self) -> bytes:
        try:
            if self.run_log.stat().st_size < 16 << 20:
                return self.run_log.read_bytes()
        except OSError:
            pass
        return b""

    def poll(self) -> str:
        screen = self.terminal.poll()
        terminal_screen = cast(AgyScreen, self.terminal.screen)
        events = terminal_screen.events
        terminal_screen.events = []
        if self.turn.conversation_id is None and self.turn.expected:
            candidates = {raw.decode() for raw in _CREATED_CONVERSATION.findall(self.log_bytes())}
            matches = [cid for cid in candidates if first_request_matches(cid, self.turn.expected)]
            if len(matches) == 1:
                self.turn.start(matches[0])
        self.turn.read()
        for event in events:
            self.turn.native(event, screen)
        return screen

    def submitting(self, expected: str) -> None:
        cast(AgyScreen, self.terminal.screen).events.clear()
        self.turn.expected = expected

    def exited(self) -> bool:
        return self.terminal.process.poll() is not None

    def await_experiments(self) -> bool:
        """Waits for this agy process's post-login experiment refresh, proceeding on timeout."""
        deadline = time.monotonic() + EXPERIMENT_WAIT
        menu_since: float | None = None
        while time.monotonic() < deadline:
            if _EXPERIMENTS_READY in self.log_bytes():
                return True
            screen = self.poll()
            now = time.monotonic()
            if self.exited():
                self.turn.block("agy exited before becoming ready", screen)
                return False
            if state(screen) != "menu":
                menu_since = None
            elif menu_since is None:
                menu_since = now
            elif now - menu_since >= DIALOG_SETTLE:
                self.turn.block("unexpected startup dialog; handle it in agy", screen)
                return False
        return True

    def await_ready(self) -> bool:
        deadline = time.monotonic() + STARTUP_TIMEOUT
        previous, since = "", time.monotonic()
        tries = 0
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
                menu = trust_menu(screen)
                if menu is not None:
                    if not workspace_matches(menu, os.getcwd()):
                        self.turn.block("workspace trust screen does not match cwd", screen)
                        return False
                    if tries >= 3:
                        self.turn.block("workspace trust screen stayed after confirmation", screen)
                        return False
                    yes = "> Yes, I trust this folder" in menu.splitlines()
                    if not yes and "> No, exit" not in menu.splitlines():
                        self.turn.block("unrecognized workspace trust choice", screen)
                        return False
                    if yes:
                        warning = "Antigravity CLI now trusts the dispatched workspace"
                        if warning not in self.turn.warnings:
                            self.turn.warnings.append(warning)
                    self.terminal.outgoing.extend(b"\r" if yes else b"\x1b[A")
                    tries += 1
                    since = now
                    continue
                self.turn.block("unexpected startup dialog; handle it in agy", screen)
                return False
            if state(screen) == "idle":
                return True
        self.turn.block("startup timed out without becoming ready; handle it in agy", previous)
        return False

    def deliver(self, prompt: str) -> bool:
        paste = clean_paste(prompt)
        self.terminal.outgoing.extend(b"\x1b[200~" + paste.encode() + b"\x1b[201~")
        # Unfolded, a paste taller than the box scrolls to its end, and wrapping adds spaces.
        tail = compact(paste)[-40:]
        started = time.monotonic()
        submitted: float | None = None
        retried = False
        while not self.turn.user_seen:
            screen = self.poll()
            now = time.monotonic()
            if self.exited() or self.turn.error:
                return False
            # agy folds a paste of over 15 lines, or with a line over 1000 characters, into
            # "[Pasted text #1 +121 lines]".
            box = input_box(screen)
            echoed = (bool(tail) and compact(box).endswith(tail)) or box.lower().startswith(
                "> [pasted text #"
            )
            # Enter never answers a menu that came up after the paste.
            if state(screen) == "menu":
                self.turn.block("agy opened a dialog while taking the task", screen)
                return False
            if submitted is None and not self.terminal.outgoing and echoed:
                time.sleep(0.5)  # Let agy finish taking the paste before Enter.
                if state(self.poll()) == "menu":
                    continue
                self.submitting(flat(paste))
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
        while not self.turn.stopped and self.turn.error is None:
            offset = self.turn.offset
            screen = self.poll()
            self.turn.watch(state(screen), busy_activity(screen))
            now = time.monotonic()
            if self.exited():
                self.poll()
                self.turn.error = (
                    f"agy exited unexpectedly with status {self.terminal.process.returncode}"
                )
                return
            done_at = self.turn.done_at
            if done_at is not None and now - done_at >= 1 and self.turn.complete(now):
                return
            if done_at is not None and now - done_at >= TRANSCRIPT_GRACE:
                self.turn.error = "agy reported completion without a final reply after this task"
                return
            if screen != previous or self.turn.offset != offset:
                previous, since = screen, now
            if self.stalled(state(screen), now - since, screen):
                return

    def stalled(self, shown: str, steady: float, screen: str) -> bool:
        if shown == "menu" and steady > DIALOG_SETTLE:
            self.turn.block("agy opened a dialog during the turn", screen)
            return True
        if shown in ("unknown", "idle") and steady >= STALL_LIMIT:
            self.turn.block("agy stopped changing before native completion was confirmed", screen)
            return True
        return False


def settings(cwd: str) -> str:
    # Never saved by agy: trusting the folder here leaves the user's settings unchanged.
    workspaces = {cwd, os.path.realpath(cwd)}
    # agy takes the folder from $PWD, which can spell the same one with another case.
    if pwd := os.environ.get("PWD"):
        try:
            if os.path.samefile(pwd, cwd):
                workspaces.add(pwd)
        except OSError:
            pass  # a path that no longer exists cannot be trusted either way
    return json.dumps({"trustedWorkspaces": sorted(workspaces)})


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
        version = subprocess.run(
            [executable, "--version"], capture_output=True, text=True, timeout=10, check=False
        )
        match = re.search(r"\b(\d+)\.(\d+)\.(\d+)\b", version.stdout)
        if version.returncode or match is None or tuple(map(int, match.groups())) < (1, 5, 10):
            raise ValueError(
                "Antigravity interactive mode requires agy 1.5.10 or later; update agy and retry"
            )
        with tempfile.TemporaryDirectory(prefix="agentnave-agy-") as tmp:
            logfile = Path(tmp) / "agy.log"
            args = [*extra, "--settings", settings(os.getcwd()), "--log-file", str(logfile)]
            if conversation_id is not None:
                args[:0] = ["--conversation", conversation_id]
            terminal = Terminal(
                executable,
                args,
                {"AGY_CLI_OSC7501": "on", "AGY_CLI_OSC7501_MESSAGES": "brief"},
                AgyScreen,
            )
            try:
                run = Run(terminal, turn, logfile)
                if run.await_ready() and run.await_experiments() and run.deliver(prompt):
                    run.await_stop()
            finally:
                close(terminal)
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
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
    # Let finally blocks stop agy and remove the private log directory on cancellation.
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

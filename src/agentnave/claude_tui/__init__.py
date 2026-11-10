"""One interactive Claude turn, normalized to stream-json. Stdlib only.

Launched with ``python -I -m agentnave.claude_tui run`` inside the invocation's supervisor group;
``print`` wraps ``claude --print`` with the permission tier's fallback mode instead.
No process here creates a session or process group; the supervisor owns the tree.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

from agentnave.claude_tui.hooks import (
    HELPER,
    INTERACTION_TOOLS,
    MODE_HELD,
    PRINT_PARITY,
    Answers,
    atomic_json,
    hook_main,
    hook_reply,
    mode_hook,
    mode_of,
    mode_warning,
    modes_unavailable,
    prompt_event,
    read_hooks,
    tier_consent,
    with_mode,
    write_settings,
)
from agentnave.claude_tui.print_mode import print_attempt, print_main, write_line
from agentnave.claude_tui.screen import (
    ANSI,
    CHOICES,
    RULE,
    Screen,
    bottom_choices,
    classify_screen,
    clean_paste,
    cold_resume_warning,
    dialog_open,
    dialog_region,
    in_input_box,
    selected,
    workspace_matches,
)
from agentnave.claude_tui.terminal import (
    STRIP_ENV,
    Terminal,
    child_environment,
    signal_groups,
    spawned_groups,
)
from agentnave.claude_tui.turn import (
    Transcript,
    Turn,
    blocked,
    drop_empty_session,
    emit,
    is_user_prompt,
    object_dict,
    transcript_event,
)

# One import path for the adapter and tests.
__all__ = [
    "ANSI",
    "Answers",
    "CHOICES",
    "HELPER",
    "INTERACTION_TOOLS",
    "MODE_HELD",
    "PRINT_PARITY",
    "RULE",
    "STRIP_ENV",
    "Screen",
    "Terminal",
    "Transcript",
    "Turn",
    "atomic_json",
    "blocked",
    "child_environment",
    "classify_screen",
    "clean_paste",
    "cold_resume_warning",
    "dialog_open",
    "dialog_region",
    "drop_empty_session",
    "emit",
    "hook_main",
    "hook_reply",
    "is_user_prompt",
    "mode_hook",
    "mode_of",
    "mode_warning",
    "modes_unavailable",
    "object_dict",
    "print_attempt",
    "print_main",
    "prompt_event",
    "read_hooks",
    "selected",
    "signal_groups",
    "spawned_groups",
    "tier_consent",
    "transcript_event",
    "with_mode",
    "write_line",
    "write_settings",
]

STARTUP_TIMEOUT = 60.0


# A startup screen is answered once it has stayed drawn this long: a key sent while a menu is
# first drawn can be lost to its redraw, and Enter on the trust menu's default exits.
SETTLE = 0.3


# Onboarding screens answered with their default, and the key that answers each.
ONBOARDING_KEYS = {"onboarding_enter": b"\r", "onboarding_skip": b"\x1b"}


STARTUP_TRIES = 3


def read_initial_prompt() -> str:
    # No buffered read-ahead: later answer lines stay on the descriptor for Answers.poll.
    line = bytearray()
    while chunk := os.read(0, 1):
        if chunk == b"\n":
            break
        line.extend(chunk)
    value = object_dict(json.loads(line))
    prompt = value.get("prompt")
    if not isinstance(prompt, str):
        raise ValueError("first stdin line must contain a prompt string")
    return prompt


BLOCKING_SCREENS = {
    "summary": "summary resume selection is not supported",
    "login": "handle login or the API key prompt in Claude Code",
    "mode_consent": "permission tier confirmation dialog is still shown",
    "permission": "unexpected startup permission dialog; handle it in Claude Code",
}


@dataclass
class Startup:
    """Answers Claude Code's startup screens, each once it has settled."""

    terminal: Terminal
    turn: Turn
    previous: str = ""
    stable_since: float = 0.0
    tries: dict[str, int] = field(default_factory=dict[str, int])

    def press(self, kind: str, key: bytes) -> bool:
        self.tries[kind] = self.tries.get(kind, 0) + 1
        if self.tries[kind] > STARTUP_TRIES * (2 if kind in CHOICES else 1):
            blocked(self.turn, f"{kind} screen stayed after answering it", self.previous)
            return False
        self.terminal.outgoing.extend(key)
        self.stable_since = time.monotonic()
        return True

    def choose(self, kind: str, screen: str) -> bool:
        label, move, warning = CHOICES[kind]
        if not selected(screen, label):
            return self.press(kind, move)
        warning = warning.format(cwd=os.getcwd())
        if warning not in self.turn.warnings:
            self.turn.warnings.append(warning)
        return self.press(kind, b"\r")

    def step(self, screen: str) -> bool | None:
        """True once ready, False when blocked, None to keep waiting."""
        now = time.monotonic()
        if screen != self.previous:
            self.previous, self.stable_since = screen, now
        kind = classify_screen(screen)
        settled = now - self.stable_since >= SETTLE and not self.terminal.outgoing
        if not settled:
            return None  # Half-drawn frames can show any text.
        if kind in BLOCKING_SCREENS:
            blocked(self.turn, BLOCKING_SCREENS[kind])
            return False
        if kind == "trust" and not workspace_matches(dialog_region(screen), os.getcwd()):
            blocked(self.turn, "workspace trust screen does not match cwd", screen)
            return False
        if kind in ("trust", "external_imports"):
            # Only the dialog's own lines count: the conversation and the input box can quote
            # the same words, and a cursor there is not the menu's.
            choices = "" if in_input_box(screen) else bottom_choices(dialog_region(screen))
            affirmative = CHOICES[kind][0]
            negatives = (
                ("no, exit", "no, continue without these permissions")
                if kind == "trust"
                else ("no, disable external imports",)
            )
            labels = [line.replace("❯", "").strip() for line in choices.lower().splitlines()]
            if (
                len(labels) != 2
                or affirmative not in labels
                or not set(labels) & set(negatives)
                or not any(selected(choices, label) for label in labels)
            ):
                blocked(self.turn, "unrecognized startup confirmation choice", screen)
                return False
            return None if self.choose(kind, choices) else False
        if kind in CHOICES:
            return None if self.choose(kind, screen) else False
        if kind in ONBOARDING_KEYS:
            return None if self.press(kind, ONBOARDING_KEYS[kind]) else False
        if kind == "cold_resume" and "cold_resume" not in self.tries:
            if (warning := cold_resume_warning(screen)) not in self.turn.warnings:
                self.turn.warnings.append(warning)
            return None if self.press(kind, b"\x1b") else False
        if kind == "ready" and now - self.stable_since >= 1:
            return True
        return None


def await_ready(terminal: Terminal, turn: Turn) -> bool:
    deadline = time.monotonic() + STARTUP_TIMEOUT
    startup = Startup(terminal, turn, stable_since=time.monotonic())
    while time.monotonic() < deadline:
        state = startup.step(terminal.poll())
        if state is not None:
            return state
        if terminal.process.poll() is not None:
            blocked(turn, "Claude exited before becoming ready", startup.previous)
            return False
    blocked(
        turn,
        "startup timed out without becoming ready; handle the screen in Claude Code",
        startup.previous,
    )
    return False


def await_turn(
    terminal: Terminal, turn: Turn, transcript: Transcript, directory: Path, prompt: str
) -> None:
    paste = clean_paste(prompt)
    terminal.outgoing.extend(b"\x1b[200~" + paste.encode() + b"\x1b[201~")
    started = time.monotonic()
    pasted_at: float | None = None
    submitted_at: float | None = None
    retried = False
    dialog_since: float | None = None
    answers = Answers()
    while True:
        screen = terminal.poll()
        now = time.monotonic()
        read_hooks(directory, turn)
        answers.poll(directory, turn)
        for row in transcript.read():
            turn.transcript(row)
        if turn.mode_blocked:
            return
        if turn.error or (turn.done and not turn.pending_prompts):
            if turn.done and not turn.assistant and not turn.error:
                turn.error = "Claude turn ended without an assistant message"
            return
        if terminal.process.poll() is not None:
            for row in transcript.read():
                turn.transcript(row)
            turn.error = f"Claude exited unexpectedly with status {terminal.process.returncode}"
            return
        if turn.pending_prompts:
            dialog_since = None
            continue
        # Mid-turn, any confirm menu is a dialog no hook answered.
        if dialog_open(screen):
            dialog_since = dialog_since or now
            if now - dialog_since > 1:
                reason = "permission dialog without a corresponding hook request"
                blocked(turn, reason)
                return
            # Delivery retries must never confirm a native permission menu, even
            # if the user transcript row has not been flushed yet.
            continue
        else:
            dialog_since = None
        if not terminal.outgoing and pasted_at is None:
            pasted_at = now
        # Confirm that the input received the paste, allowing Claude's long-paste badge.
        evidence = " ".join(paste.strip()[:40].split())
        echoed = (
            bool(evidence) and evidence in " ".join(screen.split())
        ) or "pasted" in screen.lower()
        if submitted_at is None and pasted_at is not None and now - pasted_at >= 0.6 and echoed:
            terminal.outgoing.extend(b"\r")
            submitted_at = now
        elif (
            submitted_at is not None
            and not turn.user_seen
            and now - submitted_at >= 3
            and not retried
        ):
            terminal.outgoing.extend(b"\r")
            retried = True
        if not turn.user_seen and now - started > 30:
            blocked(turn, "task delivery was not confirmed in the transcript")
            return


def run_attempt(
    executable: str,
    turn: Turn,
    transcript: Transcript,
    args: list[str],
    prompt: str,
    check_mode: bool,
) -> None:
    terminal: Terminal | None = None
    turn.mode_blocked = False
    with tempfile.TemporaryDirectory(prefix="agentnave-claude-") as tmp:
        directory = Path(tmp)
        args = [*args, "--settings", str(write_settings(directory, args, check_mode))]
        try:
            terminal = Terminal(executable, args)
            if await_ready(terminal, turn):
                await_turn(terminal, turn, transcript, directory, prompt)
            read_hooks(directory, turn)
        finally:
            if terminal is not None:
                terminal.close()


def run_main(
    executable: str,
    session_id: str | None,
    extra: list[str],
    prompt: str,
    fallback: str | None = None,
) -> int:
    turn = Turn(session_id or str(uuid.uuid4()))
    emit({"type": "system", "subtype": "init", "session_id": turn.session_id})
    transcript: Transcript | None = None
    try:
        if not os.path.isabs(executable):
            raise ValueError("Claude executable must be an absolute path")
        if any(c in turn.session_id for c in ("/", "\\")):
            raise ValueError("invalid session id")
        transcript = Transcript(turn.session_id)
        requested = mode_of(extra)
        modes = [requested] if fallback is None else [requested, fallback]
        for mode in modes:
            session = ["--resume" if session_id else "--session-id", turn.session_id]
            args = [*session, *(with_mode(extra, mode) if mode != requested else extra)]
            run_attempt(executable, turn, transcript, args, prompt, fallback is not None)
            if not turn.mode_blocked or mode == modes[-1]:
                break
            if session_id is None:
                # The interactive runtime cannot resume a session whose only prompt was held
                # back, so a new session restarts under a new ID and drops the empty one.
                drop_empty_session(transcript)
                turn.session_id = str(uuid.uuid4())
                transcript = Transcript(turn.session_id)
                emit({"type": "system", "subtype": "init", "session_id": turn.session_id})
        if turn.mode_blocked and fallback is not None:
            blocked(turn, modes_unavailable(requested, fallback))
        elif (
            fallback is not None
            and not turn.error
            and (warning := mode_warning(requested, fallback, turn.mode))
        ):
            turn.warnings.append(warning)
    except (OSError, ValueError) as exc:
        blocked(turn, str(exc))
    # A new session that never got its prompt leaves nothing to resume.
    if session_id is None and turn.error and transcript and drop_empty_session(transcript):
        turn.session_id = ""
    emit(turn.result())
    return 1 if turn.error else 0


def terminate(signum: int, _frame: object) -> None:
    # Let finally blocks reap Claude and remove private hook files on cancellation.
    raise SystemExit(128 + signum)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    run = sub.add_parser("run")
    run.add_argument("--claude", required=True)
    run.add_argument("--resume")
    run.add_argument("--fallback")
    run.add_argument("extra", nargs=argparse.REMAINDER)
    printing = sub.add_parser("print")
    printing.add_argument("--claude", required=True)
    printing.add_argument("--fallback", required=True)
    printing.add_argument("extra", nargs=argparse.REMAINDER)
    hook = sub.add_parser("hook")
    hook.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "hook":
        return hook_main(args.directory)
    signal.signal(signal.SIGTERM, terminate)
    extra = cast(list[str], args.extra)
    extra = extra[1:] if extra[:1] == ["--"] else extra
    if args.command == "print":
        return print_main(args.claude, args.fallback, extra, sys.stdin.buffer.read())
    return run_main(args.claude, args.resume, extra, read_initial_prompt(), args.fallback)

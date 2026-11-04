"""One interactive Claude turn, normalized to stream-json. Standalone stdlib helper.

Launched with ``python -I <this file> run`` inside the invocation's supervisor group.
No process here creates a session or process group; the supervisor owns the tree.
"""

from __future__ import annotations

import argparse
import codecs
import contextlib
import glob
import json
import os
import re
import select
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

# Imported only by the POSIX run path, so importing pure helpers works on Windows too.
STRIP_ENV = (
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_PROJECT_DIR",
)
# CSI, OSC and DCS strings, then any other escape: optional intermediates and a final byte
# (DECSC `\x1b7`, keypad `\x1b=`, charset `\x1b(B`). An unknown one must never stall the screen.
ANSI = re.compile(
    r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|P[^\x1b]*\x1b\\|(?![\[\]P])[ -/]*[0-~])"
)
DENIAL = "No one can answer during this run; report the blocked action in the final answer."
STARTUP_TIMEOUT = 60.0


def object_dict(value: object) -> dict[str, object]:
    return cast(dict[str, object], value) if isinstance(value, dict) else {}


def emit(event: dict[str, object]) -> None:
    print(json.dumps(event, ensure_ascii=False), flush=True)


def clean_paste(text: str) -> str:
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = "".join(c for c in text if c in "\n\t" or (ord(c) >= 32 and ord(c) != 127))
    return "\n" + text if text.lstrip().startswith(("/", "!", "#", "&")) else text


def classify_screen(text: str) -> str:
    """Dialogs take precedence over the prompt, including the prompt inside a menu."""
    lowered = ANSI.sub("", text).lower()
    if "trust this folder" in lowered or "do you trust" in lowered:
        return "trust"
    if "resume from summary" in lowered or "resume full session as-is" in lowered:
        return "summary"
    if "resume this conversation?" in lowered:
        return "cold_resume"
    if any(
        s in lowered
        for s in (
            "select login method",
            "/login",
            "choose the text style",
            "do you want to use this api key",
        )
    ):
        return "login"
    if any(
        s in lowered
        for s in (
            "do you want to proceed",
            "do you want to make this edit",
            "do you want to run",
            "do you want to create",
            "yes, and don't ask again",
            "enter to select",
        )
    ):
        return "permission"
    return "ready" if "❯" in lowered else "unknown"


def cold_resume_warning(text: str) -> str:
    flat = " ".join(ANSI.sub("", text).split())
    match = re.search(r"This conversation has been inactive.*?5-hour usage limit\.", flat, re.I)
    return match.group(0) if match else "Claude displayed a cold conversation resume usage warning."


def transcript_event(row: dict[str, object], session_id: str) -> dict[str, object] | None:
    if row.get("isSidechain") or row.get("isMeta"):
        return None
    if row.get("type") not in ("assistant", "user") or not isinstance(row.get("message"), dict):
        return None
    return {
        "type": row["type"],
        "message": row["message"],
        "session_id": session_id,
        "parent_tool_use_id": None,
    }


def is_user_prompt(event: dict[str, object]) -> bool:
    if event.get("type") != "user":
        return False
    content = cast(dict[str, object], event["message"]).get("content")
    return isinstance(content, str) or (
        isinstance(content, list)
        and any(
            isinstance(b, dict) and cast(dict[str, object], b).get("type") != "tool_result"
            for b in cast(list[object], content)
        )
    )


@dataclass
class Turn:
    session_id: str
    assistant: dict[str, object] = field(default_factory=dict[str, object])
    denials: list[dict[str, object]] = field(default_factory=list[dict[str, object]])
    warnings: list[str] = field(default_factory=list[str])
    error: str | None = None
    user_seen: bool = False
    done: bool = False
    gate_seen: bool = False
    # Background tool calls still running: like --print, the turn waits for their notifications.
    background: set[str] = field(default_factory=set[str])

    def transcript(self, row: dict[str, object]) -> None:
        event = transcript_event(row, self.session_id)
        if event is not None:
            emit(event)
            self.user_seen |= is_user_prompt(event)
            if event["type"] == "assistant":
                self.assistant = cast(dict[str, object], event["message"])
            self.track_background(event)
        if (
            not row.get("isSidechain")
            and row.get("type") == "system"
            and row.get("subtype") == "turn_duration"
        ):
            self.done = self.user_seen and not self.background

    def track_background(self, event: dict[str, object]) -> None:
        content = cast(dict[str, object], event["message"]).get("content")
        if event["type"] == "user":
            text = content if isinstance(content, str) else json.dumps(content)
            for tool_use in re.findall(r"<tool-use-id>([^<]+)</tool-use-id>", text):
                self.background.discard(tool_use)
            return
        for raw in cast(list[object], content) if isinstance(content, list) else []:
            block = object_dict(raw)
            if block.get("type") == "tool_use" and object_dict(block.get("input")).get(
                "run_in_background"
            ):
                self.background.add(str(block.get("id")))

    def hook(self, event: dict[str, object]) -> None:
        name = event.get("hook_event_name")
        gated = name == "PermissionRequest" or (
            name == "PreToolUse" and event.get("tool_name") == "AskUserQuestion"
        )
        if gated or name == "PermissionDenied":
            self.denials.append(
                {"tool_name": event.get("tool_name"), "tool_input": event.get("tool_input")}
            )
        self.gate_seen |= gated
        if name == "StopFailure":
            self.error = str(event.get("error") or "Claude StopFailure")
            if self.error in ("authentication_failed", "invalid_api_key"):
                self.error += ": authentication failed"

    def result(self) -> dict[str, object]:
        content = self.assistant.get("content")
        blocks = cast(list[object], content) if isinstance(content, list) else []
        text = "".join(
            str(object_dict(b).get("text", ""))
            for b in blocks
            if object_dict(b).get("type") == "text"
        )
        return {
            "type": "result",
            "subtype": "error_during_execution" if self.error else "success",
            "is_error": self.error is not None,
            "result": text,
            "session_id": self.session_id,
            "stop_reason": self.assistant.get("stop_reason"),
            "permission_denials": self.denials,
            "errors": [self.error] if self.error else [],
            "warnings": self.warnings,
        }


def child_environment(executable: str) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in STRIP_ENV}
    paths = env.get("PATH", "").split(os.pathsep)
    if paths and paths[0] == os.path.dirname(sys.executable):
        paths.pop(0)
    env["PATH"] = os.pathsep.join([os.path.dirname(executable), *paths])
    env["TERM"] = "xterm-256color"
    return env


def write_settings(directory: Path) -> Path:
    hooks: dict[str, object] = {}
    for name in (
        "PermissionRequest",
        "PreToolUse",
        "PermissionDenied",
        "StopFailure",
        "Notification",
    ):
        command: dict[str, object] = {
            "type": "command",
            "command": sys.executable,
            "args": ["-I", str(Path(__file__).resolve()), "hook", str(directory)],
        }
        if name in ("PermissionRequest", "PreToolUse"):
            command.update(timeout=10, onFailure="block")
        rule: dict[str, object] = {"hooks": [command]}
        if name in ("PermissionRequest", "PermissionDenied", "PreToolUse"):
            rule["matcher"] = "AskUserQuestion" if name == "PreToolUse" else "*"
        hooks[name] = [rule]
    path = directory / "settings.json"
    path.write_text(json.dumps({"hooks": hooks}), encoding="utf-8")
    return path


def hook_main(directory: Path) -> int:
    event = cast(dict[str, object], json.load(sys.stdin))
    # One atomic file per hook also works for simultaneous requests; incomplete writes
    # never look like valid events to the parent. All files live in this invocation only.
    path = directory / f"event-{time.time_ns()}-{uuid.uuid4().hex}.json"
    staging = path.with_suffix(".tmp")
    staging.write_text(json.dumps(event), encoding="utf-8")
    staging.replace(path)
    name = event.get("hook_event_name")
    if name == "PermissionRequest":
        emit(
            {
                "hookSpecificOutput": {
                    "hookEventName": name,
                    "decision": {"behavior": "deny", "message": DENIAL},
                }
            }
        )
    elif name == "PreToolUse" and event.get("tool_name") == "AskUserQuestion":
        emit(
            {
                "hookSpecificOutput": {
                    "hookEventName": name,
                    "permissionDecision": "deny",
                    "permissionDecisionReason": DENIAL,
                }
            }
        )
    return 0


class Screen:
    """Small VT screen view for Ink's cursor moves and erases, not a terminal emulator.

    Stripping ANSI from a rolling log alone leaves erased dialogs on the screen and
    would mistake a dismissed resume menu for a live one. Ignore styling and OSC.
    """

    def __init__(self) -> None:
        self.lines = [[" "] * 200 for _ in range(50)]
        self.row = 0
        self.col = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, data: bytes) -> int:
        self.pending += self.decoder.decode(data)
        queries = 0
        while self.pending:
            if self.pending.startswith("\x1b"):
                match = ANSI.match(self.pending)
                if match is None:
                    if len(self.pending) < 256:
                        break  # An escape sequence split across PTY reads.
                    match = re.match(r"\x1b", self.pending)  # Unterminated: drop the ESC only.
                    assert match is not None
                code = match.group(0)
                self.pending = self.pending[len(code) :]
                if code == "\x1b[6n":
                    queries += 1
                self.escape(code)
            else:
                char, self.pending = self.pending[0], self.pending[1:]
                self.character(char)
        return queries

    def character(self, char: str) -> None:
        if char == "\r":
            self.col = 0
        elif char == "\n":
            self.row += 1
        elif char == "\b":
            self.col = max(0, self.col - 1)
        elif char >= " " and char != "\x7f":
            if self.col >= 200:
                self.col = 0
                self.row += 1
            self.scroll()
            self.lines[self.row][self.col] = char
            self.col += 1
        self.scroll()

    def scroll(self) -> None:
        if self.row >= 50:
            self.lines.pop(0)
            self.lines.append([" "] * 200)
            self.row = 49

    def escape(self, code: str) -> None:
        if not code.startswith("\x1b["):
            return
        command = code[-1]
        params = code[2:-1]
        if params and not all(c.isdigit() or c == ";" for c in params):
            return
        values = [int(v or 0) for v in params.split(";")]
        n = values[0] or 1
        if command in ("H", "f"):
            self.row = min(49, max(0, n - 1))
            self.col = min(199, max(0, (values[1] or 1) - 1)) if len(values) > 1 else 0
        elif command in ("A", "B"):
            self.row = max(0, min(49, self.row + (n if command == "B" else -n)))
        elif command in ("C", "D"):
            self.col = max(0, min(199, self.col + (n if command == "C" else -n)))
        elif command == "G":
            self.col = min(199, n - 1)
        elif command == "J":
            start = 0 if values[0] in (1, 2, 3) else self.row
            end = self.row + 1 if values[0] == 1 else 50
            for row in range(start, end):
                self.lines[row] = [" "] * 200
        elif command == "K":
            start = 0 if values[0] in (1, 2) else min(self.col, 200)
            end = min(self.col + 1, 200) if values[0] == 1 else 200
            self.lines[self.row][start:end] = [" "] * (end - start)

    def text(self) -> str:
        return "\n".join("".join(line).rstrip() for line in self.lines).strip()


class Terminal:
    def __init__(self, executable: str, args: list[str]) -> None:
        import fcntl
        import struct
        import termios

        self.screen = Screen()
        self.outgoing = bytearray()
        self.master, slave = os.openpty()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))
            self.process = subprocess.Popen(
                [executable, *args],
                stdin=slave,
                stdout=slave,
                stderr=slave,
                env=child_environment(executable),
                close_fds=True,
            )
        except BaseException:
            os.close(self.master)
            raise
        finally:
            os.close(slave)
        os.set_blocking(self.master, False)
        # Refreshed while running: on cancellation Claude may exit before close() looks.
        self.groups: set[int] = set()
        self.groups_at = 0.0

    def poll(self) -> str:
        if time.monotonic() - self.groups_at >= 2:
            self.groups |= spawned_groups(self.process.pid)
            self.groups_at = time.monotonic()
        readable, writable, _ = select.select(
            [self.master],
            [self.master] if self.outgoing else [],
            [],
            0.05,
        )
        if readable:
            try:
                data = os.read(self.master, 65536)
            except OSError:
                data = b""
            self.outgoing.extend(b"\x1b[1;1R" * self.screen.feed(data))
        if writable:
            try:
                count = os.write(self.master, self.outgoing[:65536])
                del self.outgoing[:count]
            except BlockingIOError:
                pass
        return self.screen.text()

    def close(self) -> None:
        """Stop Claude and the commands it started in their own process groups, within the
        supervisor's two-second grace: those groups would outlive the supervisor's cleanup."""
        try:
            groups = self.groups | spawned_groups(self.process.pid)
            signal_groups(groups, signal.SIGTERM)
            if self.process.poll() is None:
                self.process.terminate()
                try:
                    self.process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
            signal_groups(groups, signal.SIGKILL)
        finally:
            os.close(self.master)


def spawned_groups(root: int) -> set[int]:
    """Process groups led by descendants of root, other than this helper's own group."""
    listing = subprocess.run(
        ["ps", "-A", "-o", "pid=,ppid=,pgid="], capture_output=True, text=True, check=False
    ).stdout
    children: dict[int, list[int]] = {}
    groups: dict[int, int] = {}
    for line in listing.splitlines():
        fields = line.split()
        if len(fields) == 3 and all(field.isdigit() for field in fields):
            pid, ppid, pgid = (int(field) for field in fields)
            children.setdefault(ppid, []).append(pid)
            groups[pid] = pgid
    found: set[int] = set()
    stack = [root]
    while stack:
        for child in children.get(stack.pop(), []):
            found.add(child)
            stack.append(child)
    own = os.getpgrp()
    return {pid for pid in found if groups.get(pid) == pid and pid != own}


def signal_groups(groups: set[int], signum: int) -> None:
    for group in groups:
        with contextlib.suppress(OSError):
            os.killpg(group, signum)


class Transcript:
    def __init__(self, session_id: str) -> None:
        root = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "projects"
        self.pattern = str(root / "*" / (glob.escape(session_id) + ".jsonl"))
        self.path = self.find()
        self.offset = self.path.stat().st_size if self.path is not None else 0

    def find(self) -> Path | None:
        paths = glob.glob(self.pattern)
        return Path(paths[0]) if paths else None

    def read(self) -> list[dict[str, object]]:
        self.path = self.path or self.find()
        if self.path is None:
            return []
        rows: list[dict[str, object]] = []
        with self.path.open("rb") as source:
            source.seek(self.offset)
            while line := source.readline():
                if not line.endswith(b"\n"):
                    break
                self.offset = source.tell()
                try:
                    value: object = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict):
                    rows.append(cast(dict[str, object], value))
        return rows


def read_hooks(directory: Path, turn: Turn) -> None:
    for path in sorted(directory.glob("event-*.json")):
        turn.hook(cast(dict[str, object], json.loads(path.read_text(encoding="utf-8"))))
        path.unlink()


def blocked(turn: Turn, reason: str, screen: str = "") -> None:
    excerpt = " ".join(screen.split())[-600:]
    turn.error = f"permission required: Claude interactive runtime blocked: {reason}"
    if excerpt:
        turn.error += f". Screen: {excerpt}"


def await_ready(terminal: Terminal, turn: Turn) -> bool:
    deadline = time.monotonic() + STARTUP_TIMEOUT
    previous, stable_since = "", time.monotonic()
    resumed = False
    while time.monotonic() < deadline:
        screen = terminal.poll()
        now = time.monotonic()
        if screen != previous:
            previous, stable_since = screen, now
        kind = classify_screen(screen)
        if kind in ("trust", "summary", "login", "permission"):
            reasons = {
                "trust": "directory is not trusted; pass interactive=false for this run or trust it in Claude Code",
                "summary": "summary resume selection is not supported in this phase",
                "login": "handle login or onboarding in Claude Code",
                "permission": "unexpected startup permission dialog; handle it in Claude Code",
            }
            blocked(turn, reasons[kind])
            return False
        if kind == "cold_resume" and not resumed and now - stable_since >= 0.5:
            turn.warnings.append(cold_resume_warning(screen))
            terminal.outgoing.extend(b"\x1b")
            resumed = True
        if kind == "ready" and now - stable_since >= 1 and not terminal.outgoing:
            return True
        if terminal.process.poll() is not None:
            blocked(turn, "Claude exited before becoming ready", screen)
            return False
    blocked(
        turn, "startup timed out without becoming ready; handle the screen in Claude Code", previous
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
    while True:
        screen = terminal.poll()
        now = time.monotonic()
        read_hooks(directory, turn)
        for row in transcript.read():
            turn.transcript(row)
        if turn.error or turn.done:
            if turn.done and not turn.assistant and not turn.error:
                turn.error = "Claude turn ended without an assistant message"
            return
        if terminal.process.poll() is not None:
            turn.error = f"Claude exited unexpectedly with status {terminal.process.returncode}"
            return
        if classify_screen(screen) == "permission":
            dialog_since = dialog_since or now
            if now - dialog_since > 1:
                reason = (
                    "permission dialog remained after hook denial"
                    if turn.gate_seen
                    else "permission dialog without a corresponding hook request"
                )
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


def run_main(executable: str, session_id: str | None, extra: list[str], prompt: str) -> int:
    turn = Turn(session_id or str(uuid.uuid4()))
    emit({"type": "system", "subtype": "init", "session_id": turn.session_id})
    terminal: Terminal | None = None
    try:
        if not os.path.isabs(executable):
            raise ValueError("Claude executable must be an absolute path")
        if any(c in turn.session_id for c in ("/", "\\")):
            raise ValueError("invalid session id")
        transcript = Transcript(turn.session_id)
        with tempfile.TemporaryDirectory(prefix="agentnave-claude-") as tmp:
            directory = Path(tmp)
            args = ["--resume" if session_id else "--session-id", turn.session_id]
            args.extend([*extra, "--settings", str(write_settings(directory))])
            try:
                terminal = Terminal(executable, args)
                if await_ready(terminal, turn):
                    await_turn(terminal, turn, transcript, directory, prompt)
                read_hooks(directory, turn)
            finally:
                if terminal is not None:
                    terminal.close()
    except (OSError, ValueError) as exc:
        blocked(turn, str(exc))
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
    run.add_argument("extra", nargs=argparse.REMAINDER)
    hook = sub.add_parser("hook")
    hook.add_argument("directory", type=Path)
    args = parser.parse_args()
    if args.command == "hook":
        return hook_main(args.directory)
    signal.signal(signal.SIGTERM, terminate)
    extra = cast(list[str], args.extra)
    return run_main(
        args.claude, args.resume, extra[1:] if extra[:1] == ["--"] else extra, sys.stdin.read()
    )


if __name__ == "__main__":
    raise SystemExit(main())

"""Reading the Claude Code screen: what it shows and which menu choice is selected."""

from __future__ import annotations

import codecs
import os
import re

# CSI, OSC and DCS strings, then any other escape: optional intermediates and a final byte
# (DECSC `\x1b7`, keypad `\x1b=`, charset `\x1b(B`). An unknown one must never stall the screen.
ANSI = re.compile(
    r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|P[^\x1b]*\x1b\\|(?![\[\]P])[ -/]*[0-~])"
)


def clean_paste(text: str) -> str:
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = "".join(c for c in text if c in "\n\t" or (ord(c) >= 32 and ord(c) != 127))
    return "\n" + text if text.lstrip().startswith(("/", "!", "#", "&")) else text


RULE = re.compile(r"─{20,}")


def workspace_matches(text: str, cwd: str) -> bool:
    """Accept only one complete path displayed before the trust choices."""
    before_choices = re.split(r"(?:yes, i trust this folder|no, exit)", text, flags=re.I)[0]
    lines = [line.strip() for line in ANSI.sub("", before_choices).splitlines()]
    # agy 1.5 prints "Accessing workspace: <path>"; Claude prints the path on its own line.
    paths = [line.removeprefix("Accessing workspace:").strip() for line in lines]
    paths = [path for path in map(os.path.expanduser, paths) if os.path.isabs(path)]
    if len(paths) != 1:
        return False
    try:
        return os.path.samefile(paths[0], cwd)
    except OSError:
        return False


def dialog_region(text: str) -> str:
    """The screen below the full-width rule above the last menu cursor or prompt: the dialog or
    input box. The conversation above it can quote any dialog's words."""
    lines = ANSI.sub("", text).splitlines()
    cursor = max((i for i, line in enumerate(lines) if "❯" in line), default=len(lines))
    rule = max((i for i in range(cursor) if RULE.fullmatch(lines[i].strip())), default=-1)
    return "\n".join(lines[rule + 1 :])


def in_input_box(text: str) -> bool:
    """The last cursor sits above the input box's lower border: in typed or pasted text."""
    lines = ANSI.sub("", text).splitlines()
    cursor = max((i for i, line in enumerate(lines) if "❯" in line), default=len(lines))
    return any(RULE.fullmatch(line.strip()) for line in lines[cursor + 1 :])


def dialog_open(text: str) -> bool:
    """A menu is open: its footer is the screen's last line, below anything Claude or the task
    text says. The input box's footer is the status line instead."""
    if in_input_box(text):
        return False
    region = dialog_region(text)
    footer = region.lower().splitlines()[-1:] or [""]
    return any(
        s in footer[0] for s in ("esc to cancel", "enter to confirm", "enter to select")
    ) or (classify_screen(text) == "permission" and re.search(r"❯\s*\d+\.", region) is not None)


def classify_screen(text: str) -> str:
    """Dialogs take precedence over the prompt, including the prompt inside a menu."""
    lowered = dialog_region(text).lower()
    if "allow external claude.md file imports?" in lowered:
        return "external_imports"
    if "trust this folder" in lowered or "do you trust" in lowered:
        return "trust"
    if "resume from summary" in lowered or "resume full session as-is" in lowered:
        return "summary"
    if "resume this conversation?" in lowered:
        return "cold_resume"
    if "choose the text style" in lowered or (
        "security notes" in lowered and "press enter to continue" in lowered
    ):
        return "onboarding_enter"
    if "terminal setup?" in lowered:
        return "onboarding_skip"
    if "do you want to use this api key" in lowered:
        return "api_key"
    if "bypass permissions mode" in lowered and "i accept" in lowered:
        return "mode_consent"
    if any(
        s in lowered
        for s in (
            "select login method",
            "/login",
            "paste code here",
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
            "would you like to proceed",
            "requests your input",
        )
    ):
        return "permission"
    if "enter to confirm" in lowered or "press enter to continue" in lowered:
        return "unknown"
    return "ready" if "❯" in lowered else "unknown"


def selected(text: str, label: str) -> bool:
    """Whether the menu's cursor is on the choice labelled exactly `label`."""
    return any(
        "❯" in line and line.replace("❯", "").strip() == label
        for line in ANSI.sub("", text).lower().splitlines()
    )


# Menus whose default is to refuse: the answer, the key that moves to it, and the warning.
CHOICES = {
    "trust": (
        "yes, i trust this folder",
        b"\x1b[B",
        "Claude Code now trusts {cwd}: the interactive runtime accepted its folder trust prompt",
    ),
    "api_key": (
        "yes",
        b"\x1b[A",
        "Claude Code now uses ANTHROPIC_API_KEY from the environment: the interactive runtime "
        "approved it as -p would use it",
    ),
    "external_imports": (
        "yes, allow external imports",
        b"\x1b[B",
        "Claude Code approved external CLAUDE.md imports, including future imports",
    ),
}


def cold_resume_warning(text: str) -> str:
    flat = " ".join(ANSI.sub("", text).split())
    match = re.search(r"This conversation has been inactive.*?5-hour usage limit\.", flat, re.I)
    return match.group(0) if match else "Claude displayed a cold conversation resume usage warning."


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

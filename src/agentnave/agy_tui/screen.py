"""agy's screen and the state its bottom lines show, read as awslabs' CAO reads it (ADR-0072)."""

from __future__ import annotations

import re

from agentnave.claude_tui.screen import ANSI, RULE, Screen

_SPINNER = re.compile(r"[⠀-⣿]")
# The input box, its rules and the footer; a menu replaces the box with its own lines.
BOTTOM = 4
# Background tasks add a panel under the input box, one line each plus a rule, and the footer
# counts them.
_TASKS = re.compile(r"(\d+) task\(s\)")


class AgyScreen(Screen):
    """Adds the edits agy 1.5 sends that Claude's Ink does not: erase and delete characters
    (`CSI X`, `CSI P`) and reverse index (`ESC M`). Without them old text stays beside the
    new, and the footer reads as glued to a rule."""

    def escape(self, code: str) -> None:
        if code == "\x1bM":
            if self.row == 0:
                self.lines.insert(0, [" "] * 200)
                self.lines.pop()
            else:
                self.row -= 1
            return
        if (
            code.startswith("\x1b[")
            and code[-1] in "XP"
            and (code[2:-1].isdigit() or code == code[:2] + code[-1])
        ):
            count = int(code[2:-1] or 1)
            line = self.lines[self.row]
            end = min(self.col + count, 200)
            if code[-1] == "X":
                line[self.col : end] = [" "] * (end - self.col)
            else:
                del line[self.col : end]
                line.extend([" "] * (200 - len(line)))
            return
        super().escape(code)


def bottom(screen: str) -> list[str]:
    lines = [line.strip() for line in ANSI.sub("", screen).splitlines() if line.strip()]
    tasks = _TASKS.search(lines[-1]) if lines else None
    return lines[-(BOTTOM + (int(tasks.group(1)) + 1 if tasks else 0)) :]


def state(screen: str) -> str:
    """menu, busy, idle or unknown, in that order: a menu can sit above the idle footer, and
    the conversation above the box can quote any of these words."""
    lines = bottom(screen)
    lowered = [line.lower() for line in lines]
    if any(("navigate" in line and "enter confirm" in line) or "[y/n]" in line for line in lowered):
        return "menu"
    if any("esc to cancel" in line or _SPINNER.match(line) for line in lowered):
        return "busy"
    if (
        lowered
        and lowered[-1].startswith("? for shortcuts")
        and any(line.startswith(">") for line in lines)
    ):
        return "idle"
    return "unknown"


def input_box(screen: str) -> str:
    """The text between the input box's two rules, the last two on the screen."""
    lines = [line.strip() for line in ANSI.sub("", screen).splitlines()]
    rules = [i for i, line in enumerate(lines) if RULE.fullmatch(line)]
    return " ".join(" ".join(lines[rules[-2] + 1 : rules[-1]]).split()) if len(rules) > 1 else ""


def busy_activity(screen: str) -> str | None:
    """Read the spinner immediately above the input box, optionally above its tip."""
    if state(screen) != "busy":
        return None
    lines = [line.strip() for line in ANSI.sub("", screen).splitlines() if line.strip()]
    rules = [i for i, line in enumerate(lines) if RULE.fullmatch(line)]
    boxes = [
        upper
        for upper, lower in zip(rules, rules[1:], strict=False)
        if any(line.startswith(">") for line in lines[upper + 1 : lower])
    ]
    if not boxes:
        return None
    before = boxes[-1] - 1
    if before >= 0 and lines[before].startswith("└ Tip:"):
        before -= 1
    if before < 0 or not (match := re.fullmatch(r"[⠀-⣿]  (.+)", lines[before])):
        return None
    text = match.group(1).lower()
    for prefix, tool in (
        ("running command", "command_execution"),
        ("reading file", "read_file"),
        ("editing file", "file_change"),
        ("searching web", "web_search"),
    ):
        if text.startswith(prefix) and text.endswith("..."):
            return tool
    return "thinking"


TRUST_QUESTION = "Do you trust the contents of this project?"
TRUST_CHOICES = ("Yes, I trust this folder", "No, exit")


def trust_menu(screen: str) -> str | None:
    """The trust dialog when it is the menu at the bottom: its two choices right above the
    navigation footer, its question and "Accessing workspace:" just above them. The
    conversation above can quote the same words, so only these bottom lines count."""
    lines = [line.strip() for line in ANSI.sub("", screen).splitlines() if line.strip()]
    if len(lines) < 5 or "navigate" not in lines[-1].lower():
        return None
    choices = [line.removeprefix(">").strip() for line in lines[-3:-1]]
    if tuple(choices) != TRUST_CHOICES:
        return None
    first = max(0, len(lines) - 8)
    above = lines[first:-3]
    if TRUST_QUESTION not in above:
        return None
    question = first + above.index(TRUST_QUESTION)
    start = next(
        (
            i
            for i in range(question - 1, max(question - 4, -1), -1)
            if lines[i].startswith("Accessing workspace:")
        ),
        None,
    )
    return None if start is None else "\n".join(lines[start:])

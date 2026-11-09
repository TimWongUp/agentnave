"""agy's screen and the state its bottom lines show, read as awslabs' CAO reads it (ADR-0072)."""

from __future__ import annotations

import re

from agentnave.claude_tui.screen import ANSI, RULE, Screen

_SPINNER = re.compile(r"[⠀-⣿]")
# The input box, its rules and the footer; a menu replaces the box with its own lines.
BOTTOM = 4


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
    return lines[-BOTTOM:]


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

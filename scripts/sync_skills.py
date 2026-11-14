"""Build agentnave-meeting from agentnave-manager, so the parts both Skills share stay one text.

Each `<!-- shared:NAME -->` block in the meeting Skill takes the manager's block of the same name;
the listed references are copied whole. Run with --check to fail when the meeting Skill is stale.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SKILLS = Path(__file__).resolve().parent.parent / "skills"
SOURCE = SKILLS / "agentnave-manager"
TARGET = SKILLS / "agentnave-meeting"
REFERENCES = (
    "studio.md",
    "claude.md",
    "codebuddy.md",
    "codex.md",
    "grok.md",
    "antigravity.md",
    "pi.md",
    "deepseek.md",
    "devin.md",
    "zcode.md",
)
BLOCK = re.compile(r"<!-- shared:([a-z-]+) -->\n(.*?)<!-- /shared:\1 -->", re.S)


def built() -> dict[Path, str]:
    """Every file of the meeting Skill this script owns, as it should read."""
    source = {m[1]: m[2] for m in BLOCK.finditer((SOURCE / "SKILL.md").read_text(encoding="utf-8"))}
    text = (TARGET / "SKILL.md").read_text(encoding="utf-8")

    def fill(match: re.Match[str]) -> str:
        name = match[1]
        if name not in source:
            raise SystemExit(f"agentnave-manager has no shared block {name!r}")
        return f"<!-- shared:{name} -->\n{source[name]}<!-- /shared:{name} -->"

    files = {TARGET / "SKILL.md": BLOCK.sub(fill, text)}
    for name in REFERENCES:
        files[TARGET / "references" / name] = (SOURCE / "references" / name).read_text(
            encoding="utf-8"
        )
    return files


def stale() -> list[Path]:
    return [
        path
        for path, text in built().items()
        if not path.is_file() or path.read_text(encoding="utf-8") != text
    ]


def main() -> int:
    if "--check" in sys.argv[1:]:
        if paths := stale():
            names = ", ".join(str(path.relative_to(SKILLS)) for path in paths)
            print(
                f"agentnave-meeting is stale: {names}; run scripts/sync_skills.py", file=sys.stderr
            )
            return 1
        return 0
    for path, text in built().items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

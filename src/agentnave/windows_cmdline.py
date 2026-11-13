"""Command lines for Windows process creation, safe for `.cmd`/`.bat` entry points."""

from __future__ import annotations

import ntpath
import os
import subprocess
import sys

# Rust std's BatBadBut fix (GHSA-q455-m56c-85mh): any other ASCII symbol is quoted.
_BATCH_UNQUOTED = frozenset("#$*+-./:?@\\_")


def windows_command_line(argv: list[str]) -> tuple[str | None, str]:
    """Application name and command line for ``CreateProcessW``.

    Windows runs ``.cmd``/``.bat`` through cmd.exe, which ignores MSVCRT quoting and
    acts on ``&``, ``|``, ``%`` and the like; build that line as Rust std does, and
    reject arguments cmd.exe cannot carry intact.
    """
    script = argv[0]
    if ntpath.splitext(script)[1].lower() not in (".bat", ".cmd"):
        return None, subprocess.list2cmdline(argv)
    if '"' in script or script.endswith("\\"):
        raise ValueError(f"batch file path cannot be passed to cmd.exe: {script}")
    # The same `%cd:~,%` trick as in arguments keeps cmd.exe from expanding `%VAR%` in the path.
    parts = ['"' + script.replace("%", "%%cd:~,%") + '"']
    for arg in argv[1:]:
        if any(c in arg for c in "\r\n\0"):
            raise ValueError("batch file arguments cannot contain CR, LF or NUL")
        parts.append(_batch_argument(arg))
    system_root = os.environ.get("SYSTEMROOT", r"C:\Windows")
    command = f'cmd.exe /e:ON /v:OFF /d /c "{" ".join(parts)}"'
    return ntpath.join(system_root, "System32", "cmd.exe"), command


def _batch_argument(arg: str) -> str:
    quote = (
        not arg
        or arg.endswith("\\")
        or any(
            (c.isascii() and not c.isalnum() and c not in _BATCH_UNQUOTED) or not c.isprintable()
            for c in arg
        )
    )
    out: list[str] = []
    backslashes = 0
    for c in arg:
        if c == "\\":
            backslashes += 1
        else:
            if c == '"':
                out.append("\\" * backslashes + '"')
            elif c == "%":
                # `%cd:~,%` expands to nothing, so cmd.exe cannot expand `%VAR%`.
                out.append("%%cd:~,")
            backslashes = 0
        out.append(c)
    if quote:
        return '"' + "".join(out) + "\\" * backslashes + '"'
    return "".join(out)


def popen_args(argv: list[str]) -> tuple[str | list[str], str | None]:
    """`args` and `executable` for `subprocess.Popen`: on Windows a batch entry point gets the
    cmd.exe-safe line, which Popen would otherwise build with MSVCRT quoting."""
    if sys.platform != "win32":
        return argv, None
    application, line = windows_command_line(argv)
    return line, application

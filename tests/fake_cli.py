"""Helper to install a Python fake provider CLI that also runs on Windows."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def write_fake_cli(directory: Path, name: str, body: str) -> Path:
    """Write a fake executable `name` whose Python source is body.

    POSIX gets a shebang script; Windows gets name.py plus the name.cmd wrapper it resolves.
    """
    script = directory / (f"{name}.py" if os.name == "nt" else name)
    script.write_text(f"#!{sys.executable}\n{body}", encoding="utf-8")
    script.chmod(0o755)
    if os.name == "nt":
        (directory / f"{name}.cmd").write_text(
            f'@"{sys.executable}" "%~dp0{name}.py" %*\n', encoding="utf-8"
        )
    return script

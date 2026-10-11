"""Check installed launchers: exact tool lists, no files before a write, no port before
open_dashboard. Usage: python verify_install.py <agentnave-mcp> <agentnave-studio-mcp>"""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from mcp import Client, StdioServerParameters

CORE = [
    "start_agent",
    "wait_agent",
    "cancel_agent",
    "describe_provider",
    "open_group",
    "run_round",
    "wait_round",
    "cancel_round",
]
STUDIO = [
    *CORE[:6],
    "wait_round",
    "cancel_round",
    "decide_answers",
    "post_host_message",
    "list_conversations",
    "read_conversation",
    "update_conversation",
    "delete_conversation",
    "open_dashboard",
]


def _listening() -> bool:
    """Whether a server process started by this script listens on TCP (POSIX only)."""
    children = subprocess.run(
        ["pgrep", "-P", str(os.getpid())], capture_output=True, text=True
    ).stdout.split()
    return any(
        subprocess.run(
            ["lsof", "-a", "-p", pid, "-iTCP", "-sTCP:LISTEN", "-t"], capture_output=True
        ).stdout.strip()
        for pid in children
    )


async def _check(launcher: str, expected: list[str], home: Path, *, studio: bool) -> None:
    env = {key: value for key, value in os.environ.items() if key != "AGENTNAVE_DATA_DIR"}
    env.update(HOME=str(home), USERPROFILE=str(home))
    async with Client(StdioServerParameters(command=launcher, env=env, cwd=home)) as client:
        names = [tool.name for tool in (await client.list_tools()).tools]
        assert names == expected, names
        assert not any(home.iterdir()), f"{launcher} wrote files before any request"
        posix = os.name != "nt"
        if posix:
            assert not _listening(), f"{launcher} listens before open_dashboard"
        if studio:
            opened = await client.call_tool("open_dashboard", {})
            assert not opened.is_error, opened.content
            if posix:
                assert _listening(), "open_dashboard did not start the dashboard"


async def main(core: str, studio: str) -> None:
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        await _check(core, CORE, home, studio=False)
        await _check(studio, STUDIO, home, studio=True)
    print("installed launchers verified")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1], sys.argv[2]))

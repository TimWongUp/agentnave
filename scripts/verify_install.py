"""Check installed launchers: exact tool lists, no files before a write, no port before
first saved operation or history listing. Usage: python verify_install.py <agentnave-mcp> <agentnave-studio-mcp>"""

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
    "wait_any",
    "cancel_agent",
    "list_invocations",
    "describe_provider",
    "open_group",
    "run_round",
    "wait_round",
    "cancel_round",
]
STUDIO = [
    *CORE[:8],
    "wait_round",
    "cancel_round",
    "decide_answers",
    "post_host_message",
    "list_conversations",
    "read_conversation",
    "update_conversation",
    "delete_conversation",
]


_WINDOWS_LISTENERS = """
$ids = @({pid}); $all = Get-CimInstance Win32_Process
do {{
  $new = @($all | Where-Object {{ $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId }} | ForEach-Object {{ $_.ProcessId }})
  $ids += $new
}} while ($new.Count)
@(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object {{ $ids -contains $_.OwningProcess -and $_.OwningProcess -ne {pid} }}).Count
"""


def _listening() -> bool:
    """Whether a server process started by this script listens on TCP."""
    if os.name == "nt":
        count = subprocess.run(
            ["powershell", "-NoProfile", "-Command", _WINDOWS_LISTENERS.format(pid=os.getpid())],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
        return int(count.strip()) > 0
    pids: list[str] = []
    parents = [str(os.getpid())]
    while parents:  # launchers may start the server as a grandchild
        parents = subprocess.run(
            ["pgrep", "-P", ",".join(parents)], capture_output=True, text=True
        ).stdout.split()
        pids += parents
    return any(
        subprocess.run(
            ["lsof", "-a", "-p", pid, "-iTCP", "-sTCP:LISTEN", "-t"], capture_output=True
        ).stdout.strip()
        for pid in pids
    )


async def _check(launcher: str, expected: list[str], home: Path, *, studio: bool) -> None:
    env = {key: value for key, value in os.environ.items() if key != "AGENTNAVE_DATA_DIR"}
    env.update(HOME=str(home), USERPROFILE=str(home))
    async with Client(StdioServerParameters(command=launcher, env=env, cwd=home)) as client:
        names = [tool.name for tool in (await client.list_tools()).tools]
        assert names == expected, names
        assert not any(home.iterdir()), f"{launcher} wrote files before any request"
        assert not _listening(), f"{launcher} listens before a saved operation or history listing"
        if studio:
            opened = await client.call_tool("list_conversations", {})
            assert not opened.is_error, opened.content
            assert _listening(), "history listing did not start the dashboard"


async def main(core: str, studio: str) -> None:
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        await _check(core, CORE, home, studio=False)
        await _check(studio, STUDIO, home, studio=True)
    print("installed launchers verified")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1], sys.argv[2]))

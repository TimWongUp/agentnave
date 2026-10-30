"""Check the installed launcher and its on-demand dashboard.

Usage: python verify_install.py <agentnave-mcp> [legacy-studio-launcher]
"""

from __future__ import annotations

import asyncio
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from mcp import Client, StdioServerParameters

TOOLS = [
    "start_agent",
    "wait",
    "cancel",
    "list",
    "list_members",
    "run_round",
    "decide_answers",
    "handover",
    "read_conversation",
]


# Windows reuses PIDs and never updates ParentProcessId, so a process older than the
# one now holding its parent's PID is unrelated and must not count as a descendant.
_WINDOWS_LISTENERS = """
$ids = @({pid}); $all = Get-CimInstance Win32_Process
$created = @{{}}; foreach ($p in $all) {{ $created[$p.ProcessId] = $p.CreationDate }}
do {{
  $new = @($all | Where-Object {{ $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId -and $_.CreationDate -ge $created[$_.ParentProcessId] }} | ForEach-Object {{ $_.ProcessId }})
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


async def _check(launcher: str, home: Path) -> None:
    env = {key: value for key, value in os.environ.items() if key != "AGENTNAVE_DATA_DIR"}
    env.update(HOME=str(home), USERPROFILE=str(home))
    async with Client(StdioServerParameters(command=launcher, env=env, cwd=home)) as client:
        names = [tool.name for tool in (await client.list_tools()).tools]
        assert names == TOOLS, names
        assert not any(home.iterdir()), f"{launcher} wrote files before any request"
        assert not _listening(), f"{launcher} listens before a saved operation or history listing"
        opened = await client.call_tool("list", {})
        assert not opened.is_error, opened.content
        assert _listening(), "history listing did not start the dashboard"


async def main(launcher: str, *legacy_launchers: str) -> None:
    for legacy in legacy_launchers:
        assert not Path(legacy).exists(), f"legacy launcher still installed: {legacy}"
    import agentnave

    skills = Path(agentnave.__file__).resolve().parent / "skills"
    assert (skills / "agentnave-manager/SKILL.md").is_file(), "missing bundled agentnave-manager"
    assert (skills / "agentnave-review/SKILL.md").is_file(), "missing bundled agentnave-review"
    with tempfile.TemporaryDirectory() as directory:
        home = Path(directory)
        await _check(launcher, home)
    print("installed launcher verified")


if __name__ == "__main__":
    asyncio.run(main(*sys.argv[1:]))

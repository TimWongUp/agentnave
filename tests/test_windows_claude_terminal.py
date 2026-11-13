# pyright: reportAttributeAccessIssue=false, reportUnknownMemberType=false, reportUnknownVariableType=false
"""Exercise the actual Windows pseudoconsole without invoking Claude."""

import asyncio
import json
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import pytest

from agentnave.adapters.claude import ClaudeAdapter
from agentnave.core import InvocationManager
from agentnave.models import InvocationRequest
from agentnave.processes import spawn_process, terminate_process_tree


@pytest.mark.skipif(os.name != "nt", reason="Windows pseudoconsole")
def test_terminal_reads_and_writes_under_conpty() -> None:
    from agentnave.claude_tui.windows_terminal import WindowsTerminal

    code = "import sys; print('READY', flush=True); print('GOT:' + input(), flush=True)"
    terminal = WindowsTerminal(sys.executable, ["-u", "-c", code])
    try:
        deadline = time.monotonic() + 10
        while "READY" not in terminal.poll() and time.monotonic() < deadline:
            pass
        terminal.outgoing.extend(b"hello\r")
        text = ""
        while "GOT:hello" not in text and time.monotonic() < deadline:
            text = terminal.poll()
        assert "GOT:hello" in text
    finally:
        terminal.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows pseudoconsole")
@pytest.mark.asyncio
async def test_windows_helper_delivers_prompt_and_answers_hook(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "fake.py"
    fake.write_text(
        r"""import ctypes, json, msvcrt, os, subprocess, sys, time
from pathlib import Path
from ctypes import wintypes

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.SetConsoleOutputCP(65001)
kernel.SetConsoleCP(65001)
kernel.GetStdHandle.restype = wintypes.HANDLE
handle = kernel.GetStdHandle(-10)
mode = wintypes.DWORD()
assert kernel.GetConsoleMode(handle, ctypes.byref(mode))
assert kernel.SetConsoleMode(handle, (mode.value & ~6) | 0x200)
msvcrt.setmode(0, os.O_BINARY)
msvcrt.setmode(1, os.O_BINARY)
args = sys.argv[1:]
session = args[args.index("--session-id") + 1]
settings = json.loads(Path(args[args.index("--settings") + 1]).read_text())
transcript = Path(os.environ["CLAUDE_CONFIG_DIR"]) / "projects" / "fake" / (session + ".jsonl")
transcript.parent.mkdir(parents=True, exist_ok=True)

def send(text):
    os.write(1, text.encode("utf-8"))

def receive(marker):
    data = b""
    while marker not in data:
        chunk = os.read(0, 1)
        if not chunk:
            raise RuntimeError("input closed")
        data += chunk
    return data

send("❯ ")
paste = receive(b"\x1b[201~")
assert b"\x1b[200~" in paste and "你好 hello".encode() in paste
send("[Pasted text: 2 lines]")
receive(b"\r")
with transcript.open("a", encoding="utf-8") as output:
    output.write(json.dumps({"type":"user","message":{"content":"hello"}}) + "\n")
hook = settings["hooks"]["PermissionRequest"][0]["hooks"][0]
event = {"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"echo ok"}}
reply = subprocess.run([hook["command"], *hook["args"]], input=json.dumps(event).encode(), capture_output=True, timeout=15)
assert reply.returncode == 0, reply.stderr
assert json.loads(reply.stdout)["hookSpecificOutput"]["decision"]["behavior"] == "allow"
with transcript.open("a", encoding="utf-8") as output:
    output.write(json.dumps({"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}) + "\n")
    output.write(json.dumps({"type":"system","subtype":"turn_duration"}) + "\n")
time.sleep(60)
""",
        encoding="utf-8",
    )
    wrapper = tmp_path / "fake-claude.cmd"
    wrapper.write_text(f'@"{sys.executable}" -u "{fake}" %*\r\n')
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "config"))
    adapter = ClaudeAdapter()
    monkeypatch.setattr(adapter, "executable_path", lambda: str(wrapper))
    command = adapter.prepare(
        InvocationRequest("claude", "你好 hello", tmp_path, provider_options={"interactive": True})
    )
    supervised = await spawn_process(command.argv, tmp_path)
    process = supervised.process
    assert process.stdin is not None and process.stdout is not None and process.stderr is not None
    events: list[dict[str, object]] = []

    async def collect() -> None:
        assert process.stdin is not None and process.stdout is not None
        process.stdin.write(command.stdin or b"")
        await process.stdin.drain()
        while line := await process.stdout.readline():
            event = json.loads(line)
            events.append(event)
            if event.get("type") == "agentnave_prompt":
                answer = {
                    "type": "agentnave_answer",
                    "prompt_id": event["prompt_id"],
                    "decision": "allow",
                }
                process.stdin.write((json.dumps(answer) + "\n").encode())
                await process.stdin.drain()

    try:
        await asyncio.wait_for(collect(), timeout=30)
        await process.wait()
        stderr = (await process.stderr.read()).decode(errors="replace")
        completion = await supervised.completion
        assert process.returncode == 0, (events, stderr)
        assert completion.kind == "exit" and completion.returncode == 0
        assert any(event.get("type") == "agentnave_prompt" for event in events)
        assert events[-1].get("result") == "done"
    finally:
        if process.returncode is None:
            await terminate_process_tree(process)
        process.stdin.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows pseudoconsole")
@pytest.mark.asyncio
async def test_cancelling_supervisor_stops_pty_child(tmp_path: Path) -> None:
    import ctypes
    from ctypes import wintypes

    pid_file = tmp_path / "child.pid"
    child = f"import os,time,pathlib; pathlib.Path({str(pid_file)!r}).write_text(str(os.getpid())); time.sleep(60)"
    helper = (
        "import sys,time; from agentnave.claude_tui.windows_terminal import WindowsTerminal; "
        f"terminal=WindowsTerminal(sys.executable, ['-c', {child!r}]); time.sleep(60)"
    )
    supervised = await spawn_process((sys.executable, "-c", helper), tmp_path)
    kernel: Any = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    child_handle = None
    try:
        async with asyncio.timeout(15):
            while not pid_file.exists():
                await asyncio.sleep(0.05)
        child_handle = kernel.OpenProcess(0x00100000, False, int(pid_file.read_text()))
        assert child_handle
        await terminate_process_tree(supervised.process)
        assert kernel.WaitForSingleObject(child_handle, 5000) == 0
    finally:
        if supervised.process.returncode is None:
            await terminate_process_tree(supervised.process)
        await supervised.completion
        if child_handle:
            kernel.CloseHandle(child_handle)


@pytest.mark.skipif(os.name != "nt", reason="Windows pseudoconsole")
@pytest.mark.asyncio
async def test_cancelling_invocation_removes_private_hook_files(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wrapper = tmp_path / "waiting-claude.cmd"
    wrapper.write_text(f'@"{sys.executable}" -c "import time; time.sleep(60)"\r\n')
    monkeypatch.setattr(tempfile, "tempdir", str(tmp_path))

    def executable(_adapter: ClaudeAdapter) -> str:
        return str(wrapper)

    monkeypatch.setattr(ClaudeAdapter, "executable_path", executable)
    manager = InvocationManager()
    request = InvocationRequest("claude", "hello", tmp_path, provider_options={"interactive": True})
    invocation = manager.start(request)
    try:
        async with asyncio.timeout(15):
            while not list(
                tmp_path.glob("agentnave-invocation-*/agentnave-claude-*/settings.json")
            ):
                await asyncio.sleep(0.05)
        await manager.cancel(invocation)
        assert not list(tmp_path.glob("agentnave-invocation-*"))
    finally:
        await manager.shutdown()


@pytest.mark.skipif(os.name != "nt", reason="Windows pseudoconsole")
def test_terminal_can_run_windows_command_wrapper(tmp_path: Path) -> None:
    from agentnave.claude_tui.windows_terminal import WindowsTerminal

    wrapper = tmp_path / "claude.cmd"
    wrapper.write_text("@echo %*\r\n@echo READY\r\n@set /p ANSWER=\r\n@echo GOT:%ANSWER%\r\n")
    injected = tmp_path / "injected"
    terminal = WindowsTerminal(str(wrapper), [f"value&echo injected>{injected}"])
    try:
        deadline = time.monotonic() + 10
        while "READY" not in terminal.poll() and time.monotonic() < deadline:
            pass
        terminal.outgoing.extend(b"hello\r")
        text = ""
        while "GOT:hello" not in text and time.monotonic() < deadline:
            text = terminal.poll()
        assert "GOT:hello" in text
        while terminal.process.poll() is None and time.monotonic() < deadline:
            terminal.poll()
        assert terminal.process.poll() is not None
        assert not injected.exists()
    finally:
        terminal.close()

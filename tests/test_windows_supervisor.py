from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from ctypes import wintypes
from pathlib import Path
from typing import cast

import pytest

from agentnave import windows_supervisor


@pytest.mark.skipif(os.name != "nt", reason="Windows Job Object and process handles")
def test_server_death_ends_supervisor_and_provider_descendants(tmp_path: Path) -> None:
    marker = tmp_path / "provider-pids.json"
    supervisor_pid = tmp_path / "supervisor.pid"
    provider = (
        "import json,os,subprocess,sys,time; "
        "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)']); "
        f"open({str(marker)!r}, 'w').write(json.dumps([os.getpid(), child.pid])); "
        "time.sleep(60)"
    )
    server = (
        "import subprocess,sys,time; "
        f"p = subprocess.Popen([sys.executable, '-I', {windows_supervisor.__file__!r}, "
        f"{str(tmp_path / 'status.json')!r}, {str(tmp_path / 'ready')!r}, "
        f"sys.executable, '-c', {provider!r}]); "
        f"open({str(supervisor_pid)!r}, 'w').write(str(p.pid)); "
        "time.sleep(60)"
    )
    kernel32 = windows_supervisor._configure_api()  # pyright: ignore[reportPrivateUsage]
    handles: list[wintypes.HANDLE] = []
    parent = subprocess.Popen([sys.executable, "-c", server])
    try:
        deadline = time.monotonic() + 10
        while not supervisor_pid.exists() or not supervisor_pid.read_text():
            assert time.monotonic() < deadline, "supervisor did not start"
            time.sleep(0.05)
        handle = cast(
            wintypes.HANDLE,
            kernel32.OpenProcess(0x00100001, False, int(supervisor_pid.read_text())),
        )
        assert handle
        handles.append(handle)
        while not marker.exists() or not marker.read_text():
            assert time.monotonic() < deadline, "provider did not start"
            time.sleep(0.05)
        for pid in json.loads(marker.read_text()):
            handle = cast(wintypes.HANDLE, kernel32.OpenProcess(0x00100001, False, pid))
            assert handle
            handles.append(handle)
        assert all(kernel32.WaitForSingleObject(handle, 0) == 258 for handle in handles)
        parent.kill()  # no Python cleanup in the service
        parent.wait(timeout=5)
        assert all(kernel32.WaitForSingleObject(handle, 8000) == 0 for handle in handles)
    finally:
        if parent.poll() is None:
            parent.kill()
        parent.wait(timeout=5)
        for handle in handles:
            kernel32.TerminateProcess(handle, 1)
            kernel32.CloseHandle(handle)

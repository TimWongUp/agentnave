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
    service_pids = tmp_path / "service-pids.json"
    provider = (
        "import json,os,subprocess,sys,time; "
        "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)']); "
        f"open({str(marker)!r}, 'w').write(json.dumps([os.getpid(), child.pid])); "
        "time.sleep(60)"
    )
    server = (
        "import json,os,subprocess,sys,time; "
        f"p = subprocess.Popen([sys.executable, '-I', {windows_supervisor.__file__!r}, "
        f"{str(tmp_path / 'status.json')!r}, {str(tmp_path / 'ready')!r}, "
        f"sys.executable, '-c', {provider!r}]); "
        f"open({str(service_pids)!r}, 'w').write(json.dumps([os.getpid(), p.pid])); "
        "time.sleep(60)"
    )
    kernel32 = windows_supervisor._configure_api()  # pyright: ignore[reportPrivateUsage]
    handles: list[wintypes.HANDLE] = []
    parent = subprocess.Popen([sys.executable, "-c", server])
    service: wintypes.HANDLE | None = None
    try:
        deadline = time.monotonic() + 10
        while not service_pids.exists() or not service_pids.read_text():
            assert time.monotonic() < deadline, "supervisor did not start"
            time.sleep(0.05)
        service_pid, supervisor_pid = json.loads(service_pids.read_text())
        service = cast(wintypes.HANDLE, kernel32.OpenProcess(0x00100001, False, service_pid))
        assert service
        handle = cast(wintypes.HANDLE, kernel32.OpenProcess(0x00100001, False, supervisor_pid))
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
        # sys.executable may be a venv redirector; terminate the actual service, not its launcher.
        assert kernel32.TerminateProcess(service, 1)
        parent.wait(timeout=5)
        outcomes = [kernel32.WaitForSingleObject(handle, 8000) for handle in handles]
        assert outcomes == [0, 0, 0], {
            "launcher_pid": parent.pid,
            "service_pid": service_pid,
            "supervisor_pid": supervisor_pid,
            "wait_outcomes": outcomes,
        }
    finally:
        if service:
            kernel32.TerminateProcess(service, 1)
            kernel32.CloseHandle(service)
        if parent.poll() is None:
            parent.kill()
        parent.wait(timeout=5)
        for handle in handles:
            kernel32.TerminateProcess(handle, 1)
            kernel32.CloseHandle(handle)

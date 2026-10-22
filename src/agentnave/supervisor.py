"""Dedicated POSIX process-group leader used by :mod:`agentnave.processes`."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time


def _hold_sigterm(_signum: int, _frame: object) -> None:
    return


def _restore_sigterm() -> None:
    signal.signal(signal.SIGTERM, signal.SIG_DFL)


def _write_status(status_fd: int, payload: dict[str, object]) -> None:
    data = (json.dumps(payload, ensure_ascii=False) + "\n").encode()
    try:
        os.write(status_fd, data[:16_384])
    finally:
        os.close(status_fd)


def _end_with_parent(parent: int) -> None:
    """Stop the whole group once the server is gone: nobody remains to collect or cancel it."""
    if os.name == "nt":
        return  # POSIX only; Windows relies on its kill-on-close Job Object.
    while os.getppid() == parent:
        time.sleep(1.0)
    group = os.getpgid(0)
    os.killpg(group, signal.SIGTERM)  # this supervisor holds SIGTERM; the provider does not
    time.sleep(2.0)
    os.killpg(group, signal.SIGKILL)


def main() -> int:
    if len(sys.argv) < 3:
        return 2
    status_fd = int(sys.argv[1])
    provider_argv = sys.argv[2:]
    signal.signal(signal.SIGTERM, _hold_sigterm)
    parent = os.getppid()
    os.environ["AGENTNAVE_SUPERVISOR"] = str(os.getpid())
    try:
        provider = subprocess.Popen(provider_argv, preexec_fn=_restore_sigterm)
    except OSError as exc:
        _write_status(status_fd, {"kind": "launch_error", "message": str(exc)[:512]})
    else:
        # Started after the fork: preexec_fn is unsafe in a multi-threaded process.
        threading.Thread(target=_end_with_parent, args=(parent,), daemon=True).start()
        _write_status(status_fd, {"kind": "exit", "returncode": provider.wait()})
    while True:
        signal.pause()  # pyright: ignore[reportAttributeAccessIssue,reportUnknownMemberType]


if __name__ == "__main__":
    raise SystemExit(main())

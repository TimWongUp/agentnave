"""Dedicated POSIX process-group leader used by :mod:`agentnave.processes`."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys


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


def main() -> int:
    if len(sys.argv) < 3:
        return 2
    status_fd = int(sys.argv[1])
    provider_argv = sys.argv[2:]
    signal.signal(signal.SIGTERM, _hold_sigterm)
    try:
        provider = subprocess.Popen(provider_argv, preexec_fn=_restore_sigterm)
    except OSError as exc:
        _write_status(status_fd, {"kind": "launch_error", "message": str(exc)[:512]})
    else:
        _write_status(status_fd, {"kind": "exit", "returncode": provider.wait()})
    while True:
        signal.pause()


if __name__ == "__main__":
    raise SystemExit(main())

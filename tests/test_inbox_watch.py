from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

from agentnave import inbox_watch
from agentnave.process_info import read_process_started


def _write_snapshot(
    root: Path,
    pid: int,
    started_at: float,
    bosses: dict[str, dict[str, object]],
) -> Path:
    inbox_dir = root / "inbox"
    inbox_dir.mkdir(parents=True, exist_ok=True)
    path = inbox_dir / f"{pid}.json"
    tmp = inbox_dir / f".{pid}.tmp"
    tmp.write_text(
        json.dumps(
            {"version": 1, "pid": pid, "started_at": started_at, "bosses": bosses},
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    os.replace(tmp, path)
    return path


def _run_watch(
    root: Path,
    pid: int,
    started_at: float,
    boss_key: str,
    *,
    timeout: float = 5.0,
    interval: float = 0.02,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-I",
            "-m",
            "agentnave.inbox_watch",
            str(root),
            str(pid),
            f"{started_at:.6f}",
            boss_key,
            "--timeout",
            str(timeout),
            "--interval",
            str(interval),
        ],
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )


def test_watch_ready_takes_priority_over_zero_in_flight_and_respects_waiting(
    tmp_path: Path,
) -> None:
    pid = os.getpid()
    started = read_process_started(pid)
    boss = "s-boss-ready"
    # While waiting=True with an unreceived event, watcher must not exit ready yet.
    _write_snapshot(
        tmp_path,
        pid,
        started,
        {
            boss: {
                "waiting": True,
                "in_flight": 0,
                "event_seq": 1,
                "delivered_seq": 0,
                "items": [
                    {"key": "task_end:1", "kind": "task_end", "role": None, "status": "succeeded"}
                ],
            }
        },
    )
    proc = subprocess.Popen(
        [
            sys.executable,
            "-I",
            "-m",
            "agentnave.inbox_watch",
            str(tmp_path),
            str(pid),
            f"{started:.6f}",
            boss,
            "--timeout",
            "5",
            "--interval",
            "0.02",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        time.sleep(0.1)
        assert proc.poll() is None
        # Once waiting clears, unreceived terminal event (in_flight=0) must return ready, not all_done.
        _write_snapshot(
            tmp_path,
            pid,
            started,
            {
                boss: {
                    "waiting": False,
                    "in_flight": 0,
                    "event_seq": 1,
                    "delivered_seq": 0,
                    "items": [
                        {
                            "key": "task_end:1",
                            "kind": "task_end",
                            "role": None,
                            "status": "succeeded",
                        }
                    ],
                }
            },
        )
        stdout, stderr = proc.communicate(timeout=5)
        assert proc.returncode == 0, stderr
        assert stdout == "ready\n"
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)


def test_watch_all_done_when_zero_in_flight_or_entry_removed(tmp_path: Path) -> None:
    pid = os.getpid()
    started = read_process_started(pid)
    boss = "s-boss-done"
    _write_snapshot(
        tmp_path,
        pid,
        started,
        {
            boss: {
                "waiting": False,
                "in_flight": 0,
                "event_seq": 2,
                "delivered_seq": 2,
                "items": [],
            }
        },
    )
    result = _run_watch(tmp_path, pid, started, boss)
    assert result.returncode == 0, result.stderr
    assert result.stdout == "all_done\n"

    # Also all_done when snapshot file is absent while the service process is still alive.
    (tmp_path / "inbox" / f"{pid}.json").unlink()
    missing = _run_watch(tmp_path, pid, started, boss)
    assert missing.returncode == 0, missing.stderr
    assert missing.stdout == "all_done\n"


def test_watch_service_gone_on_wrong_start_time_or_dead_pid(tmp_path: Path) -> None:
    pid = os.getpid()
    started = read_process_started(pid)
    boss = "s-boss-gone"
    _write_snapshot(
        tmp_path,
        pid,
        started,
        {boss: {"waiting": False, "in_flight": 1, "event_seq": 0, "delivered_seq": 0, "items": []}},
    )
    # Wrong started_at simulates PID reuse.
    reused = _run_watch(tmp_path, pid, started - 30.0, boss)
    assert reused.returncode == 0, reused.stderr
    assert reused.stdout == "service_gone\n"

    # A child that already exited has a dead PID.
    exited = subprocess.run([sys.executable, "-c", "pass"], check=True)
    assert exited.returncode == 0
    dead = _run_watch(tmp_path, 999_999_999, started, boss)
    assert dead.returncode == 0, dead.stderr
    assert dead.stdout == "service_gone\n"


def test_watch_expired_after_timeout(tmp_path: Path) -> None:
    pid = os.getpid()
    started = read_process_started(pid)
    boss = "s-boss-expired"
    _write_snapshot(
        tmp_path,
        pid,
        started,
        {boss: {"waiting": False, "in_flight": 1, "event_seq": 0, "delivered_seq": 0, "items": []}},
    )
    result = _run_watch(tmp_path, pid, started, boss, timeout=0.08, interval=0.02)
    assert result.returncode == 0, result.stderr
    assert result.stdout == "expired\n"


def test_watch_already_watching_locks_per_boss_and_releases_on_exit(tmp_path: Path) -> None:
    pid = os.getpid()
    started = read_process_started(pid)
    boss = "s-boss-lock"
    _write_snapshot(
        tmp_path,
        pid,
        started,
        {boss: {"waiting": False, "in_flight": 1, "event_seq": 0, "delivered_seq": 0, "items": []}},
    )
    first = subprocess.Popen(
        [
            sys.executable,
            "-I",
            "-m",
            "agentnave.inbox_watch",
            str(tmp_path),
            str(pid),
            f"{started:.6f}",
            boss,
            "--timeout",
            "5",
            "--interval",
            "0.02",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        lock_file_path = inbox_watch._lock_path(tmp_path, boss)  # pyright: ignore[reportPrivateUsage]
        deadline = time.monotonic() + 5.0
        while not lock_file_path.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        # Give the first watcher a moment to acquire the OS lock after creating the file.
        second_out = ""
        for _ in range(20):
            second = _run_watch(tmp_path, pid, started, boss, timeout=0.05, interval=0.01)
            if second.stdout == "already_watching\n":
                second_out = second.stdout
                break
            time.sleep(0.02)
        assert second_out == "already_watching\n"

        # Finish the first watcher and verify a third watcher can now acquire the lock.
        _write_snapshot(
            tmp_path,
            pid,
            started,
            {
                boss: {
                    "waiting": False,
                    "in_flight": 0,
                    "event_seq": 1,
                    "delivered_seq": 0,
                    "items": [
                        {
                            "key": "task_end:1",
                            "kind": "task_end",
                            "role": None,
                            "status": "succeeded",
                        }
                    ],
                }
            },
        )
        stdout, stderr = first.communicate(timeout=5)
        assert first.returncode == 0, stderr
        assert stdout == "ready\n"
    finally:
        if first.poll() is None:
            first.terminate()
            first.wait(timeout=5)

    # After release, a new watcher succeeds immediately.
    _write_snapshot(tmp_path, pid, started, {})
    third = _run_watch(tmp_path, pid, started, boss)
    assert third.returncode == 0, third.stderr
    assert third.stdout == "all_done\n"

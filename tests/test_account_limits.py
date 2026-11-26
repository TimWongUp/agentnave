"""Per-account concurrency limits (ADR-0127): settings, buckets and cross-process slots."""

from __future__ import annotations

import json
import subprocess
import sys
from collections.abc import Generator
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from pathlib import Path
from typing import get_args

import pytest

from agentnave import account_limits, accounts
from agentnave.account_limits import (
    AccountCapacityError,
    acquire_slot,
    acquire_slots,
    claim_known_slots,
    full_accounts,
)
from agentnave.members import (
    CapacityError,
    ProviderName,
    SettingsFileError,
    data_root,
    read_account_concurrencies,
    read_account_concurrency,
)


def write_settings(**values: object) -> None:
    root = data_root()
    root.mkdir(parents=True, exist_ok=True)
    (root / "settings.json").write_text(json.dumps({"version": 1, **values}))


def register(root: Path, accounts_: dict[str, Path]) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "accounts.json").write_text(
        json.dumps(
            {
                "version": 1,
                "accounts": [
                    {
                        "id": id_,
                        "provider": "claude",
                        "label": id_,
                        "dir": str(path),
                        "managed": False,
                    }
                    for id_, path in accounts_.items()
                ],
            }
        )
    )


def slot_files(provider: str, bucket: str = "default") -> list[Path]:
    directory = data_root() / "account-slots" / provider / bucket
    return [path for path in directory.glob("*.lock") if path.name != "gate.lock"]


def test_limit_defaults_to_five_and_null_means_unlimited() -> None:
    expected_defaults = {
        provider: (5 if provider in ("codex", "antigravity", "claude") else None)
        for provider in get_args(ProviderName.__value__)
    }
    assert read_account_concurrencies() == expected_defaults
    write_settings(account_concurrency={"claude": None, "codex": 1, "grok": 10, "future": 2})
    limits = read_account_concurrencies()
    assert limits["claude"] is None and read_account_concurrency("claude") is None
    assert (limits["codex"], limits["grok"], limits["antigravity"], limits["pi"]) == (
        1,
        10,
        5,
        None,
    )
    assert "future" not in limits


@pytest.mark.parametrize("value", [0, 11, True, 5.0, "5", [], {}])
def test_an_invalid_limit_refuses_the_whole_file(value: object) -> None:
    write_settings(account_concurrency={"codex": value})
    with pytest.raises(SettingsFileError, match="account_concurrency"):
        read_account_concurrencies()
    # Choosing and listing stay usable; the start that follows reports the file.
    assert full_accounts([("codex", None)]) == {}
    with pytest.raises(SettingsFileError):
        acquire_slot("codex", None)


@pytest.mark.parametrize("value", [None, 5, "claude", []])
def test_the_limit_map_must_be_an_object(value: object) -> None:
    write_settings(account_concurrency=value)
    with pytest.raises(SettingsFileError):
        read_account_concurrency("codex")


def test_buckets_follow_the_account_a_run_really_uses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bucket = accounts.concurrency_bucket
    # A CLI without an account directory has one default account.
    assert bucket("codex", None) == "default"
    work = tmp_path / "work"
    register(data_root(), {"work": work})
    assert bucket("claude", "work") == "work"
    with pytest.raises(ValueError, match="not registered"):
        bucket("claude", "gone")
    inherited = bucket("claude", None)
    assert inherited.startswith("default-") and len(inherited) == len("default-") + 12
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "other"))
    assert bucket("claude", None) not in (inherited, "work")
    # The same directory, however it is spelled, is the registered account's.
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", f"{work}/")
    assert bucket("claude", None) == "work"
    (data_root() / "accounts.json").write_text("{")
    assert bucket("claude", None).startswith("default-")


def test_a_full_account_refuses_and_release_is_idempotent() -> None:
    write_settings(account_concurrency={"codex": 1, "grok": 1})
    claim = acquire_slot("codex", None)
    with pytest.raises(AccountCapacityError, match="codex default account allows 1"):
        acquire_slot("codex", None)
    assert full_accounts([("codex", None), ("grok", None)]) == {("codex", None): 1}
    other = acquire_slot("grok", None)  # another CLI is counted apart
    claim.release()
    again = acquire_slot("codex", None)
    claim.release()  # a repeated release leaves the later holder alone
    assert len(slot_files("codex")) == 1
    again.release()
    other.release()
    assert slot_files("codex") == []
    assert (data_root() / "account-slots" / "codex" / "default" / "gate.lock").is_file()


def test_named_and_inherited_runs_of_one_account_share_its_slots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write_settings(account_concurrency={"claude": 1})
    register(data_root(), {"work": tmp_path / "work", "home": tmp_path / "home"})
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "work"))
    claim = acquire_slot("claude", None)
    with pytest.raises(AccountCapacityError, match="account 'work'"):
        acquire_slot("claude", "work")
    acquire_slot("claude", "home").release()
    claim.release()


def test_an_unlimited_cli_makes_no_slot() -> None:
    write_settings(account_concurrency={"codex": None})
    claims = acquire_slots([("codex", None)] * 40)
    assert len(claims) == 40
    assert full_accounts([("codex", None)]) == {}
    assert not (data_root() / "account-slots").exists()


def test_looking_creates_nothing() -> None:
    assert full_accounts(account_limits.known_targets()) == {}
    assert not (data_root() / "account-slots").exists()


def test_a_batch_takes_every_slot_or_none() -> None:
    write_settings(account_concurrency={"codex": 2, "grok": 1})
    with pytest.raises(AccountCapacityError, match="grok default account allows 1"):
        acquire_slots([("codex", None), ("grok", None), ("grok", None)])
    assert slot_files("codex") == [] and slot_files("grok") == []
    held = acquire_slot("grok", None)
    with pytest.raises(AccountCapacityError, match="1 are running and this would start 1"):
        acquire_slots([("codex", None), ("grok", None)])
    assert slot_files("codex") == []
    held.release()
    claims = acquire_slots([("codex", None), ("grok", None), ("codex", None)])
    assert [claim.provider for claim in claims] == ["codex", "grok", "codex"]
    for claim in claims:
        claim.release()


def test_a_failure_while_creating_slots_returns_the_created_ones(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    write_settings(account_concurrency={"codex": 5, "grok": 5})
    hold = account_limits._hold  # pyright: ignore[reportPrivateUsage]

    def fail_grok(provider: str, bucket: str) -> account_limits.AccountSlotClaim:
        if provider == "grok":
            raise OSError("disk full")
        return hold(provider, bucket)

    monkeypatch.setattr(account_limits, "_hold", fail_grok)
    with pytest.raises(OSError, match="disk full"):
        acquire_slots([("codex", None), ("grok", None)])
    assert slot_files("codex") == []


def test_seats_of_unregistered_accounts_are_left_to_their_own_launch() -> None:
    claims = claim_known_slots([("claude", "gone"), ("codex", None)])
    assert claims[0] is None and claims[1] is not None
    claims[1].release()


def test_opposite_batches_do_not_deadlock() -> None:
    write_settings(account_concurrency={"codex": 10, "grok": 10})

    def take(order: list[tuple[str, str | None]]) -> None:
        for _ in range(20):
            for claim in acquire_slots(order):
                claim.release()

    items: list[tuple[str, str | None]] = [("codex", None), ("grok", None)]
    with ThreadPoolExecutor(2) as pool:
        for done in [pool.submit(take, items), pool.submit(take, items[::-1])]:
            done.result(timeout=60)


def test_a_busy_gate_gives_up_without_a_slot(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(account_limits, "_GATE_TIMEOUT_SECONDS", 0.05)
    acquire_slot("codex", None).release()
    directory = data_root() / "account-slots" / "codex" / "default"
    with (
        account_limits._gate(directory),  # pyright: ignore[reportPrivateUsage]
        pytest.raises(CapacityError, match="gate is busy"),
    ):
        acquire_slot("codex", None)
    assert slot_files("codex") == []


_HOLDER = """
import sys
from agentnave.account_limits import AccountCapacityError, acquire_slot
try:
    claim = acquire_slot("codex", None)
except AccountCapacityError:
    print("full", flush=True)
else:
    print("held", flush=True)
    sys.stdin.read()
"""


@contextmanager
def other_server() -> Generator[tuple[subprocess.Popen[str], str]]:
    """Another process taking a codex slot of the same data root; it keeps it until killed."""
    process = subprocess.Popen(
        [sys.executable, "-c", _HOLDER], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    )
    assert process.stdout is not None and process.stdin is not None
    try:
        yield process, process.stdout.readline().strip()
    finally:
        process.kill()
        process.wait(timeout=30)
        process.stdin.close()
        process.stdout.close()


def test_slots_count_across_processes_and_a_dead_one_gives_them_back() -> None:
    write_settings(account_concurrency={"codex": 1})
    own = acquire_slot("codex", None)
    with other_server() as (_, answer):
        assert answer == "full"
    own.release()
    with other_server() as (process, answer):
        assert answer == "held"
        with pytest.raises(AccountCapacityError):
            acquire_slot("codex", None)
        process.kill()  # no cleanup of its own: the kernel drops the lock
        process.wait(timeout=30)
        [stale] = slot_files("codex")
        assert full_accounts([("codex", None)]) == {}
        assert not stale.exists()
    acquire_slot("codex", None).release()

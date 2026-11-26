"""Per-account concurrency limits across every server of one data root (ADR-0127).

Each running turn holds one OS-locked file in account-slots/<provider>/<bucket>/, so a server
that exits or crashes gives its slots back without any cleanup. The bucket's gate lock covers
counting the live slots and adding new ones; it is never removed.
"""

from __future__ import annotations

import logging
import time
from collections import Counter
from collections.abc import Generator, Iterable, Sequence
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import IO, get_args
from uuid import uuid4

from agentnave import accounts
from agentnave.locks import lock_file, release_and_remove
from agentnave.members import (
    CapacityError,
    ProviderName,
    data_root,
    read_account_concurrencies,
)

# Not under locks/: history retention sweeps that directory.
SLOTS_DIR_NAME = "account-slots"
_GATE_NAME = "gate.lock"
_GATE_RETRY_SECONDS = 0.025
_GATE_TIMEOUT_SECONDS = 5.0

type Target = tuple[str, str | None]  # a CLI and a registered account, None for the default one


def account_name(provider: str, account: str | None) -> str:
    return f"{provider} {'default account' if account is None else f'account {account!r}'}"


def full_reason(provider: str, account: str | None, limit: int) -> str:
    return f"{account_name(provider, account)} is at its per-account concurrency limit of {limit}"


class AccountCapacityError(CapacityError):
    """Starting would run more turns on one account than its CLI's limit allows."""

    def __init__(
        self, provider: str, account: str | None, limit: int, *, running: int, needed: int = 1
    ) -> None:
        super().__init__(
            f"{account_name(provider, account)} allows {limit} concurrent runs across all "
            f"AgentNave servers (per-account concurrency limit); {running} are running and this "
            f"would start {needed}. Wait for some to finish, or use another account or CLI"
        )


@dataclass(slots=True)
class AccountSlotClaim:
    """One run's slot; without a handle the account has no limit and nothing is held."""

    provider: str
    bucket: str
    _handle: IO[bytes] | None = None
    _path: Path | None = None

    def release(self) -> None:
        if self._handle is None or self._path is None:
            return
        try:
            release_and_remove(self._handle, self._path)
        except OSError:
            # The handle is closed either way; the next count removes a file left behind.
            logging.getLogger(__name__).exception("Could not remove an account slot")


def _directory(provider: str, bucket: str) -> Path:
    return data_root() / SLOTS_DIR_NAME / provider / bucket


@contextmanager
def _gate(directory: Path) -> Generator[None]:
    """Serialize counting and adding slots of one bucket across processes."""
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / _GATE_NAME
    deadline = time.monotonic() + _GATE_TIMEOUT_SECONDS
    while True:
        try:
            handle = lock_file(path, f"Account slot gate is busy: {path}")
            break
        except ValueError as exc:
            if time.monotonic() >= deadline:
                raise CapacityError(f"{exc}; retry") from exc
            time.sleep(_GATE_RETRY_SECONDS)
    try:
        yield
    finally:
        # Never unlink the gate, so every waiter locks the same inode.
        handle.close()


def _running(directory: Path) -> int:
    """Slots some process still holds, removing the others; call under the bucket's gate."""
    held = 0
    for entry in directory.glob("*.lock"):
        if entry.name == _GATE_NAME:
            continue
        try:
            handle = lock_file(entry, "Account slot is held", create=False)
        except FileNotFoundError:
            continue  # released meanwhile
        except (OSError, ValueError):
            held += 1
            continue
        release_and_remove(handle, entry)
    return held


def _hold(provider: str, bucket: str) -> AccountSlotClaim:
    path = _directory(provider, bucket) / f"{uuid4().hex}.lock"
    return AccountSlotClaim(provider, bucket, lock_file(path, "Account slot collision"), path)


def acquire_slots(items: Sequence[Target]) -> list[AccountSlotClaim]:
    """One claim for each item, in order, all or none; nothing waits for a slot to free.

    Raises AccountCapacityError, or ValueError for an account that is not registered.
    """
    limits = read_account_concurrencies()
    keys = [(item[0], accounts.concurrency_bucket(*item)) for item in items]
    needed = Counter(key for key in keys if limits.get(key[0]) is not None)
    # One order for every caller, so two batches never wait on each other's gates.
    ordered = sorted(needed)
    with ExitStack() as gates:
        for key in ordered:
            gates.enter_context(_gate(_directory(*key)))
        for key in ordered:
            limit = limits[key[0]]
            assert limit is not None
            running = _running(_directory(*key))
            if running + needed[key] > limit:
                provider, account = items[keys.index(key)]
                raise AccountCapacityError(
                    provider, account, limit, running=running, needed=needed[key]
                )
        claims: list[AccountSlotClaim] = []
        try:
            for key in keys:
                claims.append(_hold(*key) if key in needed else AccountSlotClaim(*key))
        except BaseException:
            for claim in claims:
                claim.release()
            raise
    return claims


def acquire_slot(provider: str, account: str | None) -> AccountSlotClaim:
    return acquire_slots([(provider, account)])[0]


def claim_known_slots(items: Sequence[Target]) -> list[AccountSlotClaim | None]:
    """acquire_slots for meeting seats: a seat whose account is no longer registered gets None
    and fails at its own launch, without refusing the other seats of the round."""
    known = [item for item in items if _resolves(*item)]
    claims = iter(acquire_slots(known))
    return [next(claims) if item in known else None for item in items]


def _resolves(provider: str, account: str | None) -> bool:
    try:
        accounts.resolve(provider, account)
    except ValueError:
        return False
    return True


def known_targets() -> list[Target]:
    """Every CLI's default account and every registered one."""
    targets: list[Target] = [(provider, None) for provider in get_args(ProviderName.__value__)]
    try:
        listed = accounts.listing()
    except accounts.AccountsFileError:
        return targets
    return targets + [(found.provider, found.id) for found in listed]


def full_accounts(candidates: Iterable[Target]) -> dict[Target, int]:
    """The candidates at their limit now, with that limit; one count per bucket.

    A snapshot for choosing and listing: acquire_slots decides. Whatever cannot be read counts
    as not full, and the start that follows reports it.
    """
    try:
        limits = read_account_concurrencies()
    except ValueError:
        return {}
    counted: dict[tuple[str, str], int] = {}
    full: dict[Target, int] = {}
    for provider, account in dict.fromkeys(candidates):
        limit = limits.get(provider)
        if limit is None:
            continue
        try:
            key = (provider, accounts.concurrency_bucket(provider, account))
            if key not in counted:
                counted[key] = _count(_directory(*key))
        except (OSError, ValueError):
            continue
        if counted[key] >= limit:
            full[(provider, account)] = limit
    return full


def _count(directory: Path) -> int:
    if not directory.is_dir():
        return 0  # never used: choosing creates nothing
    with _gate(directory):
        return _running(directory)


def check_account(provider: str, account: str | None) -> None:
    """Refuse early what acquire_slot would refuse now, before costly preparation."""
    if limit := full_accounts([(provider, account)]).get((provider, account)):
        raise AccountCapacityError(provider, account, limit, running=limit)


__all__ = [
    "SLOTS_DIR_NAME",
    "AccountCapacityError",
    "AccountSlotClaim",
    "Target",
    "account_name",
    "acquire_slot",
    "acquire_slots",
    "check_account",
    "claim_known_slots",
    "full_accounts",
    "full_reason",
    "known_targets",
]

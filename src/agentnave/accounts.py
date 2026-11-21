"""Named CLI accounts (ADR-0108): the App-owned accounts.json and what an account launches with.

The default account is not in the file: it starts the CLI with the inherited environment and this
module never reads the file for it. A named account starts it with the registered configuration
directory in the Adapter's account environment variable, passed to the child only. Contact
details of an account (email, organization) live in the App and never reach this module.
"""

from __future__ import annotations

import os
import unicodedata
from collections.abc import Iterable
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import TYPE_CHECKING, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from agentnave.models import ACCOUNT_ID

if TYPE_CHECKING:
    from agentnave.adapters.base import ProviderAdapter
    from agentnave.host import Host

# The CLIs that take named accounts; each also declares its account environment variable.
ACCOUNT_PROVIDERS = frozenset({"claude"})
_LABEL_MAX_LENGTH = 40


class AccountsFileError(ValueError):
    """The App-owned accounts.json cannot be read or validated."""


def _absolute(value: str) -> bool:
    # Either platform's rule, so a file the App wrote on one system is read the same on any.
    return not value.startswith("~") and (
        PurePosixPath(value).is_absolute() or PureWindowsPath(value).is_absolute()
    )


def normal_dir(value: str | Path) -> str:
    """A directory as accounts are compared by: normalized, case-folded where paths are."""
    return os.path.normcase(os.path.normpath(str(value)))


class Account(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)

    id: str = Field(pattern=ACCOUNT_ID)
    provider: Literal["claude"]
    label: str
    dir: str
    managed: bool

    @field_validator("label")
    @classmethod
    def short_label(cls, label: str) -> str:
        label = label.strip()
        if not 1 <= len(label) <= _LABEL_MAX_LENGTH or any(
            unicodedata.category(char) == "Cc" for char in label
        ):
            raise ValueError(
                f"label must be 1–{_LABEL_MAX_LENGTH} characters without control characters"
            )
        return label

    @field_validator("dir")
    @classmethod
    def absolute_dir(cls, value: str) -> str:
        if not _absolute(value):
            raise ValueError("dir must be an absolute path, not starting with ~")
        return value


class AccountsFile(BaseModel):
    """accounts.json: written by the App, read here; a hand-written copy on Linux."""

    model_config = ConfigDict(extra="forbid", strict=True)

    version: int = Field(ge=1, le=1)
    accounts: list[Account]

    @model_validator(mode="after")
    def unique(self) -> AccountsFile:
        if len({account.id for account in self.accounts}) != len(self.accounts):
            raise ValueError("account ids must be unique")
        if len({normal_dir(account.dir) for account in self.accounts}) != len(self.accounts):
            raise ValueError("account directories must be unique")
        return self


class AccountListing(BaseModel):
    """What a boss sees of a registered account: enough to name it in a dispatch."""

    model_config = ConfigDict(extra="forbid", strict=True)

    id: str
    provider: str
    label: str


def check_choice(provider: str, account: str | None) -> None:
    """The file-level rule for a seat choice: only a CLI that takes accounts may name one."""
    if account is not None and provider not in ACCOUNT_PROVIDERS:
        raise ValueError(f"{provider} does not take an account")


def read_accounts() -> AccountsFile | None:
    """accounts.json; None when missing, which means no named account."""
    from agentnave.members import _read_app_file  # pyright: ignore[reportPrivateUsage]

    return _read_app_file("accounts.json", AccountsFile, AccountsFileError)


def listing() -> list[AccountListing]:
    """The registered accounts without directories or anything the App shows only locally."""
    document = read_accounts()
    return [
        AccountListing(id=found.id, provider=found.provider, label=found.label)
        for found in (document.accounts if document is not None else ())
    ]


def resolve(provider: str, account: str | None) -> Account | None:
    """The registered account a dispatch names; None for the default account, which reads nothing."""
    if account is None:
        return None
    check_choice(provider, account)
    document = read_accounts()
    for found in document.accounts if document is not None else ():
        if found.id == account and found.provider == provider:
            return found
    raise ValueError(
        f"Account {account!r} is not registered for {provider} in accounts.json; add it in the "
        "AgentNave App or choose another account, or omit account for the default one"
    )


def env_for(adapter: ProviderAdapter, account: str | None) -> dict[str, str]:
    """The environment a named account adds to the child; nothing for the default account."""
    if account is None:
        return {}
    if adapter.account_env is None:
        raise ValueError(f"{adapter.name} does not take an account")
    found = resolve(adapter.name, account)
    assert found is not None
    return {adapter.account_env: found.dir}


def home_of(adapter: ProviderAdapter, account: str | None) -> str | None:
    """The resolved configuration directory an account's sessions live in, for comparing
    ownership; the default account's is what the inherited environment or the CLI default gives."""
    if adapter.account_env is None:
        return None
    if account is not None:
        found = resolve(adapter.name, account)
        assert found is not None
        return normal_dir(found.dir)
    inherited = os.environ.get(adapter.account_env, "").strip()
    return normal_dir(
        os.path.abspath(inherited) if inherited else os.path.expanduser(adapter.account_home)
    )


def same_home(adapter: ProviderAdapter, first: str | None, second: str | None) -> bool:
    """Whether two accounts keep their native sessions in one directory; an account that is no
    longer registered counts only as itself."""
    if first == second:
        return True
    try:
        return home_of(adapter, first) == home_of(adapter, second)
    except ValueError:
        return False


def host_account(host: Host) -> str | None:
    """The account the host's own CLI runs as: the registered one whose directory is the
    service's inherited one, else None (the default account)."""
    from agentnave.adapters import get_adapter

    if host.provider is None:
        return None
    adapter = get_adapter(host.provider)
    try:
        inherited = home_of(adapter, None)
        document = read_accounts()
    except ValueError:
        return None
    for found in document.accounts if document is not None else ():
        if found.provider == adapter.name and normal_dir(found.dir) == inherited:
            return found.id
    return None


def is_host_account(host: Host, provider: str, account: str | None) -> bool:
    """Whether a seat's CLI and account are the host's own: the same CLI, and the default account
    (which inherits whatever the host runs as) or the host's registered one."""
    return (
        provider == host.provider
        and host.provider is not None
        and (account is None or account == host_account(host))
    )


def connection_warnings(host: Host) -> list[str]:
    """Why the host's connection line may name another account than the one it runs as."""
    app_id = host.app_id or ""
    provider, _, named = app_id.partition("@")
    if not named or provider != host.provider:
        return []
    try:
        document = read_accounts()
    except ValueError:
        return []
    # Only a registered account id is told apart from the folder names of older split rows.
    if not any(found.id == named for found in (document.accounts if document else ())):
        return []
    actual = host_account(host)
    if actual == named:
        return []
    return [
        f"This connection is set up for account {named!r}, but the service runs as "
        f"{'account ' + repr(actual) if actual else 'the default account'} (decided by "
        "the inherited configuration directory); repair the connection in the AgentNave App"
    ]


def dispatch_warnings(host: Host, targets: Iterable[tuple[str, str | None]]) -> list[str]:
    """What a dispatch receipt tells about the accounts it starts: a login override in the
    environment (named accounts only; the default account is unchanged) and a connection line
    naming another account than the one the service runs as."""
    from agentnave.adapters import get_adapter

    found: list[str] = []
    providers = dict.fromkeys(targets)
    for provider, account in providers:
        if account is not None:
            found.extend(
                warning
                for warning in get_adapter(provider).account_warnings(os.environ)
                if warning not in found
            )
    if any(provider == host.provider for provider, _ in providers):
        found.extend(connection_warnings(host))
    return found


def label_of(account: str | None) -> str | None:
    """The display name of a registered account, or its id when it is gone or the file is broken."""
    if account is None:
        return None
    try:
        document = read_accounts()
    except ValueError:
        return account
    for found in document.accounts if document is not None else ():
        if found.id == account:
            return found.label
    return account

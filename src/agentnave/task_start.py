"""Resolve private-task arguments before preparing or locking a native session."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Literal, cast, get_args

from agentnave.host import Host
from agentnave.members import (
    OrchestrationRole,
    ProviderName,
    ReviewRole,
    apply_member,
    read_cli_options,
    read_members,
    read_permissions,
    role_seat,
)
from agentnave.models import InvocationRequest, ProviderOption
from agentnave.store import Record, TaskRecord

if TYPE_CHECKING:
    from agentnave.conversations import Conversations
    from agentnave.core import InvocationManager


READ_ONLY_APPLIED = "read-only instruction (access=read)"


def latest_session_task(
    records: Iterable[Record],
    session_id: str,
    provider: ProviderName | None,
    manager_session: str | None,
    not_yours: Callable[[str, str | None], str],
    *,
    explicit_identity: bool,
) -> TaskRecord | None:
    """Only the owning boss's history may supply defaults or identify a provider."""
    matches = [
        (record.state, index, task)
        for record in records
        if record.state.kind == "task"
        for index, task in enumerate(record.state.tasks)
        if task.session_id == session_id
    ]
    owned = [
        (state, index, task)
        for state, index, task in matches
        if state.manager_session == manager_session
        or (manager_session is None and not (state.manager_session or "").startswith("s-"))
    ]
    candidates = [item for item in owned if provider is None or item[2].provider == provider]
    if not candidates:
        foreign = next(
            (state for state, _, task in matches if provider is None or task.provider == provider),
            None,
        )
        if foreign is not None:
            if explicit_identity:
                return None  # Preserve explicit cross-boss resume without borrowing its metadata.
            raise ValueError(not_yours("Task conversation", foreign.manager_session))
        if owned:
            raise ValueError("A native session cannot change provider; use its original provider")
        return None
    if len({task.provider for _, _, task in candidates}) > 1:
        raise ValueError("session_id matches multiple providers; pass provider and cwd explicitly")
    return max(
        candidates,
        key=lambda item: (item[2].time, item[0].created, item[0].id, item[1]),
    )[2]


def role_access(role: str | None) -> Literal["read", "write"] | None:
    """The access a known duty carries: Explorers and reviewers read, Workers write."""
    name = (role or "").strip()
    if name in get_args(OrchestrationRole.__value__):
        return "write" if name.startswith("worker-") else "read"
    return "read" if name in get_args(ReviewRole.__value__) else None


@dataclass
class StartArguments:
    provider: ProviderName | None
    cwd: str | None
    role: str | None
    seat: OrchestrationRole | None
    access: Literal["read", "write"] | None
    defaults: list[str] = field(default_factory=lambda: [])

    def inherit(self, task: TaskRecord) -> None:
        self.provider = self._inherited(
            self.provider, cast(ProviderName, task.provider), "provider"
        )
        self.cwd = self._inherited(self.cwd, task.cwd, "cwd")
        role = (self.role or "").strip()
        self.role = self._inherited(self.role, task.role, "role")
        # A new role is a new duty: the old seat would otherwise outrank it. Blank only clears the label.
        if self.seat is not None or role in ("", task.role):
            self.seat = self._inherited(
                self.seat, cast(OrchestrationRole | None, task.seat), "seat"
            )

    def _inherited[T](self, explicit: T | None, saved: T | None, key: str) -> T | None:
        if explicit is not None or saved is None:
            return explicit
        self.defaults.append(f"{key} (session_id history)")
        return saved

    def resolve_seat(self, host: Host) -> None:
        name = (self.seat or self.role or "").strip()
        if self.provider is None:
            if not name:
                raise ValueError("Pass provider, or seat / a review role to select an App seat")
            seat = role_seat(name, host, required=True)
            assert seat is not None
            if seat.provider == host.provider:
                raise ValueError(
                    f"{name} seat is native to this host; use the host's built-in subagent"
                )
            self.provider = seat.provider
            self.defaults.append(f"provider ({name} seat)")
        implied = role_access(name)
        if implied is None:
            return
        if self.access is None:
            self.access = implied
            self.defaults.append(f"access ({name} seat)")
        elif self.access != implied:
            raise ValueError(
                f"{name} declares access={implied}; omit access or pick a matching seat"
            )


def resolve_start(
    arguments: StartArguments,
    conversations: Conversations,
    host: Host,
    session_id: str | None,
    manager_session_id: str | None,
) -> StartArguments:
    if session_id is not None:
        task = conversations.resume_task(
            session_id,
            arguments.provider,
            manager_session_id,
            explicit_identity=arguments.provider is not None and arguments.cwd is not None,
        )
        if task is not None:
            arguments.inherit(task)
        elif arguments.provider is None or arguments.cwd is None:
            raise ValueError("Unknown session_id; pass provider and cwd explicitly")
    if arguments.cwd is None:
        raise ValueError("cwd is required for a new task; pass an absolute existing directory")
    arguments.resolve_seat(host)
    if arguments.access == "read":
        arguments.defaults.append(READ_ONLY_APPLIED)
    return arguments


def apply_task_defaults(
    provider: ProviderName,
    seat_name: str | None,
    options: dict[str, ProviderOption],
    seat_applied: list[str],
    excluded: frozenset[str],
) -> tuple[dict[str, ProviderOption], list[str]]:
    options, applied = apply_member(
        read_members(),
        "task",
        provider,
        options,
        excluded,
        permissions=read_permissions(),
        cli_options=read_cli_options(),
        seat=frozenset(seat_applied),
    )
    if seat_applied:
        applied = [f"{(seat_name or '').strip()} seat", *seat_applied, *applied]
    return options, applied


def add_write_warning(
    warnings: list[str],
    manager: InvocationManager,
    request: InvocationRequest,
    invocation_id: str,
    access: Literal["read", "write"] | None,
) -> None:
    if access == "read":
        return
    sharing = [
        listing.invocation_id
        for listing in manager.list_invocations()
        if listing.status is None
        and listing.cwd == request.cwd
        and listing.invocation_id != invocation_id
    ]
    if sharing:
        warnings.append(
            f"{len(sharing)} other CLI run(s) active in this cwd ({', '.join(sharing)}); "
            "parallel writers need separate worktrees. Declare access=read for read-only work."
        )

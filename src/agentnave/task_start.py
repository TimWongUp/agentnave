"""Resolve private-task arguments before preparing or locking a native session."""

from __future__ import annotations

from collections.abc import Collection, Iterable, Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING, Literal, cast, get_args

from agentnave import accounts, provider_health
from agentnave.adapters import get_adapter
from agentnave.decision_seats import custom_seat, random_candidates_enabled
from agentnave.host import Host
from agentnave.host_choice import host_cli_mode
from agentnave.isolated import other_active_leases, shares_cwd_tree
from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.members import (
    ProviderName,
    ReviewRole,
    apply_member,
    apply_seat,
    data_root,
    orchestration_enabled,
    read_members,
    read_permissions,
    role_seat,
    validate_task_seat,
)
from agentnave.models import InvocationRequest, ProviderOption
from agentnave.orchestration_roles import IMAGE_KIND, CustomSeat, ReviewSeat, seat_role
from agentnave.seat_fallbacks import IMAGE_HOST_CLI_REASON
from agentnave.store import Record, TaskRecord

if TYPE_CHECKING:
    from agentnave.conversations import Conversations
    from agentnave.core import InvocationManager


READ_ONLY_APPLIED = "read-only instruction (access=read)"


def _foreign_session_error(session_id: str, workspace: str | None) -> ValueError:
    where = f"workspace {workspace}" if workspace else "another manager session"
    return ValueError(
        f"Native session {session_id} belongs to {where} (a different manager session) and "
        "cannot be resumed across workspaces; omit session_id to start a new thread, and use "
        "forward (or read_conversation) to carry prior results"
    )


type _BindingOrder = tuple[tuple[int, int, str], tuple[int, int, str], str, int, int, str]


def _timestamp_key(raw: str | None) -> tuple[int, int, str]:
    if not raw:
        return (1, 0, "")
    try:
        parsed = datetime.fromisoformat(raw)
        if parsed.tzinfo is None:
            parsed = parsed.replace(tzinfo=UTC)
        # Offsets at the year bounds overflow; such a time ranks like an unparsable one.
        delta = parsed.astimezone(UTC) - datetime(1970, 1, 1, tzinfo=UTC)
    except (ValueError, OverflowError):
        return (1, 0, raw)
    us = (delta.days * 86400 + delta.seconds) * 1_000_000 + delta.microseconds
    return (0, us, raw)


def _session_bindings(
    records: Iterable[Record],
    session_id: str,
    provider: ProviderName | None,
) -> list[tuple[str, _BindingOrder, str | None]]:
    bindings: list[tuple[str, _BindingOrder, str | None]] = []
    for record in records:
        state = record.state
        created_key = _timestamp_key(state.created)
        if state.kind == "task":
            for index, task in enumerate(state.tasks):
                if task.session_id == session_id and (
                    provider is None or task.provider == provider
                ):
                    order: _BindingOrder = (
                        _timestamp_key(task.time),
                        created_key,
                        state.id,
                        0,
                        index,
                        "",
                    )
                    bindings.append((task.provider, order, state.manager_session))
        elif state.kind == "show":
            by_seat = {seat.id: seat.provider for seat in state.seats}
            for seat_id, saved in state.sessions.items():
                seat_provider = by_seat.get(seat_id)
                if (
                    saved.session_id == session_id
                    and seat_provider is not None
                    and (provider is None or seat_provider == provider)
                ):
                    order = (created_key, created_key, state.id, 1, 0, seat_id)
                    bindings.append((seat_provider, order, state.manager_session))
    return bindings


def check_foreign_session(
    records: Iterable[Record],
    session_id: str,
    provider: ProviderName | None,
    manager_session: str | None,
) -> None:
    """Reject resuming a native thread unless the earliest workspace recording it matches."""
    bindings = _session_bindings(records, session_id, provider)
    if not bindings:
        return
    earliest_by_provider: dict[str, tuple[_BindingOrder, str | None]] = {}
    for bound_provider, order, owner in bindings:
        current = earliest_by_provider.get(bound_provider)
        if current is None or order < current[0]:
            earliest_by_provider[bound_provider] = (order, owner)
    if provider is not None:
        owner = earliest_by_provider[provider][1]
        if owner != manager_session:
            raise _foreign_session_error(session_id, owner)
        return
    local_providers = sorted(
        {bound_provider for bound_provider, _, owner in bindings if owner == manager_session},
        key=lambda item: earliest_by_provider[item][0],
    )
    if local_providers:
        for bound_provider in local_providers:
            owner = earliest_by_provider[bound_provider][1]
            if owner != manager_session:
                raise _foreign_session_error(session_id, owner)
        return
    _, (_, owner) = min(earliest_by_provider.items(), key=lambda item: item[1][0])
    raise _foreign_session_error(session_id, owner)


def check_session_account(
    records: Iterable[Record],
    session_id: str,
    provider: ProviderName | None,
    account: str | None,
) -> None:
    """Reject resuming a native session as an account whose configuration directory is not the
    one it was recorded in: each directory keeps its own sessions (ADR-0108)."""
    recorded: set[tuple[str, str | None]] = set()
    for record in records:
        state = record.state
        if state.kind == "task":
            recorded.update(
                (task.provider, task.account)
                for task in state.tasks
                if task.session_id == session_id
            )
        else:
            by_seat = {seat.id: seat for seat in state.seats}
            recorded.update(
                (seat.provider, seat.account)
                for seat_id, saved in state.sessions.items()
                if saved.session_id == session_id and (seat := by_seat.get(seat_id)) is not None
            )
    for bound_provider, bound_account in recorded:
        if provider is not None and bound_provider != provider:
            continue
        if not accounts.same_home(get_adapter(bound_provider), bound_account, account):
            raise ValueError(
                f"Native session {session_id} was started as "
                f"{'account ' + repr(bound_account) if bound_account else 'the default account'} "
                "and cannot be resumed as "
                f"{'account ' + repr(account) if account else 'the default account'}: each "
                "account keeps its sessions in its own configuration directory. Omit account, "
                "or start a new thread and use forward to carry prior results"
            )


def latest_session_task(
    records: Iterable[Record],
    session_id: str,
    provider: ProviderName | None,
    manager_session: str | None,
) -> TaskRecord | None:
    """Only the owning workspace's history may supply defaults, identify a provider, or resume."""
    all_records = list(records)
    check_foreign_session(all_records, session_id, provider, manager_session)
    owned = [
        (record.state, index, task)
        for record in all_records
        if record.state.kind == "task" and record.state.manager_session == manager_session
        for index, task in enumerate(record.state.tasks)
        if task.session_id == session_id
    ]
    candidates = [item for item in owned if provider is None or item[2].provider == provider]
    if not candidates:
        if owned:
            raise ValueError("A native session cannot change provider; use its original provider")
        return None
    if len({task.provider for _, _, task in candidates}) > 1:
        raise ValueError("session_id matches multiple providers; pass provider and cwd explicitly")
    return max(
        candidates,
        key=lambda item: (item[2].time, item[0].created, item[0].id, item[1]),
    )[2]


def review_role(role: str | None) -> str:
    """Only review role labels select a seat; orchestration names require seat."""
    name = (role or "").strip()
    return name if name in get_args(ReviewRole.__value__) else ""


def is_image_seat(name: str | None, custom: CustomSeat | None) -> bool:
    return (name or "").strip() == "image-generator" or (
        custom is not None and custom.kind == IMAGE_KIND
    )


def uses_random_candidates(seat_name: str | None, host: Host) -> bool:
    """Whether ADR-0093 equal-weight candidate selection applies to this seat."""
    if not random_candidates_enabled(host):
        return False
    custom = custom_seat((seat_name or "").strip(), host)
    return custom is not None and not is_image_seat(seat_name, custom)


def role_access(role: str | None) -> Literal["read", "write"] | None:
    """Explorers and reviewers read; Workers and the image generator write."""
    name = (role or "").strip()
    if definition := seat_role(name):
        return definition.access
    if name == "image-generator":
        return "write"
    return "read" if name in get_args(ReviewRole.__value__) else None


class NativeSeat(Exception):
    """The chosen seat belongs to this host's CLI, so the service can start nothing for it."""

    def __init__(self, name: str, seat: ReviewSeat) -> None:
        super().__init__(name, seat)
        self.name = name
        self.seat = seat


@dataclass
class StartArguments:
    provider: ProviderName | None
    cwd: str | None
    role: str | None
    seat: str | None  # a catalog seat, or one of the user's own seats
    access: Literal["read", "write"] | None
    defaults: list[str] = field(default_factory=lambda: [])
    # Model and effort the resumed thread last ran with; see apply_history.
    history: dict[str, str] = field(default_factory=lambda: {})
    selected: ReviewSeat | None = None
    resumed_task: TaskRecord | None = None
    account: str | None = None  # a registered account of the provider; None: the default one
    terminal: bool = False

    def inherit(self, task: TaskRecord) -> None:
        self.provider = self._inherited(
            self.provider, cast(ProviderName, task.provider), "provider"
        )
        # The thread's session lives in its account's directory, so it keeps that account.
        self.account = self._inherited(self.account, task.account, "account")
        self.cwd = self._inherited(self.cwd, task.cwd, "cwd")
        role = (self.role or "").strip()
        self.role = self._inherited(self.role, task.role, "role")
        # A new role is a new duty: the old seat would otherwise outrank it. Blank only clears the label.
        if self.seat is not None or role in ("", task.role):
            self.seat = self._inherited(self.seat, task.seat, "seat")
        if task.model:
            self.history["model"] = task.model
            if task.effort:
                self.history["effort"] = task.effort

    def apply_history(self, options: dict[str, ProviderOption]) -> dict[str, ProviderOption]:
        """Keep a resumed thread on its last model unless the caller or a seat chose one.

        Otherwise a resume without options falls back to the App or CLI default model, which
        may be a different and costlier one than the thread was started with.
        """
        if "model" in options or "model" not in self.history:
            return options
        result = {**options, "model": self.history["model"]}
        self.defaults.append("model (session_id history)")
        if "effort" in self.history and "effort" not in result:
            result["effort"] = self.history["effort"]
            self.defaults.append("effort (session_id history)")
        return result

    def _inherited[T](self, explicit: T | None, saved: T | None, key: str) -> T | None:
        if explicit is not None or saved is None:
            return explicit
        self.defaults.append(f"{key} (session_id history)")
        return saved

    def resolve_seat(
        self, host: Host, provider_options: Mapping[str, ProviderOption] | None = None
    ) -> None:
        if self.seat is not None:
            validate_task_seat(
                self.seat, host, inherited="seat (session_id history)" in self.defaults
            )
        name = self.seat or review_role(self.role)
        seat: ReviewSeat | None = None
        if self.provider is None:
            if not name:
                raise ValueError("Pass provider, or seat / a review role to select an App seat")
            seat = self.selected or role_seat(name, host, required=True)
            assert seat is not None
            self.provider = seat.provider
            self.defaults.append(f"provider ({name} seat)")
            if self.account is None and seat.account is not None:
                self.account = seat.account
                self.defaults.append(f"account ({name} seat)")
        elif name and self.account is None and self.resumed_task is None:
            # The seat's account goes with its CLI unless the caller named one; settled before
            # telling whether the seat is the host's own account.
            chosen = effective_seat(self, name, host)
            if chosen is not None and chosen.provider == self.provider and chosen.account:
                self.account = chosen.account
                self.defaults.append(f"account ({name.strip()} seat)")
        custom = custom_seat(name, host)
        # The seat's access is only a default: a declared one stands, even when it differs.
        implied = custom.access if custom is not None else role_access(name)
        if implied is not None and self.access is None:
            self.access = implied
            self.defaults.append(f"access ({name} seat)")
        accounts.check_choice(self.provider, self.account)
        own = accounts.is_host_account(host, self.provider, self.account)
        if is_image_seat(name, custom) and own:
            raise ValueError(IMAGE_HOST_CLI_REASON)
        choice = host_cli_mode(host)
        if seat is not None and own and choice == "subagent" and not self.terminal:
            options, _ = apply_seat(seat, seat.provider, provider_options or {})
            model = options.get("model", seat.model)
            effort = options.get("effort", "")
            if not isinstance(model, str) or not model.strip():
                raise ValueError("model must be a non-empty string")
            if not isinstance(effort, str):
                raise ValueError("effort must be a string")
            if model != seat.model:
                # A named model stays within the CLI's pool, as it would for a started task.
                from agentnave.member_policy import check_cli_model

                document = read_members()
                check_cli_model(
                    document.clis.get(seat.provider) if document else None, seat.provider, model
                )
            provider_health.check(data_root(), [(seat.provider, self.account, model)])
            raise NativeSeat(
                name,
                ReviewSeat(
                    provider=seat.provider, model=model, effort=effort, account=self.account
                ),
            )
        if own and choice == "skip" and not self.terminal:
            raise ValueError("CLI is the host's own CLI")


def reject_foreign_seat(
    explicit_seat: str | None, provider: ProviderName | None, task: TaskRecord | None, host: Host
) -> None:
    """A resumed native session keeps its CLI, so another CLI's seat would be a false record."""
    name = (explicit_seat or "").strip()
    if not name or provider is None:
        return
    resumed_seat = (task.seat or "").strip() if task is not None else ""
    if name == resumed_seat:
        return
    validate_task_seat(name, host)
    seat = role_seat(name, host)
    if seat is None or seat.provider == provider:
        return
    raise ValueError(
        f"{name} seat runs on {seat.provider}, but the resumed native session runs on {provider}: "
        "a native session cannot switch to a seat of another CLI, and keeping the thread's model "
        f"would record a seat that never ran. Omit seat to keep the thread's seat, choose a seat "
        f"of {provider}, or omit session_id to start a new thread with {name} and hand the work over."
    )


def resolve_start(
    arguments: StartArguments,
    conversations: Conversations,
    host: Host,
    session_id: str | None,
    manager_session_id: str | None,
    provider_options: Mapping[str, ProviderOption] | None = None,
) -> StartArguments:
    explicit_seat = arguments.seat
    if session_id is not None:
        task = conversations.resume_task(
            session_id,
            arguments.provider,
            manager_session_id,
        )
        if task is not None:
            arguments.resumed_task = task
            arguments.inherit(task)
        elif arguments.provider is None or arguments.cwd is None:
            raise ValueError("Unknown session_id; pass provider and cwd explicitly")
    if arguments.cwd is None:
        raise ValueError("cwd is required for a new task; pass an absolute existing directory")
    if session_id is not None:
        reject_foreign_seat(explicit_seat, arguments.provider, arguments.resumed_task, host)
        check_session_account(
            conversations.records.values(), session_id, arguments.provider, arguments.account
        )
    arguments.resolve_seat(host, provider_options)
    if arguments.access == "read":
        arguments.defaults.append(READ_ONLY_APPLIED)
    return arguments


def effective_seat(
    arguments: StartArguments, seat_name: str | None, host: Host
) -> ReviewSeat | None:
    """Seat settings to apply: keep the drawn candidate when resuming the same randomized seat."""
    if arguments.selected is not None:
        return arguments.selected
    configured = role_seat(seat_name, host)
    resumed_seat = arguments.resumed_task.seat if arguments.resumed_task is not None else None
    if (
        configured is not None
        and bool(seat_name)
        and resumed_seat is not None
        and seat_name.strip() == resumed_seat.strip()
        and "model" in arguments.history
        and uses_random_candidates(seat_name, host)
    ):
        return next(
            (
                c.alone()
                for c in [configured, *configured.fallbacks]
                if c.key() == (arguments.provider, arguments.account, arguments.history["model"])
            ),
            configured,
        )
    return configured


def apply_task_defaults(
    provider: ProviderName,
    seat_name: str | None,
    options: dict[str, ProviderOption],
    seat_applied: list[str],
    excluded: frozenset[str],
    host: Host,
    selected: ReviewSeat | None = None,
) -> tuple[dict[str, ProviderOption], list[str]]:
    selected = selected or role_seat(seat_name, host)
    options, applied = apply_member(
        read_members(),
        "task",
        provider,
        options,
        excluded,
        permissions=read_permissions(),
        seat=frozenset(seat_applied),
        scenario=not orchestration_enabled(host),
        seat_selected=selected is not None and selected.provider == provider,
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
    *,
    isolation: TaskIsolationInfo | None = None,
    own_lease_paths: Collection[Path] = (),
) -> None:
    if access == "read":
        return
    sharing = [
        listing.invocation_id
        for listing in manager.list_invocations()
        if listing.status is None
        and not listing.read_only
        and shares_cwd_tree(listing.cwd, request.cwd)
        and listing.invocation_id != invocation_id
    ]
    if sharing:
        warnings.append(
            f"{len(sharing)} other CLI run(s) active in this cwd ({', '.join(sharing)}); "
            "parallel writers need separate worktrees. Declare access=read for read-only work."
        )
    if isolation is not None:
        try:
            external = other_active_leases(
                isolation.repo_common_dir,
                isolation.id,
                exclude_paths=own_lease_paths,
            )
        except (OSError, ValueError):
            external = 0
        if external > 0:
            warnings.append(
                f"{external} active lease(s) held by another process in isolated directory "
                f"{isolation.id}; parallel writers need separate worktrees. "
                "Declare access=read for read-only work."
            )

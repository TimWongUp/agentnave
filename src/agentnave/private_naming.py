"""Operation-local private naming contexts; no cache survives a scan, save or live tick."""

from __future__ import annotations

import logging
from collections.abc import Generator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass, field

from agentnave.db import Database
from agentnave.member_names import private_members
from agentnave.naming import MemberName
from agentnave.store import Conversation, Record


def _facts(state: Conversation) -> tuple[tuple[object, ...], ...]:
    return tuple(
        (t.id, t.time, t.provider, t.session_id, t.model, t.reported_model, t.role)
        for t in state.tasks
    )


@dataclass
class _WorkspaceNames:
    states: dict[str, Conversation]
    facts: dict[str, tuple[tuple[object, ...], ...]] = field(
        default_factory=lambda: dict[str, tuple[tuple[object, ...], ...]]()
    )
    members: list[MemberName] = field(default_factory=lambda: list[MemberName]())

    def peers(self, state: Conversation) -> list[MemberName]:
        facts = _facts(state)
        if not self.facts or self.facts.get(state.id) != facts:
            # Replace this conversation before deriving keys: a task acquiring its native
            # session must not coexist with its older, session-less database copy.
            self.states[state.id] = state
            self.facts = {id_: _facts(s) for id_, s in self.states.items()}
            self.members = private_members(self.states.values())
        return self.members


_SCOPES: ContextVar[dict[tuple[Database, str | None, str | None], _WorkspaceNames] | None] = (
    ContextVar("private_naming_scopes", default=None)
)


@contextmanager
def naming_scope() -> Generator[None]:
    if _SCOPES.get() is not None:
        yield
        return
    token = _SCOPES.set({})
    try:
        yield
    finally:
        _SCOPES.reset(token)


def private_states(record: Record, state: Conversation | None = None) -> _WorkspaceNames:
    state = record.state if state is None else state
    workspace = state.manager_session
    key = (record.db, workspace, state.id if workspace is None else None)
    scopes = _SCOPES.get()
    if scopes is not None and key in scopes:
        return scopes[key]
    try:
        states = record.db.private_conversations(workspace)
    except OSError:
        # Naming is derived; a failed sibling read cannot prevent an authoritative save.
        logging.getLogger(__name__).warning("Could not read workspace member names")
        states = []
    names = _WorkspaceNames({s.id: s for s in states})
    if scopes is not None:
        scopes[key] = names
    return names


def naming_peers(record: Record, state: Conversation | None = None) -> list[MemberName] | None:
    state = record.state if state is None else state
    if state.kind != "task":
        return None
    return private_states(record, state).peers(state)

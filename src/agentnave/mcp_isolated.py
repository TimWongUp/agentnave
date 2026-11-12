"""MCP inputs, receipt projections, and workspace-scoped isolated_dirs tool (ADR-0085)."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Annotated, Any, Literal

from mcp.server.mcpserver.context import Context
from mcp.server.mcpserver.exceptions import ToolError
from pydantic import BaseModel, ConfigDict, Field

from agentnave import mcp_runtime as runtime
from agentnave.conversations import Conversations
from agentnave.isolated import IsolatedListItem, ReleaseResult
from agentnave.isolated.prompts import TaskIsolationInfo
from agentnave.manager_identity import session_id_hint
from agentnave.mcp_payloads import (
    IsolatedDirItemPayload,
    IsolatedDirsPayload,
    TaskIsolationPayload,
    TaskStartPayload,
)
from agentnave.mcp_runtime import (
    ManagerSessionId,
    Runtime,
    _conversations,
    _rejected,
)


class IsolationInput(BaseModel):
    model_config = ConfigDict(extra="forbid")

    ref: str | None = Field(
        default=None,
        min_length=1,
        description="Commit-ish to branch from; omit for cwd HEAD.",
    )


def start_isolation_payload(info: TaskIsolationInfo) -> TaskIsolationPayload:
    payload: TaskIsolationPayload = {
        "id": info.id,
        "path": info.path,
        "cwd": info.cwd,
        "branch": info.branch,
    }
    if info.base_oid is not None:
        payload["base_oid"] = info.base_oid
    return payload


async def attach_unreleased_count(
    payload: TaskStartPayload,
    warnings: list[str],
    conversations: Conversations,
    workspace_id: str | None,
) -> None:
    if workspace_id is None:
        return
    try:
        count = await asyncio.to_thread(conversations.isolated.unreleased_count, workspace_id)
    except (OSError, ValueError) as exc:
        warnings.append(f"Could not read unreleased isolated directory count: {exc}")
        return
    if count > 0:
        payload["unreleased_isolation_count"] = count


def isolated_item_payload(item: IsolatedListItem) -> IsolatedDirItemPayload:
    payload: IsolatedDirItemPayload = {
        "id": item.id,
        "path": item.path,
        "cwd": item.cwd,
        "branch": item.branch,
        "base_oid": item.base_oid,
        "state": item.state,
        "in_use": item.in_use,
        "created_at": item.created_at,
        "own_workspace": item.own_workspace,
    }
    if item.attention is not None:
        payload["attention"] = item.attention
    return payload


async def attach_handover_isolated_dirs(
    result: dict[str, Any],
    conversations: Conversations,
    workspace_id: str,
) -> None:
    try:
        items = await asyncio.to_thread(conversations.isolated.list_for_workspace, workspace_id)
    except (OSError, ValueError) as exc:
        result.setdefault("warnings", []).append(
            f"Could not read isolated directories for workspace {workspace_id}: {exc}"
        )
        return
    result["isolated_dirs"] = [isolated_item_payload(item) for item in items]


def _release_payload(released: ReleaseResult) -> IsolatedDirsPayload:
    payload: IsolatedDirsPayload = {
        "id": released.id,
        "status": released.status,
    }
    if released.head_oid is not None:
        payload["head_oid"] = released.head_oid
    if released.status == "confirmation_required":
        payload["uncommitted"] = released.uncommitted
        payload["unique_commits"] = released.unique_commits
        if released.confirmation is not None:
            payload["confirmation"] = released.confirmation
    elif released.uncommitted:
        payload["uncommitted"] = True
    if released.unique_commits and "unique_commits" not in payload:
        payload["unique_commits"] = True
    if released.nested_repos:
        payload["nested_repos"] = list(released.nested_repos)
    if released.reason is not None:
        payload["reason"] = released.reason
    return payload


async def isolated_dirs(
    action: Annotated[
        Literal["list", "release"],
        Field(description="list worktrees or release one by id."),
    ],
    id: Annotated[  # noqa: A002 - public tool argument name
        str | None,
        Field(min_length=1, description="wt-... id to release; omit for list."),
    ] = None,
    confirmation: Annotated[
        str | None,
        Field(
            min_length=1,
            description="Snapshot token from confirmation_required; omit on first release or list.",
        ),
    ] = None,
    cwd: Annotated[
        str | None,
        Field(
            min_length=1,
            description="list only: absolute path in a repo; lists its worktrees from every "
            "workspace, own_workspace marks yours. Omit for this workspace only.",
        ),
    ] = None,
    manager_session_id: ManagerSessionId = None,
    *,
    ctx: Context[Runtime],
) -> IsolatedDirsPayload:
    """List or delete isolated Git worktrees (no paths accepted). release permanently deletes
    the worktree, including all Git-ignored files; with uncommitted source files or unmerged
    commits, or when the worktree belongs to another workspace, it first returns inspection
    results and a confirmation token without deleting; call again with the token.
    """
    if action == "list":
        if id is not None or confirmation is not None:
            raise ToolError("list does not accept id or confirmation. Correct and retry.")
    elif id is None:
        raise ToolError("release requires id. Correct and retry.")
    elif cwd is not None:
        raise ToolError("release does not accept cwd. Correct and retry.")
    if cwd is not None and not Path(cwd).is_absolute():
        raise ToolError("cwd must be an absolute path. Correct and retry.")
    conversations = _conversations(ctx)
    try:
        workspace_id = conversations.workspace_id(manager_session_id)
    except ValueError as exc:
        raise _rejected(exc) from exc
    if workspace_id is None:
        raise ToolError("isolated_dirs needs manager_session_id. " + session_id_hint(runtime.HOST))
    try:
        if action == "list":
            items = (
                await asyncio.to_thread(conversations.isolated.list_for_workspace, workspace_id)
                if cwd is None
                else await asyncio.to_thread(
                    conversations.isolated.list_for_repo, Path(cwd), workspace_id
                )
            )
            return {"items": [isolated_item_payload(item) for item in items]}
        assert id is not None
        released = await asyncio.to_thread(
            conversations.isolated.release,
            id,
            workspace_id=workspace_id,
            confirmation=confirmation,
        )
    except (OSError, ValueError) as exc:
        raise _rejected(exc) from exc
    return _release_payload(released)


__all__ = [
    "IsolationInput",
    "attach_handover_isolated_dirs",
    "attach_unreleased_count",
    "isolated_dirs",
    "isolated_item_payload",
    "start_isolation_payload",
]

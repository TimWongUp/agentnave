"""Agent-only STDIO MCP server entry point and compatibility exports."""

from mcp_types import ToolAnnotations

from agentnave.mcp_history import (
    _handover_conversations,
    _workspace_label,
    clear_provider_cooldown,
    handover,
    list_all,
    list_members,
    read_conversation,
)
from agentnave.mcp_isolated import isolated_dirs
from agentnave.mcp_prompts import answer_prompt
from agentnave.mcp_rounds import (
    _WORKSPACE_ID,
    ReviewReceipt,
    _add_started,
    _open_show,
    _retry_seats,
    decide_answers,
    run_round,
)
from agentnave.mcp_runtime import (
    _CHAT_TOOLS,
    _HIDDEN_TOOLS,
    _TITLE_WIDE_RANGES,
    ACCESS,
    EXCLUDED_PROVIDERS,
    HOST,
    PROVIDER_OPTIONS,
    PROVIDER_SELECTION,
    TITLE_WIDTH,
    WAIT_SECONDS,
    GroupSeatSpec,
    ManagerModel,
    ManagerSessionId,
    Runtime,
    _check_arguments,
    _committed_payload,
    _conversations,
    _lifespan,
    _manager,
    _read_excluded_providers,
    _rejected,
    _review_map,
    _Server,
    _short_title,
    _unknown_handle,
    _unreceived,
    finished_payload,
    invocation_request_error,
    mcp,
    round_payload,
    started_payload,
    update_payload,
)
from agentnave.mcp_tasks import (
    _invocation_items,
    _known_invocation,
    _known_round,
    _ready_handles,
    cancel,
    list_invocations,
    start_agent,
    wait,
)

__all__ = [
    "_handover_conversations",
    "_workspace_label",
    "clear_provider_cooldown",
    "handover",
    "isolated_dirs",
    "list_all",
    "list_members",
    "read_conversation",
    "_WORKSPACE_ID",
    "ReviewReceipt",
    "_add_started",
    "_open_show",
    "_retry_seats",
    "decide_answers",
    "run_round",
    "_CHAT_TOOLS",
    "_HIDDEN_TOOLS",
    "_TITLE_WIDE_RANGES",
    "ACCESS",
    "EXCLUDED_PROVIDERS",
    "HOST",
    "PROVIDER_SELECTION",
    "TITLE_WIDTH",
    "WAIT_SECONDS",
    "GroupSeatSpec",
    "ManagerModel",
    "ManagerSessionId",
    "Runtime",
    "_check_arguments",
    "_committed_payload",
    "_conversations",
    "_lifespan",
    "_manager",
    "_read_excluded_providers",
    "_rejected",
    "_review_map",
    "_Server",
    "_short_title",
    "_unknown_handle",
    "_unreceived",
    "finished_payload",
    "invocation_request_error",
    "mcp",
    "round_payload",
    "started_payload",
    "update_payload",
    "PROVIDER_OPTIONS",
    "_invocation_items",
    "_known_invocation",
    "_known_round",
    "_ready_handles",
    "answer_prompt",
    "cancel",
    "list_invocations",
    "start_agent",
    "wait",
]

mcp.tool(
    title="Run a task with a local AI CLI",
    annotations=ToolAnnotations(
        read_only_hint=False,
        destructive_hint=True,
        idempotent_hint=False,
        open_world_hint=True,
    ),
)(start_agent)

mcp.tool(
    title="Wait for tasks or rounds",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(wait)

mcp.tool(
    title="Answer a pending native CLI prompt",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=True
    ),
)(answer_prompt)

mcp.tool(
    title="Cancel a task or round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=True, open_world_hint=False
    ),
)(cancel)

mcp.tool(
    title="List invocations for the relay",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(list_invocations)

mcp.tool(
    name="list",
    title="List handles and saved conversations",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(list_all)

mcp.tool(
    title="Read configured members",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(list_members)

mcp.tool(
    title="Clear a CLI cooldown",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(clear_provider_cooldown)

mcp.tool(
    title="Run a meeting round",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=True
    ),
)(run_round)

mcp.tool(
    title="Publish answers under review or keep them private",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(decide_answers)

mcp.tool(
    title="Hand a workspace over to another boss",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
    ),
)(handover)

mcp.tool(
    title="Read a saved conversation",
    annotations=ToolAnnotations(
        read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
    ),
)(read_conversation)

mcp.tool(
    title="List or release isolated worktrees",
    annotations=ToolAnnotations(
        read_only_hint=False, destructive_hint=True, idempotent_hint=False, open_world_hint=False
    ),
)(isolated_dirs)


def main() -> None:
    mcp.run(transport="stdio")


if __name__ == "__main__":
    main()

"""Compact MCP replies without changing user data or required container contracts."""

from typing import cast

# Opaque fields are never traversed; only empty bodies must also retain their keys.
_OPAQUE_FIELDS = frozenset(
    {"options", "provider_options", "cli_options", "output", "text", "prompt", "note", "notes"}
)
_BODY_FIELDS = frozenset({"output", "text", "prompt"})

# Required reply containers, including the relay's invocations list. Keep the same
# rule for nested rounds/modes and the untyped history/handover reply containers.
_REQUIRED_CONTAINERS = frozenset(
    {
        "invocations",
        "conversations",
        "seats",
        "entries",
        "pending_ids",
        "updates",
        "rounds",
        "unavailable",
        "decisions",
        "pending_review",
        "committed",
        "modes",
        "members",
        "messages",
        "tasks",
        "waiting",
    }
)


def without_empty_fields(value: object) -> object:
    if isinstance(value, dict):
        projected: dict[str, object] = {}
        for key, item in cast(dict[str, object], value).items():
            if key in _BODY_FIELDS:
                projected[key] = item
                continue
            if key not in _OPAQUE_FIELDS:
                item = without_empty_fields(item)
            if (key in _REQUIRED_CONTAINERS and isinstance(item, (dict, list))) or (
                item is not None and item != "" and item != [] and item != {}
            ):
                projected[key] = item
        return projected
    if isinstance(value, list):
        # Positions and scalar values in arrays are part of the caller's data.
        return [without_empty_fields(item) for item in cast(list[object], value)]
    return value

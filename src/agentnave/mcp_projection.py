"""Compact MCP replies without changing user data or required container contracts."""

from typing import cast

# Opaque fields are never traversed; only non-null empty bodies also retain their keys.
_OPAQUE_FIELDS = frozenset(
    {
        "identity",
        "options",
        "provider_options",
        "output",
        "text",
        "prompt",
        "note",
        "notes",
        "tool_input",
        "questions",
        "answers",
    }
)
_BODY_FIELDS = frozenset({"output", "text", "prompt", "tool_input"})
_IDENTITY_FIELDS = ("handle", "model_source")

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
        "items",
        "isolated_dirs",
        "changed_files_from_base",
        "latest",
    }
)


def without_empty_fields(value: object) -> object:
    if isinstance(value, dict):
        projected: dict[str, object] = {}
        for key, item in cast(dict[str, object], value).items():
            if key in _BODY_FIELDS:
                if item is not None:
                    projected[key] = item
                continue
            if key == "identity" and isinstance(item, dict):
                # The boss only cites members by handle; the App reads the full identity from
                # saved views, so replies drop the parts the handle already spells out.
                identity = cast(dict[str, object], item)
                item = {k: identity[k] for k in _IDENTITY_FIELDS if identity.get(k) is not None}
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

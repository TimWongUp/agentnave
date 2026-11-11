"""Actionable diagnostics for a boss identity that cannot name its native model."""

from agentnave.group import Entry
from agentnave.host import Host
from agentnave.store import EntryRecord, TaskRecord

# Where each host's shell exposes the chat's native session ID.
_SESSION_ID_SHELL_ENV = {
    "claude": "CLAUDE_CODE_SESSION_ID",
    "codex": "CODEX_THREAD_ID",
    "antigravity": "ANTIGRAVITY_CONVERSATION_ID",
    "codebuddy": "CODEBUDDY_SESSION_ID",
    "workbuddy": "CODEBUDDY_SESSION_ID",
    "grok": "GROK_SESSION_ID",
    "pi": "PI_SESSION_ID",
    "deepseek": "DSH_SESSION_ID",
    "hermes": "HERMES_SESSION_ID",
}

# Parameter schemas name the host variable once; full identity rules live in MCP instructions.
SESSION_ID_DESCRIPTIONS = {
    name: f"Boss chat ID from {variable}; see MCP instructions."
    for name, variable in _SESSION_ID_SHELL_ENV.items()
}


def session_id_hint(host: Host) -> str:
    variable = _SESSION_ID_SHELL_ENV.get(host.name or "")
    if variable is not None:
        return (
            f"Read {variable} in this chat's shell (PowerShell: $env:{variable}; "
            f"POSIX: printenv {variable}) and pass that exact value. "
            "Do not generate a replacement UUID or use a subagent session_id. "
            "Only generate a UUID if the host exposes no native chat ID; a failed shell "
            "command does not establish that the ID is unavailable. "
        )
    known = ", ".join(sorted(set(_SESSION_ID_SHELL_ENV.values())))
    return (
        "Read this chat's native session ID from the host context or shell first "
        f"(hosts expose one of: {known}); "
        "only if the host exposes none, generate one UUID once and reuse it for this chat. "
    )


def model_warnings(
    host: Host,
    message: Entry | EntryRecord | TaskRecord,
    chat: str | None = None,
    warned: set[str] | None = None,
) -> list[str]:
    """Say once per boss chat that its model is unknown; repeating it on every reply is noise.

    `warned` is the server's record of chats already told; without it every call warns.
    """
    if message.manager_reported_model or message.manager_model:
        return []
    if chat is not None and warned is not None:
        if chat in warned:
            return []
        warned.add(chat)
    variable = _SESSION_ID_SHELL_ENV.get(host.name or "")
    hint = (
        f"read {variable} in the shell"
        if variable is not None
        else "read this chat's native session ID"
    )
    return [
        f"Boss model unknown: {hint} and pass it as manager_session_id, or pass "
        "manager_model. Do not repeat this call; wait for work already started."
    ]

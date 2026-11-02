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
    return (
        "Read this chat's native session ID from the host context or shell first; "
        "only if the host exposes none, generate one UUID once and reuse it for this chat. "
    )


def model_warnings(host: Host, message: Entry | EntryRecord | TaskRecord) -> list[str]:
    if message.manager_reported_model or message.manager_model:
        return []
    return [
        "Boss model unavailable in App: neither native model nor manager_model was recorded. "
        + session_id_hint(host)
        + "Verify manager_session_id; if it is right, pass manager_model when trusted host "
        "context states the model. "
        "Do not repeat this call; wait for any work already started."
    ]

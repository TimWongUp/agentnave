"""Actionable diagnostics for a boss identity that cannot name its native model."""

from agentnave.group import Entry
from agentnave.host import Host
from agentnave.store import EntryRecord, TaskRecord


def session_id_hint(host: Host) -> str:
    if host.name == "codex":
        return (
            "Read CODEX_THREAD_ID in this chat's shell (PowerShell: $env:CODEX_THREAD_ID; "
            "POSIX: printenv CODEX_THREAD_ID) and pass that exact value. "
            "Do not generate a replacement UUID or use a subagent session_id. "
            "Only generate a UUID if the host exposes no native chat ID; a failed shell "
            "command does not establish that the ID is unavailable."
        )
    return (
        "Read this chat's native session ID from the host context or shell first; "
        "only if the host exposes none, generate one UUID once and reuse it for this chat."
    )


def model_warnings(host: Host, message: Entry | EntryRecord | TaskRecord) -> list[str]:
    if message.manager_reported_model or message.manager_model:
        return []
    hint = (
        "Read CODEX_THREAD_ID (PowerShell: $env:CODEX_THREAD_ID; POSIX: printenv CODEX_THREAD_ID). "
        if host.name == "codex"
        else "Read the host's native chat ID. "
    )
    return [
        "Boss model unavailable in App: neither native model nor manager_model was recorded. "
        + hint
        + "Verify manager_session_id; declare manager_model only from trusted host context. "
        "Do not repeat this call; wait for any work already started."
    ]

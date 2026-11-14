"""Read the App's private-task policy for the calling host."""

from typing import Literal

from agentnave.adapters import get_adapter
from agentnave.host import Host


def default_host_cli_mode(host: Host) -> Literal["subagent", "launch"]:
    if not host.provider:
        return "subagent"
    return get_adapter(host.provider).default_host_cli_mode


def host_cli_mode(host: Host) -> Literal["subagent", "launch", "skip"]:
    from agentnave.members import (
        MemberFileError,
        _read_app_file,  # pyright: ignore[reportPrivateUsage]
    )
    from agentnave.orchestration_roles import OrchestrationFile

    default = default_host_cli_mode(host)
    try:
        roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    except MemberFileError:
        return default
    return roles.host_cli_mode(host) if roles is not None else default

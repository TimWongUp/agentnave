"""Read the App's private-task policy for the calling host."""

from typing import Literal

from agentnave.host import Host


def host_cli_mode(host: Host) -> Literal["subagent", "launch", "skip"]:
    from agentnave.members import (
        MemberFileError,
        _read_app_file,  # pyright: ignore[reportPrivateUsage]
    )
    from agentnave.orchestration_roles import OrchestrationFile

    try:
        roles = _read_app_file("orchestration.json", OrchestrationFile, MemberFileError)
    except MemberFileError:
        return "subagent"
    return roles.host_cli_mode(host) if roles is not None else "subagent"

"""Entry point of `agentnave-studio-mcp`: the relay in front of the studio server."""

import os
import sys
from datetime import UTC, datetime
from uuid import uuid4

from agentnave.relay import command, serve

# One manager session per host connection: servers swapped in after an upgrade keep it.
SESSION_ENV = "AGENTNAVE_MANAGER_SESSION"
STARTED_ENV = "AGENTNAVE_MANAGER_STARTED_AT"


def main() -> None:
    env = {
        **os.environ,
        SESSION_ENV: uuid4().hex,
        STARTED_ENV: datetime.now(UTC).isoformat(),
    }
    # Shows reload from saved history, so a studio child never pins the relay.
    sys.exit(serve(command("agentnave_studio.mcp_server"), pin=set(), env=env))

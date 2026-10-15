"""Entry point of `agentnave-studio-mcp`: the relay in front of the studio server."""

import sys

from agentnave.relay import command, serve


def main() -> None:
    # Shows reload from saved history, so a studio child never pins the relay.
    sys.exit(serve(command("agentnave_studio.mcp_server"), pin=set()))

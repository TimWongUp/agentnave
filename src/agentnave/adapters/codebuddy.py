"""Transparent CodeBuddy Code CLI adapter."""

import os
from dataclasses import replace
from datetime import datetime

from agentnave.adapters.base import PreparedCommand
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.models import InvocationRequest


class CodeBuddyAdapter(ClaudeAdapter):
    """Use CodeBuddy's documented Claude-compatible stream result contract."""

    name = "codebuddy"
    options_description = (
        "permission_mode, agent, fallback_model, restricted_tools (boolean; restricted tools)"
    )
    executable = "codebuddy"
    _supports_interactive = False
    restricted_extra_options = frozenset()
    _options = {
        "model": "--model",
        "effort": "--effort",
        "permission_mode": "--permission-mode",
        "agent": "--agent",
        "fallback_model": "--fallback-model",
    }

    _discussion_args = (
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        '{"mcpServers":{}}',
        "--disallowedTools",
        "mcp__*",
    )

    def quota_reset(self, stdout: bytes, stderr: bytes, now: datetime) -> datetime | None:
        return None

    def prepare(self, request: InvocationRequest) -> PreparedCommand:
        prepared = super().prepare(request)
        if os.environ.get("CODEBUDDY_HOST") != "workbuddy-desktop":
            return prepared
        # The desktop's product selects account-scoped login, which the standalone
        # CLI cannot restore. Let it load its own product and existing CLI login.
        return replace(
            prepared,
            unset_env=(
                "ACC_PRODUCT_CONFIG",
                "ACC_PRODUCT_CONFIG_V2",
                "ACC_PRODUCT_CONFIG_V3",
                "ACC_PRODUCT_CONFIG_PATH",
            ),
        )

"""Transparent CodeBuddy Code CLI adapter."""

from agentnave.adapters.claude import ClaudeAdapter


class CodeBuddyAdapter(ClaudeAdapter):
    """Use CodeBuddy's documented Claude-compatible stream result contract."""

    name = "codebuddy"
    options_description = (
        "permission_mode, agent, fallback_model, restricted_tools (boolean; restricted tools)"
    )
    executable = "codebuddy"
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

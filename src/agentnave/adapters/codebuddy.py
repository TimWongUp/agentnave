"""Transparent CodeBuddy Code CLI adapter."""

from agentnave.adapters.claude import ClaudeAdapter


class CodeBuddyAdapter(ClaudeAdapter):
    """Use CodeBuddy's documented Claude-compatible stream result contract."""

    name = "codebuddy"
    executable = "codebuddy"
    _options = {
        "model": "--model",
        "effort": "--effort",
        "permission_mode": "--permission-mode",
        "agent": "--agent",
        "fallback_model": "--fallback-model",
    }

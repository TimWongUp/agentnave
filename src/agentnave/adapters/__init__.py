"""Built-in provider registry."""

from agentnave.adapters.antigravity import AntigravityAdapter
from agentnave.adapters.base import ProviderAdapter
from agentnave.adapters.claude import ClaudeAdapter
from agentnave.adapters.codebuddy import CodeBuddyAdapter
from agentnave.adapters.grok import GrokAdapter


def get_adapter(provider: str) -> ProviderAdapter:
    adapters: dict[str, ProviderAdapter] = {
        "antigravity": AntigravityAdapter(),
        "claude": ClaudeAdapter(),
        "codebuddy": CodeBuddyAdapter(),
        "grok": GrokAdapter(),
    }
    try:
        return adapters[provider]
    except KeyError as exc:
        raise ValueError(f"unsupported provider: {provider}") from exc


__all__ = ["ProviderAdapter", "get_adapter"]

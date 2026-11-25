"""The single source of decision-model providers: names, models, key rules and endpoint checks.

A bottom layer (ADR-0123): it imports only the standard library, so the App-file readers, the
provider clients and seat recommendation all depend on it and never on each other. Provider
wire formats stay in their own decision_* modules.
"""

from __future__ import annotations

import ipaddress
import urllib.parse
from typing import Literal

type DecisionProvider = Literal["gemini", "openai", "typesafe", "openrouter", "ollama", "custom"]

DEFAULT_DECISION_PROVIDER: DecisionProvider = "gemini"
GEMINI_MODEL = "gemini-decision-1"
OPENAI_MODEL = "gpt-6-luna"
DEFAULT_OLLAMA_BASE_URL = "http://127.0.0.1:11434"

DECISION_DEFAULT_MODELS: dict[DecisionProvider, str] = {
    "gemini": GEMINI_MODEL,
    "openai": OPENAI_MODEL,
    "typesafe": "jev-latest",
    "openrouter": "liquid/d1.2-ultra",
}
# Fixed-model providers accept exactly one model name.
FIXED_DECISION_MODELS: dict[DecisionProvider, str] = {
    "gemini": GEMINI_MODEL,
    "openai": OPENAI_MODEL,
}
# Providers whose endpoint the user may set.
BASE_URL_PROVIDERS: frozenset[DecisionProvider] = frozenset({"ollama", "custom"})
# Providers that never read decision-keys.json; a local Ollama endpoint takes no key.
NO_KEY_PROVIDERS: frozenset[DecisionProvider] = frozenset({"ollama"})
# Providers that work with or without a key.
OPTIONAL_KEY_PROVIDERS: frozenset[DecisionProvider] = frozenset({"custom"})

_DECISION_BASE_URL_MAX_LENGTH = 512


def is_loopback_host(host: str) -> bool:
    """True for localhost, 127.0.0.1, ::1, and any loopback IP address.

    An IPv4-mapped IPv6 address (::ffff:127.0.0.1) is not loopback here, matching the tray's rule.
    """
    stripped = host.strip("[]")
    if stripped.lower() == "localhost":
        return True
    try:
        address = ipaddress.ip_address(stripped)
    except ValueError:
        return False
    if isinstance(address, ipaddress.IPv6Address) and address.ipv4_mapped is not None:
        return False
    return address.is_loopback


def validate_decision_base_url(raw: str) -> str:
    """Validate and normalize a decision endpoint base URL (matching the tray rules)."""
    cleaned = raw.strip().rstrip("/")
    if not cleaned:
        raise ValueError("decision_base_url must not be empty")
    if len(cleaned) > _DECISION_BASE_URL_MAX_LENGTH:
        raise ValueError(
            f"decision_base_url must be at most {_DECISION_BASE_URL_MAX_LENGTH} characters"
        )
    if any(ord(ch) <= 0x20 or ord(ch) == 0x7F for ch in cleaned):
        raise ValueError("decision_base_url must not contain spaces or control characters")
    try:
        parsed = urllib.parse.urlsplit(cleaned)
    except ValueError as exc:
        raise ValueError("decision_base_url must be a valid URL") from exc
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError("decision_base_url must be an http or https URL with a host")
    authority = cleaned.split("://", 1)[1].split("/", 1)[0]
    if (
        parsed.username is not None
        or parsed.password is not None
        or "@" in parsed.netloc
        or "@" in authority
        or "\\" in authority
    ):
        raise ValueError("decision_base_url must not contain userinfo")
    if parsed.query or parsed.fragment or "?" in cleaned or "#" in cleaned:
        raise ValueError("decision_base_url must not contain query or fragment")
    try:
        port = parsed.port
    except ValueError as exc:
        raise ValueError("decision_base_url has an invalid port") from exc
    if authority.endswith(":") or port == 0:
        raise ValueError("decision_base_url has an invalid port")
    host = parsed.hostname
    if not host:
        raise ValueError("decision_base_url must contain a host")
    if parsed.scheme == "http" and not is_loopback_host(host):
        raise ValueError(
            "decision_base_url allows http only for loopback hosts (localhost, 127.0.0.1, ::1)"
        )
    return cleaned

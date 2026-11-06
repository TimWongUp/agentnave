"""Pure member names. Stable keys identify members; names are display attributes only."""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass
from typing import Literal, NotRequired, TypedDict


class MemberIdentity(TypedDict):
    key: str
    provider: str
    cli: str
    model: str
    model_short: str
    model_source: Literal["native", "requested", "unknown"]
    role: NotRequired[str | None]
    ordinal: int
    numbered: bool
    handle: str
    handle_short: str
    spoken_name: str


_CLI = {
    "claude": "Claude Code",
    "codex": "Codex",
    "antigravity": "Antigravity",
    "codebuddy": "CodeBuddy",
    "grok": "Grok",
    "pi": "Pi",
    "deepseek": "DeepSeek Harness",
    "zcode": "ZCode",
}
UNKNOWN_MODEL = "未记录模型"
_FAMILY = {
    "claude": "Claude",
    "gpt": "GPT",
    "gemini": "Gemini",
    "grok": "Grok",
    "deepseek": "DeepSeek",
    "glm": "GLM",
    "qwen": "Qwen",
    "kimi": "Kimi",
    "minimax": "MiniMax",
    "hunyuan": "Hunyuan",
    "doubao": "Doubao",
    "muse": "Muse",
}


def thread(provider: str, session_id: str | None) -> str | None:
    """Existing opaque native-session hash, shared by projections and naming."""
    if session_id is None:
        return None
    return hashlib.sha256(f"{provider}\0{session_id}".encode()).hexdigest()[:16]


@dataclass(frozen=True)
class MemberName:
    key: str
    provider: str
    requested: str = ""
    reported: str | None = None
    role: str | None = None
    ordinal: int = 1

    def identity(self, peers: list[MemberName]) -> MemberIdentity:
        model = self.reported or self.requested or ""
        short = model.rsplit("/", 1)[-1] if model else UNKNOWN_MODEL
        cli = _CLI.get(self.provider, self.provider)
        full_handle = self._handle(model or UNKNOWN_MODEL, cli)
        short_handle = self._handle(short, cli)
        numbered = any(
            peer.key != self.key
            and (full_handle == peer._handles()[0] or short_handle == peer._handles()[1])
            for peer in peers
        )
        suffix = f" #{self.ordinal}" if numbered else ""
        spoken = self._spoken()
        spoken_suffix = (
            f" #{self.ordinal}"
            if any(
                peer.key != self.key and (peer._spoken(), peer.role) == (spoken, self.role)
                for peer in peers
            )
            else ""
        )
        return {
            "key": self.key,
            "provider": self.provider,
            "cli": cli,
            "model": model,
            "model_short": short,
            "model_source": "native"
            if self.reported
            else "requested"
            if self.requested
            else "unknown",
            "role": self.role,
            "ordinal": self.ordinal,
            "numbered": numbered,
            "handle": full_handle + suffix,
            "handle_short": short_handle + suffix,
            "spoken_name": spoken + spoken_suffix,
        }

    def _handle(self, model: str, cli: str) -> str:
        return f"@{model}·{cli}" + (f"·{self.role}" if self.role else "")

    def _handles(self) -> tuple[str, str]:
        model = self.reported or self.requested or UNKNOWN_MODEL
        cli = _CLI.get(self.provider, self.provider)
        return self._handle(model, cli), self._handle(model.rsplit("/", 1)[-1], cli)

    def _spoken(self) -> str:
        model = (self.reported or self.requested or "").rsplit("/", 1)[-1]
        name = model.lower()
        for key, family in _FAMILY.items():
            if key in name:
                return family
        if re.match(r"^(o[134](?:-|$)|chatgpt|codex)", name):
            return "GPT"
        if re.match(r"^(opus|sonnet|haiku)(?:-|$)", name):
            return "Claude"
        if re.match(r"^hy\d", name):
            return "Hunyuan"
        if re.match(r"^seed(?:-|$)", name):
            return "Doubao"
        return model or UNKNOWN_MODEL

"""System One decision client for TypeSafe, OpenRouter, Ollama, and custom endpoints (ADR-0123)."""

from __future__ import annotations

import urllib.parse
from collections.abc import Mapping, Sequence
from typing import Any

from agentnave.decision_http import (
    TIMEOUT_SECONDS,
    build_keyed_batch_request,
    build_keyed_request,
    parse_keyed_batch_response,
    parse_keyed_response,
    post_json,
)
from agentnave.decision_model import Decision, DecisionError, DecisionQuestion

TYPESAFE_ENDPOINT = "https://api.typesafe.ai/v1/systemone"
OPENROUTER_ENDPOINT = "https://openrouter.ai/api/v1/systemone"


def resolve_systemone_endpoint(base_url: str) -> str:
    """Resolve an Ollama or custom base URL to its System One POST endpoint (path only)."""
    parsed = urllib.parse.urlsplit(base_url.strip())
    path = parsed.path.rstrip("/")
    if path.endswith("/systemone"):
        next_path = path
    elif path.endswith(("/v1", "/v1beta")):
        next_path = f"{path}/systemone"
    else:
        next_path = f"{path}/v1/systemone"
    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, next_path, "", ""))


def build_request(model: str, text: str, candidates: Mapping[str, str]) -> dict[str, Any]:
    """One System One choice question using state + criteria."""
    return build_keyed_request(model, text, candidates, text_key="state")


def build_batch_request(model: str, questions: Sequence[DecisionQuestion]) -> dict[str, Any]:
    """One System One request with one choice question per task."""
    return build_keyed_batch_request(model, questions, text_key="state")


def parse_response(body: object, candidates: Mapping[str, str]) -> Decision:
    """answers.<question>.choice, optional probabilities and optional confidence."""
    return parse_keyed_response(body, candidates)


def parse_batch_response(
    body: object, questions: Sequence[DecisionQuestion]
) -> list[Decision | DecisionError]:
    """answers.<question> for each question; a missing or unusable one fails only itself."""
    return parse_keyed_batch_response(body, questions)


class SystemOneDecisionClient:
    """POST /v1/systemone client with optional Bearer auth."""

    def __init__(
        self,
        endpoint: str,
        model: str,
        api_key: str | None = None,
        timeout: float = TIMEOUT_SECONDS,
    ) -> None:
        self._endpoint = endpoint
        self._model = model
        self._key = api_key or None
        self._timeout = timeout

    def _headers(self) -> dict[str, str]:
        headers = {"content-type": "application/json"}
        if self._key:
            headers["authorization"] = f"Bearer {self._key}"
        return headers

    def decide(self, text: str, candidates: Mapping[str, str]) -> Decision:
        body = post_json(
            self._endpoint,
            build_request(self._model, text, candidates),
            self._headers(),
            timeout=self._timeout,
            api_key=self._key,
        )
        return parse_response(body, candidates)

    def decide_many(self, questions: Sequence[DecisionQuestion]) -> list[Decision | DecisionError]:
        body = post_json(
            self._endpoint,
            build_batch_request(self._model, questions),
            self._headers(),
            timeout=self._timeout,
            api_key=self._key,
        )
        return parse_batch_response(body, questions)

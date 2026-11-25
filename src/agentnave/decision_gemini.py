"""Gemini Decision 1: the default decision model behind seat recommendation (ADR-0087)."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

from agentnave.decision_http import (
    INSTRUCTIONS as INSTRUCTIONS,
)
from agentnave.decision_http import (
    QUESTION as QUESTION,
)
from agentnave.decision_http import (
    TIMEOUT_SECONDS as TIMEOUT_SECONDS,
)
from agentnave.decision_http import (
    build_keyed_batch_request,
    build_keyed_request,
    parse_keyed_batch_response,
    parse_keyed_response,
    post_json,
)
from agentnave.decision_model import Decision, DecisionError, DecisionQuestion

ENDPOINT = "https://generativelanguage.googleapis.com/v1beta/decisions"
MODEL = "gemini-decision-1"
# The name this model's key has in decision-keys.json.
KEY_NAME = "gemini"


def build_request(text: str, candidates: Mapping[str, str]) -> dict[str, Any]:
    """One choice question; the criteria are the candidate ids with their descriptions."""
    return build_keyed_request(MODEL, text, candidates, text_key="input")


def build_batch_request(questions: Sequence[DecisionQuestion]) -> dict[str, Any]:
    """One request, one choice question per input; each names the task it is about."""
    return build_keyed_batch_request(MODEL, questions, text_key="input")


def parse_batch_response(
    body: object, questions: Sequence[DecisionQuestion]
) -> list[Decision | DecisionError]:
    """answers.<question> for each question; a missing or unusable one fails only itself."""
    return parse_keyed_batch_response(body, questions)


def parse_response(body: object, candidates: Mapping[str, str]) -> Decision:
    """answers.<question>.choice, probabilities and confidence."""
    return parse_keyed_response(body, candidates)


class GeminiDecisionClient:
    """One POST per decision: stdlib only, no redirects, one retry after a network failure."""

    def __init__(
        self,
        api_key: str,
        endpoint: str = ENDPOINT,
        timeout: float = TIMEOUT_SECONDS,
    ) -> None:
        self._key = api_key
        self._endpoint = endpoint
        self._timeout = timeout

    def _headers(self) -> dict[str, str]:
        return {"content-type": "application/json", "x-goog-api-key": self._key}

    def decide(self, text: str, candidates: Mapping[str, str]) -> Decision:
        body = post_json(
            self._endpoint,
            build_request(text, candidates),
            self._headers(),
            timeout=self._timeout,
            api_key=self._key,
            auth_on_400_api_key=True,
        )
        return parse_response(body, candidates)

    def decide_many(self, questions: Sequence[DecisionQuestion]) -> list[Decision | DecisionError]:
        body = post_json(
            self._endpoint,
            build_batch_request(questions),
            self._headers(),
            timeout=self._timeout,
            api_key=self._key,
            auth_on_400_api_key=True,
        )
        return parse_batch_response(body, questions)

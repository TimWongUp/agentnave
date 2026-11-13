"""Gemini Decision 1: the first decision model behind seat recommendation (ADR-0087).

The request and response formats come from the vendor documentation as a research seat read
it; no real call has verified them yet. Everything that depends on them is in build_request
and parse_response, so a real response only needs changes there.
"""

from __future__ import annotations

import http.client
import json
import urllib.error
import urllib.request
from collections.abc import Mapping, Sequence
from typing import Any, cast

from agentnave.decision_model import Decision, DecisionError, DecisionQuestion

ENDPOINT = "https://generativelanguage.googleapis.com/v1beta/decisions"
MODEL = "gemini-decision-1"
# The name this model's key has in decision-keys.json.
KEY_NAME = "gemini"
QUESTION = "seat"
INSTRUCTIONS = (
    "Pick the seat best suited to the task. Seats are described by specialty, "
    "difficulty and access."
)
TIMEOUT_SECONDS = 10
_MAX_RESPONSE_BYTES = 1_000_000
_MAX_DETAIL = 200


def build_request(text: str, candidates: Mapping[str, str]) -> dict[str, Any]:
    """One choice question; the criteria are the candidate ids with their descriptions."""
    return {
        "model": MODEL,
        "input": text,
        "questions": {
            QUESTION: {
                "type": "choice",
                "instructions": INSTRUCTIONS,
                "criteria": dict(candidates),
            }
        },
    }


def build_batch_request(questions: Sequence[DecisionQuestion]) -> dict[str, Any]:
    """One request, one choice question per input; each names the task it is about."""
    ids = [f"{QUESTION}{index}" for index in range(1, len(questions) + 1)]
    return {
        "model": MODEL,
        "input": "\n\n".join(
            f"[{key}]\n{one.text}" for key, one in zip(ids, questions, strict=True)
        ),
        "questions": {
            key: {
                "type": "choice",
                "instructions": f"{INSTRUCTIONS} Decide only for the task marked [{key}].",
                "criteria": dict(one.candidates),
            }
            for key, one in zip(ids, questions, strict=True)
        },
    }


def parse_batch_response(
    body: object, questions: Sequence[DecisionQuestion]
) -> list[Decision | DecisionError]:
    """answers.<question> for each question; a missing or unusable one fails only itself."""
    answers = _object(body, "answers")
    results: list[Decision | DecisionError] = []
    for index, one in enumerate(questions, 1):
        key = f"{QUESTION}{index}"
        try:
            results.append(_decision(_object(answers, key), one.candidates))
        except DecisionError as exc:
            results.append(exc)
    return results


def parse_response(body: object, candidates: Mapping[str, str]) -> Decision:
    """answers.<question>.choice, probabilities and confidence."""
    return _decision(_object(_object(body, "answers"), QUESTION), candidates)


def _decision(answer: dict[str, Any], candidates: Mapping[str, str]) -> Decision:
    choice = answer.get("choice")
    if not isinstance(choice, str) or choice not in candidates:
        raise DecisionError("bad_response", "The decision model chose no listed seat.")
    probabilities = answer.get("probabilities", {})
    if not isinstance(probabilities, dict) or not all(
        isinstance(key, str) and _is_number(value)
        for key, value in cast(dict[object, object], probabilities).items()
    ):
        raise DecisionError("bad_response", "The decision model returned unreadable probabilities.")
    confidence = answer.get("confidence")
    if confidence is not None and not _is_number(confidence):
        raise DecisionError("bad_response", "The decision model returned an unreadable confidence.")
    return Decision(
        choice,
        {
            key: float(value)
            for key, value in cast(dict[str, float], probabilities).items()
            if key in candidates
        },
        None if confidence is None else float(cast(float, confidence)),
    )


def _object(value: object, key: str) -> dict[str, Any]:
    found = cast(dict[str, Any], value).get(key) if isinstance(value, dict) else None
    if not isinstance(found, dict):
        raise DecisionError("bad_response", f"The decision model reply has no {key} object.")
    return cast(dict[str, Any], found)


def _is_number(value: object) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args: Any, **_kwargs: Any) -> None:
        return None


class GeminiDecisionClient:
    """One POST per decision: stdlib only, no redirects, no retry."""

    def __init__(
        self,
        api_key: str,
        endpoint: str = ENDPOINT,
        timeout: float = TIMEOUT_SECONDS,
    ) -> None:
        self._key = api_key
        self._endpoint = endpoint
        self._timeout = timeout

    def decide(self, text: str, candidates: Mapping[str, str]) -> Decision:
        return parse_response(self._post(build_request(text, candidates)), candidates)

    def decide_many(self, questions: Sequence[DecisionQuestion]) -> list[Decision | DecisionError]:
        return parse_batch_response(self._post(build_batch_request(questions)), questions)

    def _post(self, document: dict[str, Any]) -> object:
        request = urllib.request.Request(
            self._endpoint,
            data=json.dumps(document).encode(),
            headers={"content-type": "application/json", "x-goog-api-key": self._key},
            method="POST",
        )
        try:
            with urllib.request.build_opener(_NoRedirect).open(
                request, timeout=self._timeout
            ) as response:
                raw = response.read(_MAX_RESPONSE_BYTES)
        except urllib.error.HTTPError as exc:
            raise self._http_error(exc) from None
        except (OSError, http.client.HTTPException) as exc:
            reason = getattr(exc, "reason", exc)
            raise DecisionError(
                "network", f"The decision model is unreachable: {self._safe(str(reason))}"
            ) from None
        try:
            body: object = json.loads(raw)
        except ValueError:
            raise DecisionError("bad_response", "The decision model reply is not JSON.") from None
        return body

    def _http_error(self, exc: urllib.error.HTTPError) -> DecisionError:
        try:
            detail = exc.read(_MAX_RESPONSE_BYTES).decode("utf-8", errors="replace")
        except (OSError, http.client.HTTPException):
            detail = ""
        message = self._safe(_error_message(detail))
        status = exc.code
        if status == 429:
            return DecisionError("rate_limited", f"The decision model is rate limited: {message}")
        # Google reports an invalid API key as 400 INVALID_ARGUMENT, not 401.
        if status in (401, 403) or (status == 400 and "api key" in message.lower()):
            return DecisionError("auth", f"The decision model rejected the API key: {message}")
        return DecisionError("rejected", f"The decision model refused (HTTP {status}): {message}")

    def _safe(self, text: str) -> str:
        """Short, and never the key."""
        text = text.replace(self._key, "***") if self._key else text
        return text if len(text) <= _MAX_DETAIL else text[:_MAX_DETAIL] + "…"


def _error_message(detail: str) -> str:
    """Google's error.message when the body has one, else the raw text."""
    try:
        document: object = json.loads(detail)
    except ValueError:
        return detail.strip()
    error = cast(dict[str, object], document).get("error") if isinstance(document, dict) else None
    message = cast(dict[str, object], error).get("message") if isinstance(error, dict) else None
    return message if isinstance(message, str) else detail.strip()

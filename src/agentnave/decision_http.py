"""Shared HTTP transport, error mapping, and keyed choice helpers for decision clients."""

from __future__ import annotations

import http.client
import json
import math
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping, Sequence
from typing import Any, cast

from agentnave.decision_model import Decision, DecisionError, DecisionQuestion
from agentnave.decision_sources import is_loopback_host

QUESTION = "seat"
INSTRUCTIONS = (
    "Pick the seat best suited to the task. Seats are described by specialty, difficulty and "
    "access. Choose the specialty from what the task mainly delivers, and the lowest difficulty "
    "that can get the task done: a higher difficulty costs more and runs slower, and the "
    "manager redispatches one level up when a seat falls short. A long task text, many files "
    "involved, or words such as 'difficulty' or 'the hard part' in the text are never by "
    "themselves a reason to choose hard. A specialty may not offer every "
    "difficulty: when the fitting one is missing, take the closest difficulty of the same "
    "specialty rather than another specialty; when the missing one lies between two that are "
    "offered, take the lower of the two."
)
TIMEOUT_SECONDS = 20
MAX_RESPONSE_BYTES = 1_000_000
_MAX_DETAIL = 200


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args: Any, **_kwargs: Any) -> None:
        return None


def sanitize_error_text(text: str, secret: str | None = None) -> str:
    """Shorten error text and scrub any API key before returning it."""
    cleaned = text.replace(secret, "***") if secret else text
    return cleaned if len(cleaned) <= _MAX_DETAIL else cleaned[:_MAX_DETAIL] + "…"


def extract_error_message(detail: str) -> str:
    """Extract provider error.message / error / message from JSON, else return trimmed text."""
    try:
        document: object = json.loads(detail)
    except (ValueError, RecursionError):
        return detail.strip()
    if isinstance(document, dict):
        obj = cast(dict[str, object], document)
        error = obj.get("error")
        if isinstance(error, dict):
            message = cast(dict[str, object], error).get("message")
            if isinstance(message, str):
                return message
        elif isinstance(error, str):
            return error
        message = obj.get("message")
        if isinstance(message, str):
            return message
    return detail.strip()


def _http_error(
    exc: urllib.error.HTTPError,
    *,
    api_key: str | None,
    auth_on_400_api_key: bool,
) -> DecisionError:
    try:
        detail = exc.read(MAX_RESPONSE_BYTES).decode("utf-8", errors="replace")
    except (OSError, http.client.HTTPException):
        detail = ""
    message = sanitize_error_text(extract_error_message(detail), api_key)
    status = exc.code
    if status == 429:
        return DecisionError("rate_limited", f"The decision model is rate limited: {message}")
    if status in (401, 403) or (
        auth_on_400_api_key and status == 400 and "api key" in message.lower()
    ):
        return DecisionError("auth", f"The decision model rejected the API key: {message}")
    return DecisionError("rejected", f"The decision model refused (HTTP {status}): {message}")


def _send_once(
    endpoint: str,
    data: bytes,
    headers: Mapping[str, str],
    *,
    timeout: float,
    api_key: str | None,
    auth_on_400_api_key: bool,
) -> bytes:
    parsed = urllib.parse.urlsplit(endpoint)
    handlers: list[urllib.request.BaseHandler] = (
        [urllib.request.ProxyHandler({}), NoRedirect()]
        if parsed.hostname and is_loopback_host(parsed.hostname)
        else [NoRedirect()]
    )
    try:
        request = urllib.request.Request(endpoint, data=data, headers=dict(headers), method="POST")
        with urllib.request.build_opener(*handlers).open(request, timeout=timeout) as response:
            raw = response.read(MAX_RESPONSE_BYTES + 1)
            if len(raw) > MAX_RESPONSE_BYTES:
                raise DecisionError("bad_response", "The decision model reply is too large.")
            remaining = getattr(response, "length", None)
            if isinstance(remaining, int) and remaining > 0:
                raise http.client.IncompleteRead(raw, remaining)
            return raw
    except urllib.error.HTTPError as exc:
        raise _http_error(exc, api_key=api_key, auth_on_400_api_key=auth_on_400_api_key) from None
    except (OSError, http.client.HTTPException) as exc:
        reason = getattr(exc, "reason", exc)
        raise DecisionError(
            "network",
            f"The decision model is unreachable: {sanitize_error_text(str(reason), api_key)}",
        ) from None
    except ValueError:
        # What the HTTP library quotes of a bad header value could be the key itself.
        raise DecisionError("network", "The decision model request could not be sent.") from None


def post_json(
    endpoint: str,
    document: dict[str, Any],
    headers: Mapping[str, str],
    *,
    timeout: float = TIMEOUT_SECONDS,
    api_key: str | None = None,
    auth_on_400_api_key: bool = False,
) -> object:
    """POST JSON with stdlib urllib, no redirects, loopback proxy bypass, and 1 network retry."""
    data = json.dumps(document).encode()
    try:
        raw = _send_once(
            endpoint,
            data,
            headers,
            timeout=timeout,
            api_key=api_key,
            auth_on_400_api_key=auth_on_400_api_key,
        )
    except DecisionError as exc:
        if exc.code != "network":
            raise
        raw = _send_once(
            endpoint,
            data,
            headers,
            timeout=timeout,
            api_key=api_key,
            auth_on_400_api_key=auth_on_400_api_key,
        )
    try:
        body: object = json.loads(raw)
    except (ValueError, RecursionError):
        raise DecisionError("bad_response", "The decision model reply is not JSON.") from None
    return body


def batch_question_ids(count: int) -> list[str]:
    return [f"{QUESTION}{index}" for index in range(1, count + 1)]


def format_batch_input(ids: Sequence[str], questions: Sequence[DecisionQuestion]) -> str:
    return "\n\n".join(f"[{key}]\n{one.text}" for key, one in zip(ids, questions, strict=True))


def batch_instruction(key: str) -> str:
    return f"{INSTRUCTIONS} Decide only for the task marked [{key}]."


def finite_number(value: object) -> float | None:
    """Return value as a finite float, or None for bool, non-numeric, NaN, Inf, or overflow."""
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return None
    try:
        number = float(value)
    except OverflowError:
        return None
    return number if math.isfinite(number) else None


def require_object(value: object, key: str) -> dict[str, Any]:
    found = cast(dict[str, Any], value).get(key) if isinstance(value, dict) else None
    if not isinstance(found, dict):
        raise DecisionError("bad_response", f"The decision model reply has no {key} object.")
    return cast(dict[str, Any], found)


def validate_choice(answer: Mapping[str, Any], candidates: Mapping[str, str]) -> str:
    choice = answer.get("choice")
    if not isinstance(choice, str) or choice not in candidates:
        raise DecisionError("bad_response", "The decision model chose no listed seat.")
    return choice


def parse_confidence(answer: Mapping[str, Any]) -> float | None:
    confidence = answer.get("confidence")
    if confidence is None:
        return None
    parsed = finite_number(confidence)
    if parsed is None:
        raise DecisionError("bad_response", "The decision model returned an unreadable confidence.")
    return parsed


def parse_probability_dict(
    answer: Mapping[str, Any], candidates: Mapping[str, str]
) -> dict[str, float]:
    probabilities = answer.get("probabilities")
    if probabilities is None:
        return {}
    if not isinstance(probabilities, dict):
        raise DecisionError("bad_response", "The decision model returned unreadable probabilities.")
    result: dict[str, float] = {}
    for key, value in cast(dict[object, object], probabilities).items():
        number = finite_number(value)
        if not isinstance(key, str) or number is None:
            raise DecisionError(
                "bad_response", "The decision model returned unreadable probabilities."
            )
        if key in candidates:
            result[key] = number
    return result


def parse_keyed_answer(answer: dict[str, Any], candidates: Mapping[str, str]) -> Decision:
    choice = validate_choice(answer, candidates)
    probs = parse_probability_dict(answer, candidates)
    confidence = parse_confidence(answer)
    return Decision(choice, probs, confidence)


def parse_keyed_response(body: object, candidates: Mapping[str, str]) -> Decision:
    return parse_keyed_answer(require_object(require_object(body, "answers"), QUESTION), candidates)


def parse_keyed_batch_response(
    body: object, questions: Sequence[DecisionQuestion]
) -> list[Decision | DecisionError]:
    answers = require_object(body, "answers")
    results: list[Decision | DecisionError] = []
    for key, one in zip(batch_question_ids(len(questions)), questions, strict=True):
        try:
            results.append(parse_keyed_answer(require_object(answers, key), one.candidates))
        except DecisionError as exc:
            results.append(exc)
    return results


def build_keyed_request(
    model: str,
    text: str,
    candidates: Mapping[str, str],
    *,
    text_key: str,
) -> dict[str, Any]:
    return {
        "model": model,
        text_key: text,
        "questions": {
            QUESTION: {
                "type": "choice",
                "instructions": INSTRUCTIONS,
                "criteria": dict(candidates),
            }
        },
    }


def build_keyed_batch_request(
    model: str,
    questions: Sequence[DecisionQuestion],
    *,
    text_key: str,
) -> dict[str, Any]:
    ids = batch_question_ids(len(questions))
    return {
        "model": model,
        text_key: format_batch_input(ids, questions),
        "questions": {
            key: {
                "type": "choice",
                "instructions": batch_instruction(key),
                "criteria": dict(one.candidates),
            }
            for key, one in zip(ids, questions, strict=True)
        },
    }

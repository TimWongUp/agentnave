"""OpenAI Decisions API client for gpt-6-luna (ADR-0123)."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any, cast

from agentnave.decision_http import (
    INSTRUCTIONS,
    QUESTION,
    TIMEOUT_SECONDS,
    batch_instruction,
    batch_question_ids,
    finite_number,
    format_batch_input,
    parse_confidence,
    post_json,
    validate_choice,
)
from agentnave.decision_model import Decision, DecisionError, DecisionQuestion
from agentnave.decision_sources import OPENAI_MODEL

ENDPOINT = "https://api.openai.com/v1/decisions"


def _choices(candidates: Mapping[str, str]) -> list[dict[str, str]]:
    return [{"value": key, "description": desc} for key, desc in candidates.items()]


def build_request(
    text: str, candidates: Mapping[str, str], *, model: str = OPENAI_MODEL
) -> dict[str, Any]:
    """One OpenAI Decisions choice question."""
    return {
        "model": model,
        "input": text,
        "questions": [
            {
                "type": "choice",
                "name": QUESTION,
                "instructions": INSTRUCTIONS,
                "choices": _choices(candidates),
            }
        ],
    }


def build_batch_request(
    questions: Sequence[DecisionQuestion], *, model: str = OPENAI_MODEL
) -> dict[str, Any]:
    """One OpenAI Decisions request with an array of named choice questions."""
    ids = batch_question_ids(len(questions))
    return {
        "model": model,
        "input": format_batch_input(ids, questions),
        "questions": [
            {
                "type": "choice",
                "name": key,
                "instructions": batch_instruction(key),
                "choices": _choices(one.candidates),
            }
            for key, one in zip(ids, questions, strict=True)
        ],
    }


def _answers_list(body: object) -> list[object]:
    answers = cast(dict[str, object], body).get("answers") if isinstance(body, dict) else None
    if not isinstance(answers, list):
        raise DecisionError("bad_response", "The decision model reply has no answers list.")
    return cast(list[object], answers)


def _parse_probabilities(
    answer: Mapping[str, Any], candidates: Mapping[str, str]
) -> dict[str, float]:
    raw = answer.get("probabilities")
    if raw is None:
        return {}
    if not isinstance(raw, list):
        raise DecisionError("bad_response", "The decision model returned unreadable probabilities.")
    result: dict[str, float] = {}
    for entry in cast(list[object], raw):
        if not isinstance(entry, dict):
            raise DecisionError(
                "bad_response", "The decision model returned unreadable probabilities."
            )
        item = cast(dict[str, object], entry)
        value = item.get("value")
        prob = finite_number(item.get("probability"))
        if not isinstance(value, str) or prob is None:
            raise DecisionError(
                "bad_response", "The decision model returned unreadable probabilities."
            )
        if value in candidates:
            result[value] = prob
    return result


def _parse_answer(answer: dict[str, Any], candidates: Mapping[str, str]) -> Decision:
    if answer.get("type") == "refusal":
        raise DecisionError("bad_response", "The decision model refused to choose a seat.")
    choice = validate_choice(answer, candidates)
    probs = _parse_probabilities(answer, candidates)
    confidence = parse_confidence(answer)
    return Decision(choice, probs, confidence)


def _match_answer(
    answers: Sequence[object],
    by_name: Mapping[str, dict[str, Any]],
    key: str,
    index: int,
) -> dict[str, Any]:
    found = by_name.get(key)
    if found is not None:
        return found
    if index < len(answers) and isinstance(answers[index], dict):
        positional = cast(dict[str, Any], answers[index])
        if positional.get("name") is None:
            return positional
    raise DecisionError("bad_response", f"The decision model reply has no {key} answer.")


def _index_named_answers(answers: Sequence[object]) -> dict[str, dict[str, Any]]:
    by_name: dict[str, dict[str, Any]] = {}
    for entry in answers:
        if isinstance(entry, dict):
            item = cast(dict[str, Any], entry)
            name = item.get("name")
            if isinstance(name, str):
                by_name.setdefault(name, item)
    return by_name


def parse_response(body: object, candidates: Mapping[str, str]) -> Decision:
    """Parse answers array matching name='seat' or falling back to position 0."""
    answers = _answers_list(body)
    by_name = _index_named_answers(answers)
    return _parse_answer(_match_answer(answers, by_name, QUESTION, 0), candidates)


def parse_batch_response(
    body: object, questions: Sequence[DecisionQuestion]
) -> list[Decision | DecisionError]:
    """Parse answers array by question name (or position if name omitted); bad ones fail alone."""
    answers = _answers_list(body)
    by_name = _index_named_answers(answers)
    results: list[Decision | DecisionError] = []
    for index, (key, one) in enumerate(
        zip(batch_question_ids(len(questions)), questions, strict=True)
    ):
        try:
            answer = _match_answer(answers, by_name, key, index)
            results.append(_parse_answer(answer, one.candidates))
        except DecisionError as exc:
            results.append(exc)
    return results


class OpenAIDecisionClient:
    """OpenAI Decisions API client using Bearer authorization."""

    def __init__(
        self,
        api_key: str,
        model: str = OPENAI_MODEL,
        endpoint: str = ENDPOINT,
        timeout: float = TIMEOUT_SECONDS,
    ) -> None:
        self._key = api_key
        self._model = model
        self._endpoint = endpoint
        self._timeout = timeout

    def _headers(self) -> dict[str, str]:
        return {"content-type": "application/json", "authorization": f"Bearer {self._key}"}

    def decide(self, text: str, candidates: Mapping[str, str]) -> Decision:
        body = post_json(
            self._endpoint,
            build_request(text, candidates, model=self._model),
            self._headers(),
            timeout=self._timeout,
            api_key=self._key,
        )
        return parse_response(body, candidates)

    def decide_many(self, questions: Sequence[DecisionQuestion]) -> list[Decision | DecisionError]:
        body = post_json(
            self._endpoint,
            build_batch_request(questions, model=self._model),
            self._headers(),
            timeout=self._timeout,
            api_key=self._key,
        )
        return parse_batch_response(body, questions)

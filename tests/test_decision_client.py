from __future__ import annotations

import json
import threading
import time
from collections.abc import Callable, Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

from agentnave.decision_gemini import (
    ENDPOINT,
    GeminiDecisionClient,
    build_batch_request,
    build_request,
    parse_response,
)
from agentnave.decision_model import (
    Decision,
    DecisionError,
    DecisionKeysFileError,
    DecisionQuestion,
    read_decision_key,
)

KEY = "AIza-secret-key"
CANDIDATES = {"a": "role a | tags: coding", "b": "role b | tags: writing"}
GOOD = {
    "answers": {"seat": {"choice": "b", "probabilities": {"a": 0.2, "b": 0.8}, "confidence": 0.9}}
}

type Reply = tuple[int, dict[str, str], bytes]


class _Endpoint:
    """A local stand-in for the decision API; nothing here leaves the machine."""

    def __init__(self) -> None:
        self.requests: list[tuple[str, dict[str, str], Any]] = []
        self.reply: Callable[[], Reply] = lambda: (200, {}, json.dumps(GOOD).encode())
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = self.rfile.read(int(self.headers["content-length"]))
                headers = {key.lower(): value for key, value in self.headers.items()}
                owner.requests.append((self.path, headers, json.loads(body)))
                status, extra, payload = owner.reply()
                self.send_response(status)
                for key, value in extra.items():
                    self.send_header(key, value)
                self.send_header("content-length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, format: str, *args: Any) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}/v1beta/decisions"

    def client(self, timeout: float = 5) -> GeminiDecisionClient:
        return GeminiDecisionClient(KEY, self.url, timeout)


@pytest.fixture
def endpoint(monkeypatch: pytest.MonkeyPatch) -> Iterator[_Endpoint]:
    # A configured proxy must not carry the local calls.
    monkeypatch.setenv("NO_PROXY", "127.0.0.1")
    monkeypatch.setenv("no_proxy", "127.0.0.1")
    found = _Endpoint()
    yield found
    found.server.shutdown()
    found.server.server_close()


def _not_json() -> Reply:
    return 200, {}, b"not json"


def _error(status: int, message: str, **headers: str) -> Callable[[], Reply]:
    body = json.dumps({"error": {"code": status, "message": message}}).encode()
    return lambda: (status, headers, body)


def test_request_and_response_follow_the_documented_formats(endpoint: _Endpoint) -> None:
    assert ENDPOINT == "https://generativelanguage.googleapis.com/v1beta/decisions"
    decision = endpoint.client().decide("Task: add a switch", CANDIDATES)
    assert decision == Decision("b", {"a": 0.2, "b": 0.8}, 0.9)
    [(path, headers, body)] = endpoint.requests
    assert path == "/v1beta/decisions"
    assert headers["x-goog-api-key"] == KEY and headers["content-type"] == "application/json"
    assert body == {
        "model": "gemini-decision-1",
        "input": "Task: add a switch",
        "questions": {
            "seat": {
                "type": "choice",
                "instructions": body["questions"]["seat"]["instructions"],
                "criteria": CANDIDATES,
            }
        },
    }
    assert KEY not in json.dumps(body)
    assert build_request("Task: add a switch", CANDIDATES) == body


def test_several_tasks_go_in_one_request_with_one_question_each(endpoint: _Endpoint) -> None:
    other = {"c": "role c | tags: review"}
    questions = [DecisionQuestion("Task: one", CANDIDATES), DecisionQuestion("Task: two", other)]
    endpoint.reply = lambda: (
        200,
        {},
        json.dumps(
            {
                "answers": {
                    "seat1": {
                        "choice": "a",
                        "probabilities": {"a": 0.7, "b": 0.3},
                        "confidence": 0.7,
                    },
                    "seat2": {"choice": "c"},
                }
            }
        ).encode(),
    )
    assert endpoint.client().decide_many(questions) == [
        Decision("a", {"a": 0.7, "b": 0.3}, 0.7),
        Decision("c"),
    ]
    [(path, headers, body)] = endpoint.requests
    assert path == "/v1beta/decisions" and headers["x-goog-api-key"] == KEY
    assert body == build_batch_request(questions)
    assert set(body["questions"]) == {"seat1", "seat2"}
    assert [body["questions"][key]["criteria"] for key in ("seat1", "seat2")] == [CANDIDATES, other]
    assert "[seat1]\nTask: one" in body["input"] and "[seat2]\nTask: two" in body["input"]
    assert KEY not in json.dumps(body)


def test_a_bad_answer_fails_only_its_own_task(endpoint: _Endpoint) -> None:
    questions = [DecisionQuestion("one", CANDIDATES), DecisionQuestion("two", CANDIDATES)]
    for answers in (
        {"seat1": {"choice": "z"}, "seat2": {"choice": "b"}},
        {"seat2": {"choice": "b"}},
    ):
        endpoint.reply = lambda answers=answers: (
            200,
            {},
            json.dumps({"answers": answers}).encode(),
        )
        first, second = endpoint.client().decide_many(questions)
        assert isinstance(first, DecisionError) and first.code == "bad_response"
        assert second == Decision("b")


def test_a_failed_batch_request_fails_as_a_whole_and_is_not_retried(endpoint: _Endpoint) -> None:
    questions = [DecisionQuestion("one", CANDIDATES), DecisionQuestion("two", CANDIDATES)]
    endpoint.reply = _error(429, "quota exceeded")
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide_many(questions)
    assert raised.value.code == "rate_limited" and len(endpoint.requests) == 1
    endpoint.reply = lambda: (200, {}, b"{}")
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide_many(questions)
    assert raised.value.code == "bad_response"


@pytest.mark.parametrize(
    "body",
    [
        {},
        {"answers": {}},
        {"answers": {"seat": {"choice": "z"}}},
        {"answers": {"seat": {"choice": 3}}},
        {"answers": {"seat": {"choice": "a", "probabilities": [0.5, 0.5]}}},
        {"answers": {"seat": {"choice": "a", "probabilities": {"a": "high"}}}},
        {"answers": {"seat": {"choice": "a", "confidence": "high"}}},
        [],
    ],
)
def test_unexpected_responses_are_bad_responses(body: object) -> None:
    with pytest.raises(DecisionError) as raised:
        parse_response(body, CANDIDATES)
    assert raised.value.code == "bad_response"


def test_a_missing_probabilities_or_confidence_is_tolerated() -> None:
    assert parse_response({"answers": {"seat": {"choice": "a"}}}, CANDIDATES) == Decision("a")


@pytest.mark.parametrize(
    ("reply", "code"),
    [
        (_error(429, "quota exceeded"), "rate_limited"),
        (_error(401, "unauthorized"), "auth"),
        (_error(403, "forbidden"), "auth"),
        # Google reports a wrong key as 400, and an unsupported region as a refusal.
        (_error(400, "API key not valid. Please pass a valid API key."), "auth"),
        (_error(400, "User location is not supported for the API use."), "rejected"),
        (_error(500, "internal"), "rejected"),
        (_not_json, "bad_response"),
    ],
)
def test_http_failures_are_told_apart_and_never_retried(
    endpoint: _Endpoint, reply: Callable[[], Reply], code: str
) -> None:
    endpoint.reply = reply
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide("task", CANDIDATES)
    assert raised.value.code == code
    assert len(endpoint.requests) == 1
    if code == "rejected" and "location" in raised.value.message:
        assert "not supported" in raised.value.message


def test_redirects_are_not_followed(endpoint: _Endpoint) -> None:
    other = _Endpoint()
    try:
        endpoint.reply = lambda: (302, {"location": other.url}, b"")
        with pytest.raises(DecisionError) as raised:
            endpoint.client().decide("task", CANDIDATES)
        assert raised.value.code == "rejected" and "302" in raised.value.message
        assert other.requests == []
    finally:
        other.server.shutdown()
        other.server.server_close()


def test_error_details_are_truncated_and_never_hold_the_key(endpoint: _Endpoint) -> None:
    endpoint.reply = _error(400, f"bad request {KEY} " + "x" * 5000)
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide("task", CANDIDATES)
    assert KEY not in raised.value.message and "***" in raised.value.message
    assert len(raised.value.message) < 400


def test_unreachable_and_slow_endpoints_are_network_errors(endpoint: _Endpoint) -> None:
    def slow() -> Reply:
        time.sleep(1)
        return 200, {}, json.dumps(GOOD).encode()

    endpoint.reply = slow
    with pytest.raises(DecisionError) as raised:
        endpoint.client(timeout=0.2).decide("task", CANDIDATES)
    assert raised.value.code == "network" and KEY not in raised.value.message
    url = endpoint.url
    endpoint.server.shutdown()
    endpoint.server.server_close()
    with pytest.raises(DecisionError) as refused:
        GeminiDecisionClient(KEY, url, 1).decide("task", CANDIDATES)
    assert refused.value.code == "network" and KEY not in refused.value.message


def test_decision_keys_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    path = tmp_path / "decision-keys.json"
    assert read_decision_key("gemini") is None
    path.write_text(json.dumps({"version": 1, "keys": {"gemini": f"  {KEY}\n", "other": "o"}}))
    assert read_decision_key("gemini") == KEY and read_decision_key("missing") is None
    path.write_text(json.dumps({"version": 1, "keys": {"gemini": " "}}))
    assert read_decision_key("gemini") is None
    path.write_text(json.dumps({"version": 1}))
    with pytest.raises(DecisionKeysFileError, match="Invalid decision-keys.json"):
        read_decision_key("gemini")
    for content in (
        "{" + KEY,
        json.dumps({"version": 2, "keys": {"gemini": KEY}}),
        json.dumps({"version": 1, "keys": {"gemini": KEY, "bad": 7}}),
        json.dumps({"version": True, "keys": {"gemini": KEY}}),
        json.dumps([KEY]),
    ):
        path.write_text(content)
        with pytest.raises(DecisionKeysFileError) as raised:
            read_decision_key("gemini")
        assert KEY not in str(raised.value)


@pytest.mark.parametrize(
    "key", [f"{KEY}\nX-Evil: 1", f"{KEY}\r\nmore", "AIza secret", "AIza\tsecret", "密钥AIza"]
)
def test_a_key_that_is_not_visible_ascii_is_a_config_error_that_never_quotes_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, key: str
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    (tmp_path / "decision-keys.json").write_text(
        json.dumps({"version": 1, "keys": {"gemini": key}})
    )
    with pytest.raises(DecisionKeysFileError) as raised:
        read_decision_key("gemini")
    text = str(raised.value)
    assert "decision-keys.json" in text and "not allowed" in text
    assert not any(part in text for part in ("AIza", "secret", "Evil", "more", "密钥"))


def test_a_key_with_a_line_break_given_to_the_client_fails_without_quoting_it(
    endpoint: _Endpoint,
) -> None:
    client = GeminiDecisionClient(f"{KEY}\nX-Evil: 1", endpoint.url, 5)
    for call in (
        lambda: client.decide("task", CANDIDATES),
        lambda: client.decide_many([DecisionQuestion("task", CANDIDATES)]),
    ):
        with pytest.raises(DecisionError) as raised:
            call()
        assert raised.value.code == "network"
        assert not any(part in raised.value.message for part in ("AIza", "Evil", "secret"))
        assert raised.value.__cause__ is None and raised.value.__suppress_context__
    assert endpoint.requests == []


@pytest.mark.parametrize(
    "confidence", ["1" + "0" * 400, "NaN", "Infinity", "-Infinity", "true", '"high"']
)
def test_a_confidence_or_probability_that_is_not_a_finite_number_is_a_bad_response(
    endpoint: _Endpoint, confidence: str
) -> None:
    def reply(answer: str) -> Callable[[], Reply]:
        return lambda: (200, {}, f'{{"answers": {{"seat": {answer}, "seat1": {answer}}}}}'.encode())

    for answer in (
        f'{{"choice": "a", "confidence": {confidence}}}',
        f'{{"choice": "a", "probabilities": {{"a": {confidence}}}}}',
    ):
        endpoint.reply = reply(answer)
        with pytest.raises(DecisionError) as raised:
            endpoint.client().decide("task", CANDIDATES)
        assert raised.value.code == "bad_response"
        # In a batch it fails the one question that answered so.
        [only] = endpoint.client().decide_many([DecisionQuestion("task", CANDIDATES)])
        assert isinstance(only, DecisionError) and only.code == "bad_response"
    # A number as large as a float allows is still a number.
    endpoint.reply = reply('{"choice": "a", "confidence": 1e300}')
    assert endpoint.client().decide("task", CANDIDATES).confidence == 1e300


def test_a_deeply_nested_reply_is_a_bad_response(endpoint: _Endpoint) -> None:
    nested = "[" * 100_000
    endpoint.reply = lambda: (200, {}, nested.encode())
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide("task", CANDIDATES)
    assert raised.value.code == "bad_response"
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide_many([DecisionQuestion("task", CANDIDATES)])
    assert raised.value.code == "bad_response"
    # The same inside an error body only changes what the message says.
    endpoint.reply = lambda: (429, {}, nested.encode())
    with pytest.raises(DecisionError) as raised:
        endpoint.client().decide("task", CANDIDATES)
    assert raised.value.code == "rate_limited"

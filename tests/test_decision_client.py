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
    assert len(endpoint.requests) == 2  # one retry, then the error
    url = endpoint.url
    endpoint.server.shutdown()
    endpoint.server.server_close()
    with pytest.raises(DecisionError) as refused:
        GeminiDecisionClient(KEY, url, 1).decide("task", CANDIDATES)
    assert refused.value.code == "network" and KEY not in refused.value.message


def test_one_network_failure_is_retried(endpoint: _Endpoint) -> None:
    def slow_once() -> Reply:
        if len(endpoint.requests) == 1:
            time.sleep(1)
        return 200, {}, json.dumps(GOOD).encode()

    endpoint.reply = slow_once
    assert endpoint.client(timeout=0.2).decide("task", CANDIDATES).choice == "b"
    assert len(endpoint.requests) == 2


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


def test_loopback_calls_bypass_configured_http_proxy(monkeypatch: pytest.MonkeyPatch) -> None:
    from agentnave.decision_systemone import SystemOneDecisionClient

    monkeypatch.setenv("HTTP_PROXY", "http://127.0.0.1:1")
    monkeypatch.setenv("HTTPS_PROXY", "http://127.0.0.1:1")
    monkeypatch.delenv("NO_PROXY", raising=False)
    monkeypatch.delenv("no_proxy", raising=False)

    found = _Endpoint()
    try:
        client = SystemOneDecisionClient(found.url, "qwen3:0.6b", api_key=None, timeout=5)
        assert client.decide("task", CANDIDATES).choice == "b"
        assert len(found.requests) == 1
        assert "authorization" not in found.requests[0][1]
    finally:
        found.server.shutdown()
        found.server.server_close()


def test_systemone_client_single_batch_omitted_probabilities_and_errors(
    endpoint: _Endpoint,
) -> None:
    from agentnave.decision_systemone import (
        OPENROUTER_ENDPOINT,
        TYPESAFE_ENDPOINT,
        SystemOneDecisionClient,
        build_batch_request,
        build_request,
        resolve_systemone_endpoint,
    )

    assert TYPESAFE_ENDPOINT == "https://api.typesafe.ai/v1/systemone"
    assert OPENROUTER_ENDPOINT == "https://openrouter.ai/api/v1/systemone"
    assert resolve_systemone_endpoint("http://127.0.0.1:11434") == (
        "http://127.0.0.1:11434/v1/systemone"
    )
    assert resolve_systemone_endpoint("http://127.0.0.1:11434/") == (
        "http://127.0.0.1:11434/v1/systemone"
    )
    assert resolve_systemone_endpoint("https://host.test/v1") == "https://host.test/v1/systemone"
    assert (
        resolve_systemone_endpoint("https://host.test/v1beta/")
        == "https://host.test/v1beta/systemone"
    )
    assert (
        resolve_systemone_endpoint("https://host.test/api/v1/systemone/")
        == "https://host.test/api/v1/systemone"
    )

    # Omitted probabilities (e.g. OpenRouter models) and null probabilities are tolerated.
    endpoint.reply = lambda: (200, {}, json.dumps({"answers": {"seat": {"choice": "a"}}}).encode())
    client = SystemOneDecisionClient(endpoint.url, "liquid/d1.2-ultra", api_key="or-secret")
    assert client.decide("Task: one", CANDIDATES) == Decision("a", {}, None)
    [(path, headers, body)] = endpoint.requests
    assert path == "/v1beta/decisions"
    assert headers["authorization"] == "Bearer or-secret"
    assert headers["content-type"] == "application/json"
    assert body == build_request("liquid/d1.2-ultra", "Task: one", CANDIDATES)
    assert "state" in body and "input" not in body

    endpoint.reply = lambda: (
        200,
        {},
        json.dumps({"answers": {"seat": {"choice": "b", "probabilities": None}}}).encode(),
    )
    assert client.decide("Task: two", CANDIDATES) == Decision("b", {}, None)

    # Batch request with partial failure.
    other = {"c": "role c"}
    questions = [DecisionQuestion("One", CANDIDATES), DecisionQuestion("Two", other)]
    endpoint.reply = lambda: (
        200,
        {},
        json.dumps(
            {
                "answers": {
                    "seat1": {"choice": "z"},
                    "seat2": {"choice": "c", "probabilities": {"c": 0.9}, "confidence": 0.9},
                }
            }
        ).encode(),
    )
    first, second = client.decide_many(questions)
    assert isinstance(first, DecisionError) and first.code == "bad_response"
    assert second == Decision("c", {"c": 0.9}, 0.9)
    assert endpoint.requests[-1][2] == build_batch_request("liquid/d1.2-ultra", questions)

    # Ollama-style error body and secret scrubbing (400 with "api key" is rejected, not Google auth).
    endpoint.reply = lambda: (400, {}, json.dumps({"error": "bad api key or-secret"}).encode())
    with pytest.raises(DecisionError) as err400:
        client.decide("task", CANDIDATES)
    assert err400.value.code == "rejected"
    assert "or-secret" not in err400.value.message and "***" in err400.value.message


def test_openai_decisions_client_arrays_refusal_probabilities_and_batch(
    endpoint: _Endpoint,
) -> None:
    from agentnave.decision_openai import (
        ENDPOINT as OPENAI_ENDPOINT,
    )
    from agentnave.decision_openai import (
        OpenAIDecisionClient,
        build_batch_request,
        build_request,
        parse_batch_response,
        parse_response,
    )
    from agentnave.decision_sources import OPENAI_MODEL

    assert OPENAI_ENDPOINT == "https://api.openai.com/v1/decisions"
    assert OPENAI_MODEL == "gpt-6-luna"

    client = OpenAIDecisionClient("sk-openai-secret", endpoint=endpoint.url, timeout=5)
    endpoint.reply = lambda: (
        200,
        {},
        json.dumps(
            {
                "answers": [
                    {
                        "type": "choice",
                        "name": "seat",
                        "choice": "b",
                        "probabilities": [
                            {"value": "a", "probability": 0.2},
                            {"value": "b", "probability": 0.8},
                        ],
                        "confidence": 0.85,
                    }
                ]
            }
        ).encode(),
    )
    decision = client.decide("Task: fix", CANDIDATES)
    assert decision == Decision("b", {"a": 0.2, "b": 0.8}, 0.85)
    [(_, headers, body)] = endpoint.requests
    assert headers["authorization"] == "Bearer sk-openai-secret"
    assert body == build_request("Task: fix", CANDIDATES)
    assert body["questions"][0]["choices"] == [
        {"value": "a", "description": CANDIDATES["a"]},
        {"value": "b", "description": CANDIDATES["b"]},
    ]

    # Positional fallback when name is omitted, and missing probabilities allowed.
    assert parse_response({"answers": [{"choice": "a"}]}, CANDIDATES) == Decision("a", {}, None)

    # Refusal and invalid probability items become bad_response.
    bad_bodies: list[object] = [
        {"answers": [{"name": "seat", "type": "refusal", "refusal": "cannot decide"}]},
        {"answers": [{"name": "other", "choice": "a"}]},
        {"answers": [{"name": "seat", "choice": "a", "probabilities": {"a": 0.5}}]},
        {"answers": [{"name": "seat", "choice": "a", "probabilities": [{"value": "a"}]}]},
        {
            "answers": [
                {
                    "name": "seat",
                    "choice": "a",
                    "probabilities": [{"value": "a", "probability": float("nan")}],
                }
            ]
        },
        {"answers": {}},
    ]
    for bad in bad_bodies:
        with pytest.raises(DecisionError) as raised:
            parse_response(bad, CANDIDATES)
        assert raised.value.code == "bad_response"

    # Batch matches by name even out of order; refusal/missing fails only its slot.
    other = {"c": "role c"}
    questions = [DecisionQuestion("One", CANDIDATES), DecisionQuestion("Two", other)]
    endpoint.reply = lambda: (
        200,
        {},
        json.dumps(
            {
                "answers": [
                    {
                        "name": "seat2",
                        "choice": "c",
                        "probabilities": [{"value": "c", "probability": 1.0}],
                    },
                    {"name": "seat1", "type": "refusal"},
                ]
            }
        ).encode(),
    )
    first, second = client.decide_many(questions)
    assert isinstance(first, DecisionError) and first.code == "bad_response"
    assert second == Decision("c", {"c": 1.0}, None)
    assert endpoint.requests[-1][2] == build_batch_request(questions)

    # Positional fallback in batch when names are omitted.
    assert parse_batch_response({"answers": [{"choice": "a"}, {"choice": "c"}]}, questions) == [
        Decision("a"),
        Decision("c"),
    ]


def test_client_for_config_key_policies_across_providers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from agentnave.decision_gemini import GeminiDecisionClient
    from agentnave.decision_openai import OpenAIDecisionClient
    from agentnave.decision_systemone import SystemOneDecisionClient
    from agentnave.seat_recommendation import (
        DecisionModelConfig,
        RecommendationError,
        client_for_config,
    )

    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))

    # ollama does not read decision-keys.json even if missing or broken.
    (tmp_path / "decision-keys.json").write_text("{broken")
    ollama_client = client_for_config(
        DecisionModelConfig("ollama", "qwen3:0.6b", "http://127.0.0.1:11434")
    )
    assert isinstance(ollama_client, SystemOneDecisionClient)

    # custom reports config_error when decision-keys.json is broken, but works when missing.
    with pytest.raises(RecommendationError) as custom_broken:
        client_for_config(DecisionModelConfig("custom", "m", "https://example.com"))
    assert custom_broken.value.code == "config_error"

    (tmp_path / "decision-keys.json").unlink()
    custom_nokey = client_for_config(DecisionModelConfig("custom", "m", "https://example.com"))
    assert isinstance(custom_nokey, SystemOneDecisionClient)

    for provider, model in (
        ("gemini", "gemini-decision-1"),
        ("openai", "gpt-6-luna"),
        ("typesafe", "jev-latest"),
        ("openrouter", "liquid/d1.2-ultra"),
    ):
        with pytest.raises(RecommendationError) as missing:
            client_for_config(DecisionModelConfig(provider, model, None))  # type: ignore[arg-type]
        assert missing.value.code == "no_key"

    (tmp_path / "decision-keys.json").write_text(
        json.dumps(
            {
                "version": 1,
                "keys": {
                    "gemini": "g-key",
                    "openai": "o-key",
                    "typesafe": "t-key",
                    "openrouter": "r-key",
                    "custom": "c-key",
                },
            }
        )
    )
    assert isinstance(
        client_for_config(DecisionModelConfig("gemini", "gemini-decision-1")),
        GeminiDecisionClient,
    )
    assert isinstance(
        client_for_config(DecisionModelConfig("openai", "gpt-6-luna")),
        OpenAIDecisionClient,
    )
    assert isinstance(
        client_for_config(DecisionModelConfig("typesafe", "jev-latest")),
        SystemOneDecisionClient,
    )
    assert isinstance(
        client_for_config(DecisionModelConfig("openrouter", "liquid/d1.2-ultra")),
        SystemOneDecisionClient,
    )
    assert isinstance(
        client_for_config(DecisionModelConfig("custom", "m", "https://example.com")),
        SystemOneDecisionClient,
    )


def test_resolve_systemone_endpoint_never_matches_host_suffix() -> None:
    from agentnave.decision_systemone import resolve_systemone_endpoint

    assert resolve_systemone_endpoint("https://systemone") == "https://systemone/v1/systemone"
    assert resolve_systemone_endpoint("https://v1") == "https://v1/v1/systemone"
    assert (
        resolve_systemone_endpoint("https://example.com/v1beta/")
        == "https://example.com/v1beta/systemone"
    )
    assert (
        resolve_systemone_endpoint("https://example.com/custom/systemone")
        == "https://example.com/custom/systemone"
    )


def test_truncated_http_200_body_triggers_single_network_retry() -> None:
    import socket
    import threading

    from agentnave.decision_systemone import SystemOneDecisionClient

    attempts = 0
    done = threading.Event()
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.bind(("127.0.0.1", 0))
    server.listen(4)
    port = server.getsockname()[1]

    def serve() -> None:
        nonlocal attempts
        while not done.is_set():
            server.settimeout(0.2)
            try:
                conn, _ = server.accept()
            except TimeoutError:
                continue
            with conn, conn.makefile("rb") as request:
                # Closing with any of the request unread resets the connection on Windows.
                length = 0
                while line := request.readline().strip():
                    name, _, value = line.partition(b":")
                    if name.lower() == b"content-length":
                        length = int(value)
                request.read(length)
                attempts += 1
                if attempts == 1:
                    # Declare full length, send truncated JSON, and close socket.
                    conn.sendall(
                        b"HTTP/1.1 200 OK\r\n"
                        b"Content-Type: application/json\r\n"
                        b"Content-Length: 200\r\n\r\n"
                        b'{"answers":{"seat":{"cho'
                    )
                else:
                    body = json.dumps({"answers": {"seat": {"choice": "a"}}}).encode()
                    conn.sendall(
                        b"HTTP/1.1 200 OK\r\n"
                        b"Content-Type: application/json\r\n"
                        + f"Content-Length: {len(body)}\r\n\r\n".encode()
                        + body
                    )

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    try:
        client = SystemOneDecisionClient(
            endpoint=f"http://127.0.0.1:{port}/v1/systemone", model="m", timeout=5
        )
        assert client.decide("Task", CANDIDATES) == Decision("a", {}, None)
        assert attempts == 2
    finally:
        done.set()
        thread.join(timeout=2)
        server.close()

from __future__ import annotations

import json
import threading
from collections.abc import Callable, Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest
from mcp.server.mcpserver.exceptions import ToolError

from agentnave import seat_recommendation
from agentnave.decision_model import Decision
from agentnave.t3_cache import InstanceAvailability
from agentnave.t3_recommend import T3RecommendTask, recommend_t3_seats
from agentnave.t3_seats import T3SeatsFile


def _t3_doc() -> T3SeatsFile:
    return T3SeatsFile.model_validate(
        {
            "version": 1,
            "enabled": True,
            "seats": [
                {
                    "id": "w-easy",
                    "kind": "worker",
                    "role": "通用实现",
                    "description": "简单修改",
                    "access": "write",
                    "difficulty": "easy",
                    "candidates": [{"instance": "i-1", "model": "m-1", "options": []}],
                },
                {
                    "id": "w-hard",
                    "kind": "worker",
                    "role": "通用实现",
                    "description": "复杂架构",
                    "access": "write",
                    "difficulty": "hard",
                    "candidates": [{"instance": "i-2", "model": "m-2", "options": []}],
                },
            ],
        }
    )


class _LocalSystemOne:
    def __init__(self) -> None:
        self.requests: list[tuple[str, dict[str, str], dict[str, Any]]] = []
        self.reply: Callable[[], tuple[int, dict[str, Any]]] = lambda: (
            200,
            {"answers": {"seat1": {"choice": "worker-hard"}}},
        )
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                body = json.loads(self.rfile.read(int(self.headers["content-length"])))
                headers = {k.lower(): v for k, v in self.headers.items()}
                owner.requests.append((self.path, headers, body))
                status, doc = owner.reply()
                raw = json.dumps(doc).encode()
                self.send_response(status)
                self.send_header("content-length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

            def log_message(self, format: str, *args: Any) -> None:
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}"


@pytest.fixture
def systemone() -> Iterator[_LocalSystemOne]:
    srv = _LocalSystemOne()
    yield srv
    srv.server.shutdown()
    srv.server.server_close()


@pytest.mark.asyncio
async def test_t3_recommend_uses_ollama_without_decision_keys(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, systemone: _LocalSystemOne
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    (tmp_path / "decision-seats.json").write_text(
        json.dumps(
            {
                "version": 1,
                "decision_provider": "ollama",
                "decision_model": "qwen3:0.6b",
                "decision_base_url": systemone.base_url,
            }
        )
    )
    # Even a broken decision-keys.json does not affect ollama.
    (tmp_path / "decision-keys.json").write_text("{broken")

    payload = await recommend_t3_seats(
        [T3RecommendTask(task="design architecture", access="write")],
        _t3_doc(),
        mode="decision",
        probe_instance=lambda _inst, _model: InstanceAvailability(available=True),
    )
    results = payload.get("results", [])
    assert len(results) == 1 and results[0].get("seat") == "worker-hard"
    [(path, headers, body)] = systemone.requests
    assert path == "/v1/systemone"
    assert "authorization" not in headers
    assert body["model"] == "qwen3:0.6b" and "state" in body


@pytest.mark.asyncio
async def test_t3_recommend_custom_optional_key_and_broken_config_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, systemone: _LocalSystemOne
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    (tmp_path / "decision-seats.json").write_text(
        json.dumps(
            {
                "version": 1,
                "decision_provider": "custom",
                "decision_model": "decider-v1",
                "decision_base_url": f"{systemone.base_url}/v1beta",
            }
        )
    )
    systemone.reply = lambda: (200, {"answers": {"seat1": {"choice": "worker-easy"}}})

    # Missing decision-keys.json succeeds without Authorization.
    payload = await recommend_t3_seats(
        [T3RecommendTask(task="small edit", access="write")],
        _t3_doc(),
        mode="decision",
        probe_instance=lambda _inst, _model: InstanceAvailability(available=True),
    )
    assert payload.get("results", [])[0].get("seat") == "worker-easy"
    assert systemone.requests[0][0] == "/v1beta/systemone"
    assert "authorization" not in systemone.requests[0][1]

    # Present custom key sends Bearer token.
    (tmp_path / "decision-keys.json").write_text(
        json.dumps({"version": 1, "keys": {"custom": "custom-secret"}})
    )
    await recommend_t3_seats(
        [T3RecommendTask(task="small edit", access="write")],
        _t3_doc(),
        mode="decision",
        probe_instance=lambda _inst, _model: InstanceAvailability(available=True),
    )
    assert systemone.requests[1][1]["authorization"] == "Bearer custom-secret"

    # Corrupted decision-keys.json raises config_error even for custom.
    (tmp_path / "decision-keys.json").write_text("{broken")
    with pytest.raises(ToolError, match="config_error"):
        await recommend_t3_seats(
            [T3RecommendTask(task="small edit", access="write")],
            _t3_doc(),
            mode="decision",
            probe_instance=lambda _inst, _model: InstanceAvailability(available=True),
        )

    # Corrupted decision-seats.json raises config_error instead of falling back to gemini.
    (tmp_path / "decision-seats.json").write_text(
        json.dumps({"version": 1, "decision_provider": "ollama"})
    )
    with pytest.raises(ToolError, match="config_error"):
        await recommend_t3_seats(
            [T3RecommendTask(task="small edit", access="write")],
            _t3_doc(),
            mode="decision",
            probe_instance=lambda _inst, _model: InstanceAvailability(available=True),
        )


@pytest.mark.asyncio
async def test_t3_recommend_passes_full_provider_config_for_cloud_providers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    seen: list[seat_recommendation.DecisionModelConfig] = []

    class _FakeClient:
        def decide(self, text: str, candidates: Any) -> Decision:
            raise AssertionError("T3 uses decide_many")

        def decide_many(self, questions: Any) -> list[Decision]:
            return [Decision("worker-easy") for _ in questions]

    def _fake_client_for_config(
        config: seat_recommendation.DecisionModelConfig,
    ) -> _FakeClient:
        seen.append(config)
        return _FakeClient()

    monkeypatch.setattr(seat_recommendation, "client_for_config", _fake_client_for_config)
    for provider in ("openai", "typesafe", "openrouter"):
        (tmp_path / "decision-seats.json").write_text(
            json.dumps({"version": 1, "decision_provider": provider})
        )
        payload = await recommend_t3_seats(
            [T3RecommendTask(task="small edit", access="write")],
            _t3_doc(),
            mode="decision",
            probe_instance=lambda _inst, _model: InstanceAvailability(available=True),
        )
        assert payload.get("results", [])[0].get("seat") == "worker-easy"
    assert seen == [
        seat_recommendation.DecisionModelConfig("openai", "gpt-6-luna", None),
        seat_recommendation.DecisionModelConfig("typesafe", "jev-latest", None),
        seat_recommendation.DecisionModelConfig("openrouter", "liquid/d1.2-ultra", None),
    ]

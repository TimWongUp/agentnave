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
from agentnave.t3_recommend import (
    T3_CHILD_IDENTITY_SUFFIX,
    T3_STAGED_NEXT,
    T3_STAGED_NOTE,
    T3_STAGED_READ_ONLY_INSTRUCTIONS,
    T3_WRITE_NEXT,
    T3RecommendTask,
    antigravity_delegate_next,
    normalize_t3_recommend_args,
    recommend_t3_seats,
)
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


@pytest.mark.asyncio
async def test_t3_staged_may_implement_validation_selection_and_receipts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("AGENTNAVE_DATA_DIR", str(tmp_path))
    doc = T3SeatsFile.model_validate(
        {
            "version": 1,
            "enabled": True,
            "seats": [
                {
                    "id": "r-easy",
                    "kind": "explorer",
                    "role": "探索",
                    "description": "只读排查",
                    "access": "read",
                    "difficulty": "easy",
                    "candidates": [{"instance": "i-r", "model": "m-r", "options": []}],
                },
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
                {
                    "id": "art",
                    "kind": "image",
                    "role": "插画",
                    "description": "生成插画",
                    "access": "write",
                    "candidates": [{"instance": "i-img", "model": "m-img", "options": []}],
                },
            ],
            "review": {
                "correctness-reviewer": [{"instance": "i-rev", "model": "m-rev", "options": []}]
            },
        }
    )

    def ok(_inst: str, _model: str) -> InstanceAvailability:
        return InstanceAvailability(available=True)

    # Validation errors.
    with pytest.raises(ToolError, match="may_implement=true is only valid with access='read'"):
        await recommend_t3_seats(
            [T3RecommendTask(task="edit", access="write", may_implement=True)],
            doc,
            mode="decision",
            probe_instance=ok,
        )
    for staged_flag in (False, True):
        with pytest.raises(ToolError, match="Image generation seat"):
            await recommend_t3_seats(
                [
                    T3RecommendTask(
                        task="look", access="read", seat="art", may_implement=staged_flag
                    )
                ],
                doc,
                mode="split",
                probe_instance=ok,
            )
    with pytest.raises(ToolError, match="not a writable seat"):
        await recommend_t3_seats(
            [T3RecommendTask(task="look", access="read", seat="explorer-easy", may_implement=True)],
            doc,
            mode="split",
            probe_instance=ok,
        )
    with pytest.raises(ToolError, match="cannot be used with may_implement=true"):
        await recommend_t3_seats(
            [
                T3RecommendTask(
                    task="look", access="read", seat="correctness-reviewer", may_implement=True
                )
            ],
            doc,
            mode="split",
            probe_instance=ok,
        )
    with pytest.raises(ToolError, match="seat_required") as missing_exc:
        await recommend_t3_seats(
            [T3RecommendTask(task="investigate", access="read", may_implement=True)],
            doc,
            mode="split",
            probe_instance=ok,
        )
    missing_msg = str(missing_exc.value)
    assert "worker-easy" in missing_msg and "worker-hard" in missing_msg
    assert "art" not in missing_msg and "explorer-easy" not in missing_msg

    # Legacy single-item normalization keeps may_implement.
    assert normalize_t3_recommend_args(
        {"task": "check", "access": "read", "may_implement": True, "manager_session_id": "boss-1"}
    ) == {"tasks": [{"task": "check", "access": "read", "may_implement": True}]}

    # Decision mode auto-selection for may_implement=True excludes image seats and uses staged text.
    recorded: list[tuple[str, dict[str, str]]] = []

    class _FakeClient:
        def decide(self, text: str, candidates: Any) -> Decision:
            raise AssertionError("T3 uses decide_many")

        def decide_many(self, questions: Any) -> list[Decision]:
            for q in questions:
                recorded.append((q.text, dict(q.candidates)))
            return [Decision("worker-hard") for _ in questions]

    def _fake_client_for_config(_cfg: seat_recommendation.DecisionModelConfig) -> _FakeClient:
        return _FakeClient()

    monkeypatch.setattr(seat_recommendation, "client_for_config", _fake_client_for_config)
    staged_payload = await recommend_t3_seats(
        [T3RecommendTask(task="find root cause before fixing", access="read", may_implement=True)],
        doc,
        mode="decision",
        probe_instance=ok,
    )
    assert len(recorded) == 1
    assert "may turn to implementation" in recorded[0][0]
    assert set(recorded[0][1]) == {"worker-easy", "worker-hard"}
    assert "art" not in recorded[0][1]
    staged_item = staged_payload.get("results", [])[0]
    assert staged_item.get("seat") == "worker-hard"
    assert staged_item.get("note") == T3_STAGED_NOTE
    assert staged_item.get("brief_suffix") == (
        f"{T3_CHILD_IDENTITY_SUFFIX} {T3_STAGED_READ_ONLY_INSTRUCTIONS}"
    )
    assert T3_STAGED_NEXT in staged_payload["next"]
    assert T3_WRITE_NEXT in staged_payload["next"]

    # Pure read-only omits T3_STAGED_NEXT and T3_WRITE_NEXT; pure write includes T3_WRITE_NEXT only.
    read_payload = await recommend_t3_seats(
        [T3RecommendTask(task="just inspect", access="read")],
        doc,
        mode="decision",
        probe_instance=ok,
    )
    assert T3_STAGED_NEXT not in read_payload["next"]
    assert T3_WRITE_NEXT not in read_payload["next"]

    write_payload = await recommend_t3_seats(
        [T3RecommendTask(task="apply fix", access="write", seat="worker-easy")],
        doc,
        mode="split",
        probe_instance=ok,
    )
    assert T3_STAGED_NEXT not in write_payload["next"]
    assert T3_WRITE_NEXT in write_payload["next"]
    assert "t3_worktree_handoff" in T3_WRITE_NEXT
    assert "t3_thread_read" in T3_WRITE_NEXT
    assert "--force" in T3_WRITE_NEXT


def test_non_t3_staged_usable_excludes_image_seats() -> None:
    from agentnave.orchestration_roles import CustomSeatStatus

    worker = CustomSeatStatus(
        kind="worker",
        role="通用实现",
        description="改代码",
        access="write",
        difficulty="easy",
        provider="claude",
        model="sonnet",
    )
    image = CustomSeatStatus(
        kind="image",
        role="插画",
        description="生成插画",
        access="write",
        provider="codex",
        model="gpt",
    )
    catalog_both = seat_recommendation._Catalog(  # pyright: ignore[reportPrivateUsage]
        seat_recommendation.DecisionModelConfig(), {"worker": worker, "art": image}
    )
    usable_staged = seat_recommendation._usable(  # pyright: ignore[reportPrivateUsage]
        catalog_both, "read", staged=True
    )
    assert isinstance(usable_staged, dict) and set(usable_staged) == {"worker"}
    usable_write = seat_recommendation._usable(  # pyright: ignore[reportPrivateUsage]
        catalog_both, "write", staged=False
    )
    assert isinstance(usable_write, dict) and set(usable_write) == {"worker", "art"}

    catalog_image_only = seat_recommendation._Catalog(  # pyright: ignore[reportPrivateUsage]
        seat_recommendation.DecisionModelConfig(), {"art": image}
    )
    err = seat_recommendation._usable(  # pyright: ignore[reportPrivateUsage]
        catalog_image_only, "read", staged=True
    )
    assert isinstance(err, seat_recommendation.RecommendationError)
    assert err.code == "no_candidates"


def test_antigravity_delegate_next_is_conditional_and_platform_specific() -> None:
    for posix in ("darwin", "linux"):
        text = antigravity_delegate_next(posix)
        assert "call it directly" in text
        assert "clientRequestId" in text
        assert "acp-mcp-call delegate_task - <<'T3_JSON'" in text
        assert "- < payload.json" in text
        assert "T3_ACP_MCP_CALL" not in text
        assert "PowerShell" not in text

    win = antigravity_delegate_next("win32")
    assert "call it directly" in win
    assert "clientRequestId" in win
    assert "| & $env:T3_ACP_MCP_CALL delegate_task -" in win
    assert "\\uXXXX" in win
    assert "ELECTRON_RUN_AS_NODE" not in win
    assert "<<'T3_JSON'" not in win

"""The decision-model interface of seat recommendation, and its App-written API keys (ADR-0087).

A decision model gets an input and candidates (id to description) and picks one candidate with
probabilities. Provider-specific wire formats live in their own implementation modules.
"""

from __future__ import annotations

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Literal, Protocol, cast

from agentnave.members import _repair, data_root  # pyright: ignore[reportPrivateUsage]

KEYS_FILE = "decision-keys.json"

type DecisionErrorCode = Literal["network", "rate_limited", "auth", "rejected", "bad_response"]


class DecisionError(Exception):
    """The decision model could not answer; the message never holds the API key."""

    def __init__(self, code: DecisionErrorCode, message: str) -> None:
        super().__init__(message)
        self.code: DecisionErrorCode = code
        self.message = message


class DecisionKeysFileError(ValueError):
    """decision-keys.json cannot be read or validated; messages never echo its content."""


@dataclass(frozen=True)
class Decision:
    choice: str
    probabilities: Mapping[str, float] = field(default_factory=lambda: {})
    confidence: float | None = None


@dataclass(frozen=True)
class DecisionQuestion:
    """One choice to make: the input text and the candidates (id to description)."""

    text: str
    candidates: Mapping[str, str]


class DecisionClient(Protocol):
    def decide(self, text: str, candidates: Mapping[str, str]) -> Decision:
        """Pick one candidate id for the input; raises DecisionError."""
        ...

    def decide_many(self, questions: Sequence[DecisionQuestion]) -> list[Decision | DecisionError]:
        """Answer several independent questions in one request, in order.

        A question the model answered badly is a DecisionError in its place; a failure of the
        request as a whole raises DecisionError."""
        ...


def read_decision_key(name: str) -> str | None:
    """One decision model's API key from the App-written file; None when not set.

    Pydantic errors would echo the offending value, so this reads the file by hand.
    """
    try:
        text = (data_root() / KEYS_FILE).read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        raise DecisionKeysFileError(
            f"Unable to read {KEYS_FILE} ({type(exc).__name__}). {_repair(KEYS_FILE)}"
        ) from None
    try:
        document: object = json.loads(text)
    except ValueError:
        document = None
    fields = cast(dict[str, object], document) if isinstance(document, dict) else {}
    keys = fields.get("version") == 1 and type(fields["version"]) is int and fields.get("keys")
    if not isinstance(keys, dict) or not all(
        isinstance(value, str) for value in cast(dict[str, object], keys).values()
    ):
        raise DecisionKeysFileError(
            f"Invalid {KEYS_FILE}: needs version 1 and keys as an object of strings. "
            + _repair(KEYS_FILE)
        )
    key = cast(dict[str, str], keys).get(name, "").strip()
    # The key becomes an HTTP header: visible ASCII only, so a stray line break never reaches the
    # HTTP library, whose errors would quote it.
    if not all("!" <= char <= "~" for char in key):
        raise DecisionKeysFileError(
            f"Invalid {KEYS_FILE}: the key contains characters that are not allowed. "
            + _repair(KEYS_FILE)
        )
    return key or None

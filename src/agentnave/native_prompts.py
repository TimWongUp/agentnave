"""Provider-neutral, invocation-local native prompt channel and audit records."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import Literal, NotRequired, TypedDict


def timestamp() -> str:
    return datetime.now(UTC).isoformat()


class PromptPayload(TypedDict):
    prompt_id: str
    kind: Literal["permission", "question"]
    tool_name: str
    tool_input: str
    questions: NotRequired[list[dict[str, object]]]
    created_at: str
    content_source: Literal["subagent_data"]


@dataclass
class NativePrompt:
    prompt_id: str
    kind: Literal["permission", "question"]
    tool_name: str
    tool_input: str = ""
    questions: list[dict[str, object]] = field(default_factory=list[dict[str, object]])
    created_at: str = field(default_factory=timestamp)
    status: Literal["pending", "answered", "closed"] = "pending"
    answer: dict[str, object] | None = None
    closed_at: str | None = None
    notified: bool = False

    def payload(self) -> PromptPayload:
        result: PromptPayload = {
            "prompt_id": self.prompt_id,
            "kind": self.kind,
            "tool_name": self.tool_name,
            "tool_input": self.tool_input,
            "created_at": self.created_at,
            "content_source": "subagent_data",
        }
        if self.kind == "question":
            result["questions"] = self.questions
        return result

    def history(self) -> dict[str, object]:
        return {
            **self.payload(),
            "status": self.status,
            "answer": self.answer,
            "closed_at": self.closed_at,
        }


def validate_answer(
    prompt: NativePrompt, decision: str | None, reason: str | None, answers: dict[str, str] | None
) -> None:
    if reason is not None and (len(reason) > 500 or not reason.isprintable()):
        raise ValueError("reason must be a single printable line of at most 500 characters")
    if prompt.kind == "permission":
        if decision not in ("allow", "deny") or answers is not None:
            raise ValueError("permission prompts require decision=allow/deny and no answers")
    else:
        questions = {str(q.get("question")) for q in prompt.questions}
        if decision is not None or reason is not None or not answers or set(answers) != questions:
            raise ValueError(
                "question prompts require answers for exactly the listed questions, without decision/reason"
            )
        if any(not value.strip() or len(value) > 4000 for value in answers.values()):
            raise ValueError("each answer must contain 1–4000 characters")


@dataclass
class PromptChannel:
    prompts: dict[str, NativePrompt] = field(default_factory=dict[str, NativePrompt])
    writer: asyncio.StreamWriter | None = None
    encode: Callable[[dict[str, object]], bytes] | None = None
    changed: Callable[[], None] | None = None

    def receive(self, prompt: NativePrompt) -> bool:
        existing = self.prompts.get(prompt.prompt_id)
        if existing is not None:
            if prompt.status == "closed" and existing.status == "pending":
                existing.status, existing.closed_at = "closed", timestamp()
                try:
                    self.save()
                except OSError:
                    logging.getLogger(__name__).exception("Could not save native prompt closure")
            return False
        if prompt.status != "pending":
            return False
        self.prompts[prompt.prompt_id] = prompt
        try:
            self.save()
        except OSError:
            logging.getLogger(__name__).exception(
                "Could not save native prompt; answer will retry persistence"
            )
        return True

    def pending(self) -> list[NativePrompt]:
        return [p for p in self.prompts.values() if p.status == "pending"]

    def take(self) -> NativePrompt | None:
        for prompt in self.pending():
            if not prompt.notified:
                prompt.notified = True
                return prompt
        return None

    def save(self) -> None:
        if self.changed is not None:
            self.changed()

    async def answer(
        self,
        prompt_id: str,
        decision: str | None,
        reason: str | None,
        answers: dict[str, str] | None,
        manager_session_id: str | None,
        *,
        cancel: bool = False,
    ) -> NativePrompt:
        prompt = self.prompts.get(prompt_id)
        if prompt is None:
            raise ValueError(f"unknown prompt_id: {prompt_id}")
        if prompt.status != "pending":
            raise ValueError(
                f"prompt {prompt_id} is already {prompt.status}; it can only be answered once"
            )
        if not cancel:
            validate_answer(prompt, decision, reason, answers)
        writer, encode = self.writer, self.encode
        if writer is None or writer.is_closing() or encode is None:
            raise ValueError("native prompt channel is closed")
        reply: dict[str, object] = {"prompt_id": prompt_id, "decision": decision or "allow"}
        if reason is not None:
            reply["message"] = reason
        if answers is not None:
            reply["answers"] = answers
        data = encode(reply)
        prompt.status = "answered"
        prompt.answer = {
            **reply,
            "manager_session_id": manager_session_id,
            "actor": "service_cancel" if cancel else "boss",
            "answered_at": timestamp(),
        }
        try:
            self.save()  # Persist the decision before it can execute a tool.
        except OSError:
            if not cancel:
                prompt.status, prompt.answer = "pending", None
                raise
            logging.getLogger(__name__).exception(
                "Could not save cancellation; retaining audit for final save"
            )
        try:
            writer.write(data)  # Claim before yielding: concurrent answers cannot both send.
            await writer.drain()
        except (BrokenPipeError, ConnectionResetError) as exc:
            prompt.status, prompt.closed_at = "closed", timestamp()
            self.save()
            raise ValueError("native prompt channel closed while sending the answer") from exc
        return prompt

    def close(self) -> None:
        if self.writer is not None:
            self.writer.close()
        for prompt in self.pending():
            prompt.status, prompt.closed_at = "closed", timestamp()
        if self.prompts:
            try:
                self.save()
            except OSError:
                logging.getLogger(__name__).exception("Could not save native prompt closure")

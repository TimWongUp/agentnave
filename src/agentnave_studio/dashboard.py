"""Separate loopback public board and director views. No mutation or filesystem routes."""

from __future__ import annotations

import json
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import cast

_AVATAR_FAMILIES = (
    "claude",
    "gpt",
    "gemini",
    "grok",
    "deepseek",
    "glm",
    "qwen",
    "kimi",
    "minimax",
)


def _avatar_key(*parts: str) -> str:
    return json.dumps(parts, ensure_ascii=False, separators=(",", ":"))


def _avatar_family(model: object) -> str | None:
    name = str(model or "").lower().rsplit("/", 1)[-1]
    if found := next((family for family in _AVATAR_FAMILIES if family in name), None):
        return found
    if re.match(r"^(o[134](?:-|$)|chatgpt|codex)", name):
        return "gpt"
    if re.match(r"^(opus|sonnet|haiku)(?:-|$)", name):
        return "claude"
    return None


def _public_avatar_settings(config: dict[str, object], snapshot: bytes) -> dict[str, object]:
    """Return only overrides needed by a public view; other capabilities keep the full map."""
    try:
        detail: object = json.loads(snapshot)
    except ValueError:
        return {}
    if not isinstance(detail, dict):
        return {}
    data = cast(dict[str, object], detail)
    if data.get("view") != "public":
        return config
    required: set[str] = set()

    def need(provider: object, model: object) -> None:
        if not isinstance(model, str) or not model:
            return
        if isinstance(provider, str) and provider:
            exact = _avatar_key("model", provider, model)
            if exact in config:
                required.add(exact)
                return
        if family := _avatar_family(model):
            required.add(_avatar_key("family", family, ""))

    host = data.get("host")
    messages = data.get("messages")
    if isinstance(messages, list):
        for message in cast(list[object], messages):
            if isinstance(message, dict):
                item = cast(dict[str, object], message)
                if item.get("speaker") == "@host":
                    need(host, item.get("manager_model"))
    seats = data.get("seats")
    if isinstance(seats, list):
        for seat in cast(list[object], seats):
            if isinstance(seat, dict):
                item = cast(dict[str, object], seat)
                need(item.get("provider"), item.get("model"))
    return {key: value for key, value in config.items() if key in required}


class Dashboard:
    def __init__(self, root: Path | None = None) -> None:
        self.snapshots: dict[str, bytes] = {}
        self.last_access = 0.0
        assets = Path(__file__).with_name("dashboard_assets")
        self.assets = {
            "": ("text/html; charset=utf-8", (assets / "index.html").read_bytes()),
            "app.js": ("text/javascript; charset=utf-8", (assets / "app.js").read_bytes()),
            "style.css": ("text/css; charset=utf-8", (assets / "style.css").read_bytes()),
            "avatars.png": ("image/png", (assets / "avatars.png").read_bytes()),
        }
        self.assets["avatars.js"] = (
            "text/javascript; charset=utf-8",
            (assets / "avatars.js").read_bytes(),
        )
        for path in assets.glob("avatar-*"):
            mime = {".png": "image/png", ".svg": "image/svg+xml", ".ico": "image/x-icon"}.get(
                path.suffix
            )
            if mime:
                self.assets[path.name] = (mime, path.read_bytes())
        dashboard = self

        class Handler(BaseHTTPRequestHandler):
            def setup(self) -> None:
                super().setup()
                self.connection.settimeout(2)

            def log_message(self, format: str, *args: object) -> None:
                pass  # Never write HTTP logs to the MCP transport or disclose capability URLs.

            def do_GET(self) -> None:
                if self.headers.get("Host") != dashboard.host or self.headers.get("Origin") not in (
                    None,
                    dashboard.origin,
                ):
                    self.send_error(403)
                    return
                parts = self.path.split("/")
                snapshot = dashboard.snapshots.get(parts[1]) if len(parts) == 3 else None
                if snapshot is None:
                    self.send_error(404)
                    return
                resource = parts[2]
                cache_control = "no-store"
                if resource == "avatar-settings":
                    try:
                        configured: object = (
                            json.loads((root / "avatars.json").read_text()) if root else {}
                        )
                        value = (
                            _public_avatar_settings(cast(dict[str, object], configured), snapshot)
                            if isinstance(configured, dict)
                            else {}
                        )
                    except (OSError, ValueError):
                        value = {}
                    content_type, body = (
                        "application/json; charset=utf-8",
                        json.dumps(value).encode(),
                    )
                elif resource == "state":
                    dashboard.last_access = time.monotonic()
                    content_type, body = (
                        "application/json; charset=utf-8",
                        snapshot,
                    )
                elif resource == "transcript.jsonl":
                    data = cast(dict[str, object], json.loads(snapshot))
                    if data.get("view") == "workbench" or data.get("mode") == "task":
                        self.send_error(404)
                        return
                    messages = cast(list[object], data["messages"])
                    body = "".join(
                        json.dumps(m, ensure_ascii=False) + "\n" for m in messages
                    ).encode()
                    content_type = "application/x-ndjson; charset=utf-8"
                elif resource in dashboard.assets:
                    content_type, body = dashboard.assets[resource]
                    if resource:  # Static files never change within one random-port origin.
                        cache_control = "private, max-age=86400"
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", cache_control)
                self.send_header("X-Content-Type-Options", "nosniff")
                self.send_header("Referrer-Policy", "no-referrer")
                self.send_header(
                    "Content-Security-Policy",
                    "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'",
                )
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.host = f"127.0.0.1:{self.server.server_port}"
        self.origin = f"http://{self.host}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def url(self, token: str) -> str:
        return f"{self.origin}/{token}/"

    def update(self, token: str, snapshot: bytes) -> None:
        self.snapshots[token] = snapshot

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

"""Separate loopback public board and director views. No mutation or filesystem routes."""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import cast


class Dashboard:
    def __init__(self) -> None:
        self.snapshots: dict[str, bytes] = {}
        assets = Path(__file__).with_name("dashboard_assets")
        self.assets = {
            "": ("text/html; charset=utf-8", (assets / "index.html").read_bytes()),
            "app.js": ("text/javascript; charset=utf-8", (assets / "app.js").read_bytes()),
            "style.css": ("text/css; charset=utf-8", (assets / "style.css").read_bytes()),
            "avatars.png": ("image/png", (assets / "avatars.png").read_bytes()),
        }
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
                if len(parts) != 3 or parts[1] not in dashboard.snapshots:
                    self.send_error(404)
                    return
                token, resource = parts[1:]
                cache_control = "no-store"
                if resource == "state":
                    content_type, body = (
                        "application/json; charset=utf-8",
                        dashboard.snapshots[token],
                    )
                elif resource == "transcript.jsonl":
                    data = cast(dict[str, object], json.loads(dashboard.snapshots[token]))
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
                    "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'",
                )
                self.end_headers()
                self.wfile.write(body)

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
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

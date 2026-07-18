#!/usr/bin/env python3
"""
Minimal webhook event receiver for the makod demo.

Receives CloudEvent POSTs from makod's --erp-webhook-url and stores them
in memory so the smoke test (and any browser/curl) can query them.

Endpoints
---------
POST  /*         Accept a webhook payload; print it to stdout; store it.
GET   /events    Return all stored events as a JSON array.
GET   /health    Liveness probe.
DELETE /events   Clear the event log.
"""
from http.server import BaseHTTPRequestHandler, HTTPServer
import datetime
import json
import os
import threading

_lock = threading.Lock()
_events: list = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):  # noqa: D102
        print(f"[webhook] {self.address_string()} — {fmt % args}", flush=True)

    # ── inbound CloudEvent ────────────────────────────────────────────────────

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length).decode("utf-8", errors="replace")
        try:
            body = json.loads(raw)
        except json.JSONDecodeError:
            body = raw

        entry = {
            "received_at": datetime.datetime.utcnow().isoformat() + "Z",
            "path": self.path,
            "headers": {k: v for k, v in self.headers.items()},
            "body": body,
        }
        with _lock:
            _events.append(entry)

        # Pretty-print to container stdout so `docker compose logs webhook` works.
        print(json.dumps(entry, indent=2, ensure_ascii=False), flush=True)

        self._respond(200, b'{"ok":true}', "application/json")

    # ── query / admin ─────────────────────────────────────────────────────────

    def do_GET(self):
        if self.path in ("/events", "/events/"):
            with _lock:
                payload = json.dumps(_events, indent=2, ensure_ascii=False).encode()
            self._respond(200, payload, "application/json")
        elif self.path in ("/health", "/health/"):
            with _lock:
                count = len(_events)
            self._respond(200, json.dumps({"ok": True, "events": count}).encode(), "application/json")
        else:
            self._respond(404, b'{"error":"not found"}', "application/json")

    def do_DELETE(self):
        if self.path in ("/events", "/events/"):
            with _lock:
                _events.clear()
            self._respond(204, b"", None)
        else:
            self._respond(404, b'{"error":"not found"}', "application/json")

    # ── helpers ───────────────────────────────────────────────────────────────

    def _respond(self, code: int, body: bytes, content_type):
        self.send_response(code)
        if content_type:
            self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)


port = int(os.environ.get("PORT", "8000"))
print(f"[webhook] demo receiver listening on :{port}", flush=True)
HTTPServer(("0.0.0.0", port), Handler).serve_forever()

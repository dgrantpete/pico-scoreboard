"""Local read-only viewer for the ESPN sample pipeline.

Serves a single-page UI (ui.html) over the store and the generated artifacts.
Strictly read-only: each request opens a fresh read-only Store (per-thread
sqlite connections; WAL reader snapshots never block the tray's writer, and no
long-lived read transaction pins the WAL). Pipeline commands stay in the CLI —
this only renders what they produced.
"""

import json
import re
import sys
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

from .db import Store

DEFAULT_PORT = 3776  # "ESPN" on a phone keypad
LIVE_MAX_AGE_SECONDS = 120  # a stopped tray must never show a stale LIVE badge

_ARTIFACT_NAME_RE = re.compile(r"^[A-Za-z0-9._-]+\.(json|yaml)$")
_ARTIFACT_KINDS = (
    ("schema_", "schema"),
    ("presence_", "presence"),
    ("discover_", "discover"),
    ("espn_openapi_", "spec"),
)


def _event_states(body: bytes) -> dict[str, int]:
    """Counts of competitions[0].status.type.state across a body's events;
    mirrors Collector._event_states (not imported: collect.py drags in requests)."""
    try:
        events = json.loads(body).get("events", [])
    except (ValueError, AttributeError):
        return {}
    states: dict[str, int] = {}
    for event in events:
        competitions = event.get("competitions") or [{}]
        state = competitions[0].get("status", {}).get("type", {}).get("state") or "unknown"
        states[state] = states.get(state, 0) + 1
    return states


class UiServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, handler, *, db_path: Path, generated_dir: Path):
        super().__init__(address, handler)
        self.db_path = db_path
        self.generated_dir = generated_dir


class UiHandler(BaseHTTPRequestHandler):
    server: UiServer

    def log_message(self, *_args) -> None:
        pass

    def do_GET(self) -> None:
        path = urlsplit(self.path).path
        if path == "/":
            self._serve_index()
        elif path == "/api/dashboard":
            self._send_json(self._dashboard_payload())
        elif path == "/api/artifacts":
            self._send_json(self._artifacts_payload())
        elif path.startswith("/api/artifact/"):
            self._serve_artifact(path.removeprefix("/api/artifact/"))
        else:
            self.send_error(404)

    def _serve_index(self) -> None:
        page = Path(__file__).with_name("ui.html").read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(page)))
        self.end_headers()
        self.wfile.write(page)

    def _send_json(self, payload: dict) -> None:
        raw = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _dashboard_payload(self) -> dict:
        db_path = self.server.db_path
        if not db_path.exists():
            return {"available": False, "db_path": str(db_path)}
        store = Store(db_path, readonly=True)
        try:
            stats = store.league_stats()
            latest = {
                (sport, league): (epoch, body)
                for sport, league, epoch, _max_age, body in store.latest_bodies_per_league()
            }
            bodies, raw_bytes, stored_bytes = store.body_totals()
        finally:
            store.close()

        now = time.time()
        leagues = []
        for sport, league, polls, distinct, dates, first, last, non_200, changed in stats:
            entry = {
                "sport": sport,
                "league": league,
                "polls": polls,
                "distinct": distinct,
                "dates": dates,
                "first": first,
                "last": last,
                "non_200": non_200 or 0,
                "changed": changed or 0,
            }
            latest_row = latest.get((sport, league))
            if latest_row is not None:
                epoch, body = latest_row
                states = _event_states(body)
                age = now - epoch
                entry["age_seconds"] = round(age)
                entry["states"] = states
                entry["live"] = states.get("in", 0) > 0 and age < LIVE_MAX_AGE_SECONDS
            leagues.append(entry)

        wal = db_path.with_name(db_path.name + "-wal")
        return {
            "available": True,
            "generated_at": now,
            "leagues": leagues,
            "totals": {
                "bodies": bodies,
                "raw_bytes": raw_bytes,
                "stored_bytes": stored_bytes,
                "db_bytes": db_path.stat().st_size,
                "wal_bytes": wal.stat().st_size if wal.exists() else 0,
            },
        }

    def _artifacts_payload(self) -> dict:
        generated = self.server.generated_dir
        files = []
        if generated.is_dir():
            for path in sorted(generated.iterdir()):
                if not path.is_file() or not _ARTIFACT_NAME_RE.match(path.name):
                    continue
                kind = next(
                    (kind for prefix, kind in _ARTIFACT_KINDS if path.name.startswith(prefix)),
                    "other",
                )
                stat = path.stat()
                files.append(
                    {"name": path.name, "kind": kind, "size": stat.st_size, "mtime": stat.st_mtime}
                )
        return {"files": files}

    def _serve_artifact(self, name: str) -> None:
        generated = self.server.generated_dir
        if not _ARTIFACT_NAME_RE.match(name):
            self.send_error(404)
            return
        target = (generated / name).resolve()
        if generated.resolve() not in target.parents or not target.is_file():
            self.send_error(404)
            return
        raw = target.read_bytes()
        content_type = (
            "application/json" if target.suffix == ".json" else "text/plain; charset=utf-8"
        )
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


def serve(db_path: Path, generated_dir: Path, port: int, *, open_browser: bool = True) -> int:
    try:
        server = UiServer(
            ("127.0.0.1", port), UiHandler, db_path=db_path, generated_dir=generated_dir
        )
    except OSError as exc:
        print(f"error: cannot bind 127.0.0.1:{port} ({exc})", file=sys.stderr)
        return 2
    url = f"http://127.0.0.1:{port}/"
    print(f"pipeline viewer at {url}  (Ctrl-C to stop)", flush=True)
    if open_browser:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nstopped")
    finally:
        server.server_close()
    return 0

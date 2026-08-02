"""Fake-ESPN HTTP server: serves the two upstream routes the backend fetches.

    GET /{sport}/{league}/scoreboard          -> {"events": [...]}
    GET /{sport}/{league}/summary?event={id}  -> {"commentary": [...]}

The real Rust backend is pointed here via APP_ESPN__BASE_URL; it reads only
HTTP status + body, so plain 200 application/json is the whole contract.
Configured leagues serve their ScenarioState/ReplayRuntime; unconfigured
leagues get an empty slate (200 {"events":[]}) so the device's other enabled
leagues simply show no games.

`mock.yml` is watched by mtime and hot-reloaded: a valid change swaps the
serving state atomically; an invalid file is logged loudly and the current
state keeps serving. Replay entries whose name + identity survive a reload
keep their warped clock (see ReplayRuntime).
"""

import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from .mockdata import (
    MockError,
    MockFile,
    ReplayEntry,
    ReplayRuntime,
    ScenarioEntry,
    ScenarioState,
    StoreReplaySource,
    load_mock,
)

WATCH_INTERVAL = 2.0
_EMPTY_SCOREBOARD = b'{"events":[]}'


def _log(message: str) -> None:
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S")
    print(f"{stamp} {message}", flush=True)


def build_state(
    mockfile: MockFile,
    prev_state: dict | None,
    testdata_root: Path,
    config_dir: Path,
    dsn_factory,
) -> dict:
    """(sport, league) -> ScenarioState | ReplayRuntime.

    Replay clocks survive reloads: an entry whose name AND identity() match
    the previous state's inherits its wall_t0 and source (no re-read).
    """
    from .bundle import BundleReader  # local: bundle support is optional at import

    state: dict = {}
    for entry in mockfile.entries:
        key = (entry.sport, entry.league)
        if isinstance(entry, ScenarioEntry):
            state[key] = ScenarioState(entry, testdata_root)
            continue

        assert isinstance(entry, ReplayEntry)
        previous = (prev_state or {}).get(key)
        if (
            isinstance(previous, ReplayRuntime)
            and previous.entry.name == entry.name
            and previous.entry.identity() == entry.identity()
        ):
            state[key] = previous
            continue
        if entry.source == "store":
            from .db import Store

            store = Store(dsn_factory())
            source = StoreReplaySource(store, entry.league, entry.date)
        else:
            source = BundleReader((config_dir / entry.bundle).resolve())
        state[key] = ReplayRuntime(entry, source)
        _log(
            f"replay {entry.name}: {len(source.scoreboard)} scoreboard polls, "
            f"{len(source.summaries)} summary streams, speed x{entry.speed}"
        )
    return state


class MockServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, state: dict):
        super().__init__(address, MockHandler)
        self.state = state          # swapped atomically by the watcher


class MockHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):  # noqa: A002 - stdlib signature
        pass                                # replaced by our own per-request line

    def _respond(self, status: int, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802 - stdlib naming
        parsed = urlparse(self.path)
        parts = [p for p in parsed.path.split("/") if p]
        now = time.time()
        state: dict = self.server.state

        if len(parts) == 3 and parts[2] == "scoreboard":
            entry_state = state.get((parts[0], parts[1]))
            if entry_state is None:
                self._respond(200, _EMPTY_SCOREBOARD)
                self._trace(parts, "empty slate (unconfigured)")
                return
            if isinstance(entry_state, ReplayRuntime):
                body = entry_state.scoreboard(now)
                self._respond(200, body)
                self._trace(parts, f"replay offset {entry_state.current_offset(now):.0f}s")
            else:
                self._respond(200, entry_state.scoreboard())
                self._trace(parts, "scenario")
            return

        if len(parts) == 3 and parts[2] == "summary":
            event_id = (parse_qs(parsed.query).get("event") or [None])[0]
            entry_state = state.get((parts[0], parts[1]))
            body = None
            if event_id and entry_state is not None:
                if isinstance(entry_state, ReplayRuntime):
                    body = entry_state.summary(now, event_id)
                else:
                    body = entry_state.summary(event_id)
            if body is None:
                self._respond(404, b'{"error":"no summary"}')
                self._trace(parts, f"summary {event_id} -> 404")
            else:
                self._respond(200, body)
                self._trace(parts, f"summary {event_id}")
            return

        self._respond(404, b'{"error":"not found"}')
        self._trace(parts, "404")

    def _trace(self, parts: list, detail: str) -> None:
        _log(f"{'/'.join(parts):<40} {detail}")


class _MockWatcher:
    """mtime-poll hot-reloader. Deliberately not shared with the collector's
    _TargetsWatcher — that one is entangled with session-rollover semantics;
    here a valid change just swaps `server.state` in place."""

    def __init__(self, path: Path, server: MockServer, testdata_root: Path, dsn_factory):
        self._path = path
        self._server = server
        self._testdata = testdata_root
        self._dsn_factory = dsn_factory
        self._mtime = self._stat()

    def _stat(self) -> float | None:
        try:
            return self._path.stat().st_mtime
        except OSError:
            return None

    def watch(self, stop: threading.Event) -> None:
        while not stop.wait(WATCH_INTERVAL):
            mtime = self._stat()
            if mtime is None or mtime == self._mtime:
                continue
            self._mtime = mtime
            try:
                mockfile = load_mock(self._path)
                state = build_state(
                    mockfile,
                    self._server.state,
                    self._testdata,
                    self._path.parent,
                    self._dsn_factory,
                )
            except (MockError, OSError) as exc:
                _log(f"INVALID mock config, keeping current state: {exc}")
                continue
            self._server.state = state
            _log(f"mock config reloaded: {len(state)} league(s)")


def run(config_path: Path, port: int, testdata_root: Path, dsn_factory) -> int:
    try:
        mockfile = load_mock(config_path)
        state = build_state(mockfile, None, testdata_root, config_path.parent, dsn_factory)
    except (MockError, OSError) as exc:
        _log(f"FATAL invalid mock config at startup: {exc}")
        return 2

    server = MockServer(("0.0.0.0", port), state)
    stop = threading.Event()
    watcher = _MockWatcher(config_path, server, testdata_root, dsn_factory)
    threading.Thread(target=watcher.watch, args=(stop,), name="mock-watch", daemon=True).start()

    _log(
        f"mock-espn serving {len(state)} league(s) on :{port} -- watching {config_path}"
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        _log("bye")
    finally:
        stop.set()
        server.server_close()
    return 0

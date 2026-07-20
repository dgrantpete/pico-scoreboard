"""Long-running collector service: sessions, heartbeat, targets hot-reload.

One session = one process run x one config epoch. A targets.yml change ends
the current session (`end_reason='reload'`) and starts a fresh one recording
the new effective config, so gap accounting stays exact and the single-thread
scheduler never sees config change mid-flight. SIGTERM/SIGINT end with
'shutdown'; anything else that kills the process leaves an open session for
the next start's orphan sweep to close as 'crash'.

Zero enabled targets is a first-class state: the service idles and
heartbeats, waiting for config — a freshly deployed stack with `targets: []`
is healthy, not broken.
"""

import os
import signal
import socket
import sys
import threading
import traceback
from datetime import datetime, timezone
from pathlib import Path

from .collect import Collector
from .db import Store
from .targets import TargetsError, TargetsFile, load_targets

WATCH_INTERVAL = 10.0       # seconds between targets.yml mtime checks
HEARTBEAT_INTERVAL = 60.0


def _log(message: str) -> None:
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S")
    print(f"{stamp} service                  {message}", flush=True)


class _TargetsWatcher:
    """Polls the targets file's mtime; on a VALID change, publishes the new
    config and signals the session to roll over. An invalid file is logged
    loudly and the running config stays in force."""

    def __init__(self, path: Path, current: TargetsFile, session_stop: threading.Event):
        self._path = path
        self._session_stop = session_stop
        self._mtime = self._stat_mtime()
        self.config = current
        self.reloaded = False

    def _stat_mtime(self) -> float | None:
        try:
            return self._path.stat().st_mtime
        except OSError:
            return None

    def watch(self, quit_event: threading.Event) -> None:
        while not quit_event.is_set() and not self._session_stop.is_set():
            quit_event.wait(WATCH_INTERVAL)
            mtime = self._stat_mtime()
            if mtime is None or mtime == self._mtime:
                continue
            self._mtime = mtime
            try:
                config = load_targets(self._path)
            except (TargetsError, OSError) as exc:
                _log(f"INVALID targets file, keeping current config: {exc}")
                continue
            self.config = config
            self.reloaded = True
            _log(f"targets changed: {len(config.enabled())} enabled -- rolling session")
            self._session_stop.set()


def serve(targets_path: Path, dsn: str) -> int:
    try:
        config = load_targets(targets_path)
    except (TargetsError, OSError) as exc:
        _log(f"FATAL invalid targets file at startup: {exc}")
        return 2

    store = Store(dsn)
    store.ensure_schema()
    swept = store.sweep_orphan_sessions()
    if swept:
        _log(f"swept {swept} orphan session(s) as crash")

    hostname = socket.gethostname()
    version = os.environ.get("COLLECTOR_VERSION", "dev")
    quit_event = threading.Event()
    current_stop: list[threading.Event] = []    # the live session's stop event

    def _on_signal(signum, frame):
        _log(f"signal {signal.Signals(signum).name} -- shutting down")
        quit_event.set()
        if current_stop:
            current_stop[0].set()

    signal.signal(signal.SIGTERM, _on_signal)
    signal.signal(signal.SIGINT, _on_signal)

    while not quit_event.is_set():
        enabled = list(config.enabled())
        session_id = store.start_session(
            hostname=hostname, version=version, targets=[t.as_doc() for t in enabled]
        )
        _log(
            f"session {session_id} start: {', '.join(t.name for t in enabled) or '0 targets'}"
            f" -- watching {targets_path}"
        )

        session_stop = threading.Event()
        current_stop[:] = [session_stop]
        if quit_event.is_set():                 # signal raced session start
            session_stop.set()

        watcher = _TargetsWatcher(targets_path, config, session_stop)
        watch_thread = threading.Thread(
            target=watcher.watch, args=(quit_event,), name="targets-watch", daemon=True
        )
        watch_thread.start()

        def _beat(sid: int = session_id, stop: threading.Event = session_stop) -> None:
            while not stop.wait(HEARTBEAT_INTERVAL):
                try:
                    store.heartbeat(sid)
                except Exception as exc:
                    _log(f"heartbeat failed: {exc}")

        beat_thread = threading.Thread(target=_beat, name="heartbeat", daemon=True)
        beat_thread.start()

        try:
            if enabled:
                Collector(store, session_id, enabled, stop_event=session_stop).run()
            else:
                session_stop.wait()
        except Exception:
            _log("collector crashed:\n" + traceback.format_exc())
            session_stop.set()
            beat_thread.join(timeout=5)
            try:
                store.reconnect()               # the crash may have poisoned the conn
                store.end_session(session_id, "crash")
            except Exception as exc:
                _log(f"could not close session (orphan sweep will): {exc}")
            store.close()
            return 1

        session_stop.set()
        watch_thread.join(timeout=WATCH_INTERVAL + 5)
        beat_thread.join(timeout=5)
        reason = "shutdown" if quit_event.is_set() else "reload"
        try:
            store.end_session(session_id, reason)
        except Exception:
            # A Postgres restart poisons an idle main connection; the session
            # bookkeeping matters more than the first attempt.
            store.reconnect()
            store.end_session(session_id, reason)
        config = watcher.config

    _log("bye")
    store.close()
    return 0


def main(targets: str, dsn: str) -> int:
    return serve(Path(targets), dsn)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))

"""Target-driven poll loop.

Single thread, min-heap scheduler: each target polls on its own cadence
(adaptive from the response's Cache-Control max-age, or a gentler idle
interval when nothing is live) with per-run error backoff, all writing
through one Store connection under one collector session.

Scoreboard targets with `follow_summaries` spawn dynamic summary runs, one
per live event: created when an event id first shows state "in", polled at
live cadence, and retired two polls after the id leaves the live set (so the
post-final summary body is captured). Followers are derived state — a config
reload rebuilds them within one scoreboard poll of the new session.
"""

import heapq
import json
import re
import threading
import time
from collections import Counter
from collections.abc import Callable
from datetime import datetime, timezone

import psycopg
import requests

from .db import Store
from .leagues import game_day
from .targets import ScoreboardTarget

POLL_FLOOR_SECONDS = 5      # never poll faster than this, even if max-age says so
DEFAULT_INTERVAL = 30       # used when the response carries no max-age
BACKOFF_INITIAL = 5
BACKOFF_CAP = 60
REQUEST_TIMEOUT = 15
SUMMARY_FINAL_POLLS = 2     # polls after an event leaves the live set (post-final body)

_MAX_AGE_RE = re.compile(r"max-age=(\d+)")


class _Run:
    """Mutable per-endpoint collection state (one scoreboard target, or one
    dynamic summary follower)."""

    def __init__(self, key: str, target: ScoreboardTarget, event_id: str | None = None):
        self.key = key
        self.target = target
        self.event_id = event_id                # None => scoreboard endpoint
        self.backoff = 0.0
        self.last_hash: str | None = None
        self.done = False
        self.finishing: int | None = None       # summary runs: polls left after leaving live set
        self.polls = 0
        self.changes = 0
        self.errors = 0
        self.hashes: set[str] = set()
        self.states: Counter[str] = Counter()
        self.live_ids: set[str] = set()

    @property
    def endpoint(self) -> str:
        return "summary" if self.event_id is not None else "scoreboard"

    @property
    def url(self) -> str:
        if self.event_id is not None:
            return self.target.summary_url(self.event_id)
        return self.target.scoreboard_url


class Collector:
    def __init__(
        self,
        store: Store,
        session_id: int,
        targets: list[ScoreboardTarget],
        *,
        stop_event: threading.Event | None = None,
        log: Callable[[str], None] | None = None,
    ):
        self._store = store
        self._session_id = session_id
        self._stop = stop_event if stop_event is not None else threading.Event()
        self._emit = log if log is not None else (lambda line: print(line, flush=True))
        self._runs = {t.name: _Run(t.name, t) for t in targets}
        self._heap: list[tuple[float, str]] = []

    def run(self) -> None:
        session = requests.Session()
        # ESPN 403s unrecognized UA prefixes; python-requests/* is allowlisted,
        # so lead with it (we really are requests) and keep identity as suffix.
        session.headers["User-Agent"] = "python-requests/2.32.3 pico-scoreboard/espn-collector"
        start = time.monotonic()
        self._heap = [(start, key) for key in self._runs]
        heapq.heapify(self._heap)
        try:
            while self._heap:
                due, key = heapq.heappop(self._heap)
                if not self._wait_until(due):
                    break
                run = self._runs[key]
                next_due = self._poll_once(session, run)
                if run.done:
                    del self._runs[key]
                else:
                    heapq.heappush(self._heap, (next_due, key))
        except KeyboardInterrupt:
            self._emit("interrupted")
            self._stop.set()
        finally:
            session.close()
            self._print_summary(time.monotonic() - start)

    def _wait_until(self, due: float) -> bool:
        """Wait until `due`, in short slices so stop/Ctrl-C stay responsive.
        Returns False when the stop event fires."""
        while True:
            if self._stop.is_set():
                return False
            now = time.monotonic()
            if now >= due:
                return True
            self._stop.wait(min(1.0, due - now))

    def _poll_once(self, session: requests.Session, run: _Run) -> float:
        date_param = game_day() if run.endpoint == "scoreboard" else None
        requested_at = datetime.now(timezone.utc)
        try:
            resp = session.get(
                run.url,
                params={"dates": date_param} if date_param else None,
                timeout=REQUEST_TIMEOUT,
            )
        except requests.RequestException as exc:
            return self._error_backoff(run, f"ERROR {type(exc).__name__}: {exc}")

        body = resp.content
        max_age = self._parse_max_age(resp.headers.get("Cache-Control", ""))
        try:
            body_hash = self._store.insert_response(
                session_id=self._session_id,
                target=run.target.name,
                endpoint=run.endpoint,
                sport=run.target.sport,
                league=run.target.league,
                event_id=run.event_id,
                date_param=date_param,
                requested_at=requested_at,
                http_status=resp.status_code,
                max_age=max_age,
                body=body,
                headers=dict(resp.headers),
            )
            self._store.commit()
        except psycopg.OperationalError as exc:
            self._store.reconnect()
            return self._error_backoff(run, f"ERROR db: {exc}")

        run.polls += 1
        changed = body_hash != run.last_hash
        if changed and run.last_hash is not None:
            run.changes += 1
        run.last_hash = body_hash
        run.hashes.add(body_hash)

        marker = "*" if changed else "~"
        detail = f"{marker} {resp.status_code} {len(body) / 1024:.1f}KB max-age={max_age}"

        if resp.status_code != 200:
            return self._error_backoff(run, detail)
        run.backoff = 0.0

        if run.endpoint == "scoreboard":
            run.states, live_ids = self._scan_events(body)
            if run.target.follow_summaries:
                self._sync_followers(run, live_ids)
            run.live_ids = live_ids
        elif run.finishing is not None:
            run.finishing -= 1
            if run.finishing <= 0:
                run.done = True
                detail += " -- follower retired"

        self._log(run, detail)
        return time.monotonic() + self._next_interval(run, max_age)

    def _error_backoff(self, run: _Run, detail: str) -> float:
        run.errors += 1
        run.backoff = min(run.backoff * 2, BACKOFF_CAP) if run.backoff else BACKOFF_INITIAL
        self._log(run, f"{detail} (retry in {run.backoff:.0f}s)")
        return time.monotonic() + run.backoff

    def _sync_followers(self, run: _Run, live_ids: set[str]) -> None:
        """Reconcile this scoreboard run's summary followers with its live set."""
        now = time.monotonic()
        for event_id in sorted(live_ids - run.live_ids):
            key = f"{run.key}#summary:{event_id}"
            if key in self._runs:
                self._runs[key].finishing = None    # event came back to life
                continue
            self._runs[key] = _Run(key, run.target, event_id)
            heapq.heappush(self._heap, (now, key))
            self._log(run, f"+ follower {event_id}")
        for event_id in run.live_ids - live_ids:
            key = f"{run.key}#summary:{event_id}"
            follower = self._runs.get(key)
            if follower is not None and follower.finishing is None:
                follower.finishing = SUMMARY_FINAL_POLLS

    def _next_interval(self, run: _Run, max_age: int | None) -> float:
        adaptive = max(max_age + 1, POLL_FLOOR_SECONDS) if max_age is not None else DEFAULT_INTERVAL
        if run.endpoint == "summary":
            return adaptive                     # live by definition; idle never applies
        if run.target.fixed_interval is not None:
            return run.target.fixed_interval
        live = run.states.get("in", 0) > 0
        if run.target.idle_interval is not None and not live:
            return run.target.idle_interval
        return adaptive

    @staticmethod
    def _scan_events(body: bytes) -> tuple[Counter[str], set[str]]:
        """(state counts, live event ids) across events in a scoreboard body."""
        try:
            events = json.loads(body).get("events", [])
        except (ValueError, AttributeError):
            return Counter(), set()
        states: Counter[str] = Counter()
        live_ids: set[str] = set()
        for event in events:
            competitions = event.get("competitions") or [{}]
            state = competitions[0].get("status", {}).get("type", {}).get("state")
            states[state if state else "unknown"] += 1
            if state == "in" and event.get("id"):
                live_ids.add(str(event["id"]))
        return states, live_ids

    @staticmethod
    def _parse_max_age(cache_control: str) -> int | None:
        match = _MAX_AGE_RE.search(cache_control)
        return int(match.group(1)) if match else None

    def _log(self, run: _Run, message: str) -> None:
        stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S")
        self._emit(f"{stamp} {run.key:<24} {message}")

    def _print_summary(self, elapsed: float) -> None:
        self._emit(f"session over after {elapsed / 60:.1f} min")
        for run in self._runs.values():
            self._emit(
                f"  {run.key:<24} polls={run.polls} distinct={len(run.hashes)}"
                f" changes={run.changes} errors={run.errors}"
            )

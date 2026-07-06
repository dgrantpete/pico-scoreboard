"""Live scoreboard collector.

Single thread, min-heap scheduler: each league polls on its own cadence
(adaptive from the response's Cache-Control max-age, or a gentler idle
interval when nothing is live) with per-league error backoff, all writing
through one Store connection. Optional stop/pause events and a status
snapshot make the same loop drivable from the tray app.
"""

import heapq
import json
import re
import threading
import time
from collections import Counter
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import datetime, timezone

import requests

from .db import Store
from .leagues import League

POLL_FLOOR_SECONDS = 5      # never poll faster than this, even if max-age says so
DEFAULT_INTERVAL = 30       # used when the response carries no max-age
BACKOFF_INITIAL = 5
BACKOFF_CAP = 60
REQUEST_TIMEOUT = 15
POST_STREAK_REQUIRED = 2    # consecutive all-post polls before a league is done

_MAX_AGE_RE = re.compile(r"max-age=(\d+)")


@dataclass(frozen=True)
class LeagueStatus:
    """Thread-safe view of one league's collection state for UIs."""

    key: str
    polls: int
    changes: int
    errors: int
    last_http_status: int | None
    last_poll_epoch: float | None
    next_due_in: float | None       # seconds from now; None before first schedule
    states: Mapping[str, int]       # event states from the last 200 body
    live: bool
    done: bool


class _LeagueRun:
    """Mutable per-league collection state."""

    def __init__(self, key: str, league: League):
        self.key = key
        self.league = league
        self.backoff = 0.0
        self.last_hash: str | None = None
        self.post_streak = 0
        self.done = False
        self.polls = 0
        self.changes = 0
        self.errors = 0
        self.hashes: set[str] = set()
        self.last_http_status: int | None = None
        self.last_poll_epoch: float | None = None
        self.states: Counter[str] = Counter()


class Collector:
    def __init__(
        self,
        store: Store,
        leagues: dict[str, League],
        date_param: str | Callable[[], str],
        *,
        duration: float | None = None,
        until_all_post: bool = False,
        fixed_interval: float | None = None,
        idle_interval: float | None = None,
        source: str = "live",
        stop_event: threading.Event | None = None,
        pause_event: threading.Event | None = None,
        log: Callable[[str], None] | None = None,
    ):
        self._store = store
        self._date: Callable[[], str] = (
            date_param if callable(date_param) else (lambda: date_param)
        )
        self._duration = duration
        self._until_all_post = until_all_post
        self._fixed_interval = fixed_interval
        self._idle_interval = idle_interval
        self._source = source
        self._stop = stop_event if stop_event is not None else threading.Event()
        self._pause = pause_event if pause_event is not None else threading.Event()
        self._emit = log if log is not None else (lambda line: print(line, flush=True))
        self._runs = {key: _LeagueRun(key, league) for key, league in leagues.items()}
        self._lock = threading.Lock()
        self._statuses: dict[str, LeagueStatus] = {}

    def snapshot(self) -> dict[str, LeagueStatus]:
        with self._lock:
            return dict(self._statuses)

    def run(self) -> None:
        session = requests.Session()
        session.headers["User-Agent"] = "pico-scoreboard/espn-collector"
        start = time.monotonic()
        deadline = start + self._duration if self._duration is not None else None
        heap = [(start, key) for key in self._runs]
        heapq.heapify(heap)
        try:
            while heap:
                due, key = heapq.heappop(heap)
                if not self._wait_until(due, deadline):
                    if not self._stop.is_set():
                        self._emit("duration reached")
                    break
                run = self._runs[key]
                next_due = self._poll_once(session, run)
                self._publish_status(run, next_due)
                if not run.done:
                    heapq.heappush(heap, (next_due, key))
        except KeyboardInterrupt:
            self._emit("\ninterrupted")
        finally:
            session.close()
            self._print_summary(time.monotonic() - start)

    def _wait_until(self, due: float, deadline: float | None) -> bool:
        """Wait until `due`, in short slices so stop/pause/Ctrl-C stay responsive.
        Returns False when the stop event fires or the run deadline arrives."""
        while True:
            if self._stop.is_set():
                return False
            now = time.monotonic()
            if deadline is not None and now >= deadline:
                return False
            if self._pause.is_set():
                self._stop.wait(1.0)
                continue
            if now >= due:
                return True
            limit = due if deadline is None else min(due, deadline)
            self._stop.wait(min(1.0, limit - now))

    def _poll_once(self, session: requests.Session, run: _LeagueRun) -> float:
        date_param = self._date()
        requested_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
        epoch = time.time()
        try:
            resp = session.get(
                run.league.scoreboard_url,
                params={"dates": date_param},
                timeout=REQUEST_TIMEOUT,
            )
        except requests.RequestException as exc:
            run.errors += 1
            run.backoff = min(run.backoff * 2, BACKOFF_CAP) if run.backoff else BACKOFF_INITIAL
            self._log(run, f"ERROR {type(exc).__name__}: {exc} (retry in {run.backoff:.0f}s)")
            return time.monotonic() + run.backoff

        body = resp.content
        max_age = self._parse_max_age(resp.headers.get("Cache-Control", ""))
        body_hash = self._store.insert_response(
            sport=run.league.sport,
            league=run.league.slug,
            date_param=date_param,
            requested_at=requested_at,
            epoch=epoch,
            http_status=resp.status_code,
            max_age=max_age,
            body=body,
            headers_json=json.dumps(dict(resp.headers)),
            source=self._source,
        )
        self._store.commit()

        run.polls += 1
        run.last_http_status = resp.status_code
        run.last_poll_epoch = epoch
        changed = body_hash != run.last_hash
        if changed and run.last_hash is not None:
            run.changes += 1
        run.last_hash = body_hash
        run.hashes.add(body_hash)

        marker = "*" if changed else "~"
        detail = f"{marker} {resp.status_code} {len(body) / 1024:.1f}KB max-age={max_age}"

        if resp.status_code != 200:
            run.errors += 1
            run.backoff = min(run.backoff * 2, BACKOFF_CAP) if run.backoff else BACKOFF_INITIAL
            self._log(run, f"{detail} (retry in {run.backoff:.0f}s)")
            return time.monotonic() + run.backoff
        run.backoff = 0.0
        run.states = self._event_states(body)

        if self._until_all_post and run.states and set(run.states) == {"post"}:
            run.post_streak += 1
            if run.post_streak >= POST_STREAK_REQUIRED:
                run.done = True
                self._log(run, f"{detail} -- all events post, league done")
                return 0.0
        else:
            run.post_streak = 0

        self._log(run, detail)
        return time.monotonic() + self._next_interval(run, max_age)

    def _next_interval(self, run: _LeagueRun, max_age: int | None) -> float:
        if self._fixed_interval is not None:
            return self._fixed_interval
        adaptive = max(max_age + 1, POLL_FLOOR_SECONDS) if max_age is not None else DEFAULT_INTERVAL
        live = run.states.get("in", 0) > 0
        if self._idle_interval is not None and not live:
            return self._idle_interval
        return adaptive

    def _publish_status(self, run: _LeagueRun, next_due: float) -> None:
        status = LeagueStatus(
            key=run.key,
            polls=run.polls,
            changes=run.changes,
            errors=run.errors,
            last_http_status=run.last_http_status,
            last_poll_epoch=run.last_poll_epoch,
            next_due_in=max(0.0, next_due - time.monotonic()) if not run.done else None,
            states=dict(run.states),
            live=run.states.get("in", 0) > 0,
            done=run.done,
        )
        with self._lock:
            self._statuses[run.key] = status

    @staticmethod
    def _event_states(body: bytes) -> Counter[str]:
        """Count of competitions[0].status.type.state across events in a body."""
        try:
            events = json.loads(body).get("events", [])
        except (ValueError, AttributeError):
            return Counter()
        states: Counter[str] = Counter()
        for event in events:
            competitions = event.get("competitions") or [{}]
            state = competitions[0].get("status", {}).get("type", {}).get("state")
            states[state if state else "unknown"] += 1
        return states

    @staticmethod
    def _parse_max_age(cache_control: str) -> int | None:
        match = _MAX_AGE_RE.search(cache_control)
        return int(match.group(1)) if match else None

    def _log(self, run: _LeagueRun, message: str) -> None:
        stamp = datetime.now().strftime("%H:%M:%S")
        self._emit(f"{stamp} {run.key:<10} {message}")

    def _print_summary(self, elapsed: float) -> None:
        self._emit(f"\ncollected for {elapsed / 60:.1f} min -> {self._store.path}")
        for run in self._runs.values():
            self._emit(
                f"  {run.key:<10} polls={run.polls} distinct={len(run.hashes)}"
                f" changes={run.changes} errors={run.errors}"
                + (" (done)" if run.done else "")
            )

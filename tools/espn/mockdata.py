"""Mock-ESPN config model and payload synthesis.

`mock.yml` declares, per (sport, league), either a *scenario* — a slate
composed from fixture events in `backend/testdata/` — or a *replay* — a
time-warped re-serve of a captured stream (Postgres store or exported
bundle). Parsing is `targets.py`-strict: unknown keys, duplicate names,
missing fixtures, or bad durations are `MockError`s, so a typo'd demo config
fails loudly instead of silently serving nothing.

All served-date rewriting funnels through `shift_event_dates` — ESPN dates
appear at BOTH `event.date` and `competitions[0].date` in minute-precision
`%Y-%m-%dT%H:%MZ` form, and the backend parses the event-level one for
pregame start times. A serve must never crash on an unparseable date; the
field is left untouched instead.
"""

import json
import re
import threading
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

import yaml

_ESPN_DATE = "%Y-%m-%dT%H:%MZ"

_DURATION_RE = re.compile(r"^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$")

_ENTRY_KEYS_COMMON = {"name", "mode", "sport", "league"}
_SCENARIO_KEYS = _ENTRY_KEYS_COMMON | {"events"}
_SCENARIO_EVENT_KEYS = {"fixture", "start_in", "commentary"}
_REPLAY_KEYS = _ENTRY_KEYS_COMMON | {"source", "date", "bundle", "speed", "loop", "start_offset"}

DEFAULT_PREGAME_LEAD_S = 60 * 60      # start_in default for pre-state fixtures
PAST_SHIFT_S = 90 * 60                # in/post fixtures read as "started 90m ago"
LOOP_TAIL_HOLD_S = 60.0               # replay loop: dwell on the final state


class MockError(ValueError):
    """Invalid mock config. Message carries full context for logs."""


def parse_duration(value) -> float:
    """'90m' / '2h' / '45s' / '1h30m' / bare number (seconds) -> seconds."""
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if value < 0:
            raise MockError(f"duration must be >= 0, got {value!r}")
        return float(value)
    if isinstance(value, str):
        match = _DURATION_RE.match(value.strip())
        if match and any(match.groups()):
            h, m, s = (int(g) if g else 0 for g in match.groups())
            return float(h * 3600 + m * 60 + s)
    raise MockError(f"invalid duration {value!r} (try 45s, 90m, 2h, 1h30m)")


def parse_espn_date(value) -> datetime | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        return datetime.strptime(value, _ESPN_DATE).replace(tzinfo=timezone.utc)
    except ValueError:
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return None


def format_espn_date(dt: datetime) -> str:
    return dt.astimezone(timezone.utc).strftime(_ESPN_DATE)


def shift_event_dates(event: dict, warp) -> None:
    """Rewrite the event's date fields through `warp(datetime) -> datetime`.

    Touches `event["date"]` and `competitions[0]["date"]` — the two places
    ESPN carries the start time. Unparseable dates are left untouched.
    """
    holders = [event]
    competitions = event.get("competitions")
    if isinstance(competitions, list) and competitions and isinstance(competitions[0], dict):
        holders.append(competitions[0])
    for holder in holders:
        parsed = parse_espn_date(holder.get("date"))
        if parsed is not None:
            holder["date"] = format_espn_date(warp(parsed))


# --- config model ------------------------------------------------------------


@dataclass(frozen=True)
class ScenarioEvent:
    fixture: str                # path relative to the testdata root
    start_in: float | None      # seconds; pre-state fixtures only
    commentary: str | None      # served at /summary for this event's id


@dataclass(frozen=True)
class ScenarioEntry:
    name: str
    sport: str
    league: str
    events: tuple[ScenarioEvent, ...]


@dataclass(frozen=True)
class ReplayEntry:
    name: str
    sport: str
    league: str
    source: str                 # 'store' | 'bundle'
    date: str | None            # YYYYMMDD (source: store)
    bundle: str | None          # path relative to the mock.yml dir (source: bundle)
    speed: float
    loop: bool
    start_offset: float         # seconds into the capture

    def identity(self) -> tuple:
        """Replay-clock preservation key: a hot-reload that leaves these
        unchanged keeps the running warped clock."""
        return (self.sport, self.league, self.source, self.date, self.bundle,
                self.speed, self.loop, self.start_offset)


@dataclass(frozen=True)
class MockFile:
    entries: tuple


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise MockError(message)


def _common_fields(entry: dict) -> tuple[str, str, str]:
    for key in ("name", "sport", "league"):
        _require(
            isinstance(entry.get(key), str) and entry[key],
            f"entry missing required string {key!r}: {entry!r}",
        )
    return entry["name"], entry["sport"], entry["league"]


def _parse_scenario(entry: dict) -> ScenarioEntry:
    name, sport, league = _common_fields(entry)
    unknown = set(entry) - _SCENARIO_KEYS
    _require(not unknown, f"unknown keys {sorted(unknown)} in scenario {name!r}")
    raw_events = entry.get("events")
    _require(
        isinstance(raw_events, list) and raw_events,
        f"scenario {name!r} needs a non-empty events list",
    )
    events = []
    for raw in raw_events:
        _require(isinstance(raw, dict), f"scenario {name!r}: each event must be a mapping")
        unknown = set(raw) - _SCENARIO_EVENT_KEYS
        _require(not unknown, f"unknown keys {sorted(unknown)} in an event of {name!r}")
        fixture = raw.get("fixture")
        _require(
            isinstance(fixture, str) and fixture,
            f"scenario {name!r}: event missing 'fixture'",
        )
        commentary = raw.get("commentary")
        _require(
            commentary is None or (isinstance(commentary, str) and commentary),
            f"scenario {name!r}: commentary must be a non-empty string",
        )
        start_in = raw.get("start_in")
        events.append(
            ScenarioEvent(
                fixture=fixture,
                start_in=None if start_in is None else parse_duration(start_in),
                commentary=commentary,
            )
        )
    return ScenarioEntry(name=name, sport=sport, league=league, events=tuple(events))


def _parse_replay(entry: dict) -> ReplayEntry:
    name, sport, league = _common_fields(entry)
    unknown = set(entry) - _REPLAY_KEYS
    _require(not unknown, f"unknown keys {sorted(unknown)} in replay {name!r}")
    source = entry.get("source")
    _require(source in ("store", "bundle"), f"replay {name!r}: source must be store|bundle")
    date = entry.get("date")
    bundle = entry.get("bundle")
    if source == "store":
        _require(
            isinstance(date, str) and re.fullmatch(r"\d{8}", date) and bundle is None,
            f"replay {name!r}: source store needs date: \"YYYYMMDD\" (quoted) and no bundle",
        )
    else:
        _require(
            isinstance(bundle, str) and bundle and date is None,
            f"replay {name!r}: source bundle needs a bundle path and no date",
        )
    speed = entry.get("speed", 1.0)
    _require(
        isinstance(speed, (int, float)) and not isinstance(speed, bool) and speed > 0,
        f"replay {name!r}: speed must be a positive number",
    )
    loop = entry.get("loop", False)
    _require(isinstance(loop, bool), f"replay {name!r}: loop must be a bool")
    return ReplayEntry(
        name=name,
        sport=sport,
        league=league,
        source=source,
        date=date,
        bundle=bundle,
        speed=float(speed),
        loop=loop,
        start_offset=parse_duration(entry.get("start_offset", 0)),
    )


def parse_mock(text: str) -> MockFile:
    try:
        doc = yaml.safe_load(text)
    except yaml.YAMLError as exc:
        raise MockError(f"YAML syntax error: {exc}")
    _require(isinstance(doc, dict), f"mock file must be a mapping, got {type(doc).__name__}")
    _require(doc.get("version") == 1, f"unsupported mock version {doc.get('version')!r}")
    unknown = set(doc) - {"version", "leagues"}
    _require(not unknown, f"unknown top-level keys {sorted(unknown)}")
    raw_entries = doc.get("leagues")
    _require(isinstance(raw_entries, list), "leagues must be a list (use [] for none)")

    entries = []
    for raw in raw_entries:
        _require(isinstance(raw, dict), f"each league entry must be a mapping, got {raw!r}")
        mode = raw.get("mode")
        if mode == "scenario":
            entries.append(_parse_scenario(raw))
        elif mode == "replay":
            entries.append(_parse_replay(raw))
        else:
            raise MockError(f"unknown mode {mode!r} in entry {raw.get('name')!r}")

    names = [e.name for e in entries]
    dupes = {n for n in names if names.count(n) > 1}
    _require(not dupes, f"duplicate entry names {sorted(dupes)}")
    pairs = [(e.sport, e.league) for e in entries]
    dupes = {p for p in pairs if pairs.count(p) > 1}
    _require(not dupes, f"multiple entries for the same league: {sorted(dupes)}")
    return MockFile(entries=tuple(entries))


def load_mock(path: Path) -> MockFile:
    return parse_mock(path.read_text(encoding="utf-8"))


# --- scenario runtime --------------------------------------------------------


class ScenarioState:
    """Immutable per-league serving state for a scenario entry.

    Bodies are built ONCE (anchored at build time), so the backend sees
    byte-stable payloads and its games-list ETag/304 flow works. `pre`
    fixtures get start = t0 + start_in; `in`/`post` fixtures are shifted so
    their original start maps to t0 - 90m ("earlier today").
    """

    def __init__(self, entry: ScenarioEntry, testdata_root: Path):
        t0 = datetime.now(timezone.utc)
        events = []
        summaries: dict[str, bytes] = {}
        seen_ids: set[str] = set()
        for item in entry.events:
            path = testdata_root / item.fixture
            try:
                event = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError) as exc:
                raise MockError(f"scenario {entry.name!r}: cannot load {path}: {exc}")
            _require(
                isinstance(event, dict) and event.get("id"),
                f"scenario {entry.name!r}: {item.fixture} is not a bare event object with an id",
            )
            event_id = str(event["id"])
            _require(
                event_id not in seen_ids,
                f"scenario {entry.name!r}: duplicate event id {event_id} "
                f"({item.fixture}) — summary routing would collide",
            )
            seen_ids.add(event_id)

            state = self._event_state(event)
            if item.start_in is not None:
                _require(
                    state == "pre",
                    f"scenario {entry.name!r}: start_in on {item.fixture} "
                    f"(state {state!r}) — only pre-state fixtures take a start time",
                )
            if state == "pre":
                lead = item.start_in if item.start_in is not None else DEFAULT_PREGAME_LEAD_S
                target = t0 + timedelta(seconds=lead)
                shift_event_dates(event, lambda _orig, t=target: t)
            else:
                original = parse_espn_date(event.get("date"))
                if original is not None:
                    delta = (t0 - timedelta(seconds=PAST_SHIFT_S)) - original
                    shift_event_dates(event, lambda orig, d=delta: orig + d)

            events.append(event)
            if item.commentary is not None:
                summaries[event_id] = json.dumps(
                    {"commentary": [{"sequence": 1, "text": item.commentary}]}
                ).encode()

        self.entry = entry
        self.scoreboard_body = json.dumps({"events": events}).encode()
        self.summaries = summaries

    @staticmethod
    def _event_state(event: dict) -> str:
        competitions = event.get("competitions") or [{}]
        return competitions[0].get("status", {}).get("type", {}).get("state") or "unknown"

    def scoreboard(self) -> bytes:
        return self.scoreboard_body

    def summary(self, event_id: str) -> bytes | None:
        return self.summaries.get(event_id)


# --- replay runtime ----------------------------------------------------------


class ReplayRuntime:
    """Time-warped serving state over a ReplaySource (store or bundle).

    Warped capture offset: `offset(now) = start_offset + (now - wall_t0) * speed`.
    With `loop`, the offset wraps modulo (span + LOOP_TAIL_HOLD_S) so the
    final state dwells briefly before the slate restarts.

    Served scoreboard bodies get their event dates rewritten through the warp
    FUNCTION (not a constant delta): `t_wall(t_cap) = wall_t0 +
    (t_cap - capture_t0 - start_offset) / speed`, so displayed start times
    stay consistent with warped progress at any speed. Summary bodies are
    served verbatim (the backend reads only commentary text).
    """

    def __init__(self, entry: ReplayEntry, source, wall_t0: float | None = None):
        _require(
            bool(source.scoreboard),
            f"replay {entry.name!r}: capture has no 200-status scoreboard polls",
        )
        self.entry = entry
        self.source = source
        self.wall_t0 = wall_t0 if wall_t0 is not None else time.time()
        last_offset = source.scoreboard[-1][0]
        _require(
            entry.start_offset <= last_offset,
            f"replay {entry.name!r}: start_offset {entry.start_offset:.0f}s is past "
            f"the capture's end ({last_offset:.0f}s)",
        )
        self._span = (last_offset - entry.start_offset) + LOOP_TAIL_HOLD_S
        self._rewrite_cache: dict[tuple[str, int], bytes] = {}
        self._cache_lock = threading.Lock()

    def _warped(self, now: float) -> tuple[float, int]:
        """(capture offset, loop cycle index) for wall time `now`."""
        progress = max(0.0, now - self.wall_t0) * self.entry.speed
        if self.entry.loop:
            cycle, progress = divmod(progress, self._span)
            return self.entry.start_offset + progress, int(cycle)
        return self.entry.start_offset + progress, 0

    @staticmethod
    def _at(records: list, offset: float):
        """Last (offset_s, key) record with offset_s <= offset, else the first."""
        chosen = records[0]
        for record in records:
            if record[0] <= offset:
                chosen = record
            else:
                break
        return chosen

    def scoreboard(self, now: float) -> bytes:
        offset, cycle = self._warped(now)
        record_offset, key = self._at(self.source.scoreboard, offset)
        cache_key = (key, cycle)
        with self._cache_lock:
            cached = self._rewrite_cache.get(cache_key)
        if cached is not None:
            return cached
        body = self._rewrite(self.source.body(key), cycle)
        with self._cache_lock:
            if len(self._rewrite_cache) > 64:
                self._rewrite_cache.clear()
            self._rewrite_cache[cache_key] = body
        return body

    def summary(self, now: float, event_id: str) -> bytes | None:
        records = self.source.summaries.get(event_id)
        if not records:
            return None
        offset, _cycle = self._warped(now)
        _record_offset, key = self._at(records, offset)
        return self.source.body(key)

    def _rewrite(self, raw: bytes, cycle: int) -> bytes:
        try:
            doc = json.loads(raw)
        except ValueError:
            return raw
        capture_t0 = self.source.capture_t0
        cycle_wall_t0 = self.wall_t0 + cycle * (self._span / self.entry.speed)

        def warp(t_cap: datetime) -> datetime:
            cap_offset = (t_cap - capture_t0).total_seconds()
            wall = cycle_wall_t0 + (cap_offset - self.entry.start_offset) / self.entry.speed
            return datetime.fromtimestamp(wall, tz=timezone.utc)

        for event in doc.get("events") or []:
            if isinstance(event, dict):
                shift_event_dates(event, warp)
        return json.dumps(doc).encode()

    def current_offset(self, now: float) -> float:
        """Warped capture offset, for per-request logging (monotonicity check)."""
        return self._warped(now)[0]


class StoreReplaySource:
    """ReplaySource over the v2 Postgres store.

    One Store connection serves all body fetches; psycopg connections are not
    thread-safe and ThreadingHTTPServer may overlap requests, so `body()` is
    lock-guarded (the only real client is one backend — contention is nil).
    """

    def __init__(self, store, league: str, date_param: str):
        self._store = store
        self._lock = threading.Lock()
        rows = [
            (requested_at, body_hash)
            for requested_at, http_status, body_hash in store.iter_stream(league, date_param)
            if http_status == 200
        ]
        _require(bool(rows), f"no 200 scoreboard polls for {league} {date_param}")
        self.capture_t0 = rows[0][0]
        self.scoreboard = [
            ((at - self.capture_t0).total_seconds(), h) for at, h in rows
        ]
        # Summary streams: event ids come from the day's distinct bodies.
        event_ids: set[str] = set()
        seen_hashes: set[str] = set()
        for _offset, body_hash in self.scoreboard:
            if body_hash in seen_hashes:
                continue
            seen_hashes.add(body_hash)
            try:
                events = json.loads(store.fetch_body(body_hash)).get("events", [])
            except ValueError:
                continue
            for event in events:
                if isinstance(event, dict) and event.get("id"):
                    event_ids.add(str(event["id"]))
        self.summaries: dict[str, list[tuple[float, str]]] = {}
        for event_id in sorted(event_ids):
            records = [
                ((at - self.capture_t0).total_seconds(), h)
                for at, status, h in store.iter_summary_stream(league, event_id)
                if status == 200
            ]
            if records:
                self.summaries[event_id] = records

    def body(self, key: str) -> bytes:
        with self._lock:
            return self._store.fetch_body(key)

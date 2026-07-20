"""Per-game capture-quality classification.

Walks each (league, game-day) scoreboard stream in poll order and reports,
per event: how early capture started, whether pregame/final were observed,
the worst poll gap during the live window, and summary-poll counts — with an
explicit REPLAY_GRADE verdict so "do I have a clean game to replay?" is a
computed fact, not a judgment call. Gaps that overlap a window where no
collector session was running are annotated `down`, distinguishing "collector
was off" from "collector was up and slow".

Bodies dedup by hash, so each distinct body is parsed exactly once — the
parse count equals the change count, not the poll count.
"""

import json
from datetime import datetime, timezone

from .db import Store

# The replay-grade bar (see also BACKLOG: bundle exports gate on this).
REPLAY_PREGAME_LEAD_S = 30 * 60   # first sighting at least this long before start
REPLAY_MAX_LIVE_GAP_S = 120       # worst poll-to-poll gap while live
REPLAY_MIN_POST_POLLS = 2         # polls observing the final state


def _parse_events(body: bytes) -> dict[str, dict]:
    """event_id -> {state, start, label} for one scoreboard body."""
    try:
        events = json.loads(body).get("events", [])
    except (ValueError, AttributeError):
        return {}
    out: dict[str, dict] = {}
    for event in events:
        event_id = event.get("id")
        if not event_id:
            continue
        competitions = event.get("competitions") or [{}]
        competition = competitions[0]
        state = competition.get("status", {}).get("type", {}).get("state") or "unknown"
        label = event.get("shortName") or event.get("name") or event_id
        out[str(event_id)] = {"state": state, "start": event.get("date"), "label": label}
    return out


def _parse_start(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%MZ").replace(tzinfo=timezone.utc)
    except ValueError:
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return None


class _EventCoverage:
    def __init__(self, event_id: str):
        self.event_id = event_id
        self.label = event_id
        self.start: datetime | None = None
        self.first_seen: datetime | None = None
        self.pregame_seen = False
        self.first_in: datetime | None = None
        self.first_post: datetime | None = None
        self.post_polls = 0
        self.max_live_gap_s = 0.0
        self.gap_overlaps_downtime = False
        self.summary_polls = 0
        self._last_live_poll: datetime | None = None

    def observe(self, at: datetime, state: str, start: str | None, label: str) -> None:
        self.label = label
        if self.start is None:
            self.start = _parse_start(start)
        if self.first_seen is None:
            self.first_seen = at
        if state == "pre":
            self.pregame_seen = True
        elif state == "in":
            if self.first_in is None:
                self.first_in = at
        elif state == "post":
            if self.first_post is None:
                self.first_post = at
            self.post_polls += 1

    def observe_gap(self, prev: datetime, at: datetime, in_downtime: bool) -> None:
        """Called for each consecutive 200-poll pair while this event is live."""
        gap = (at - prev).total_seconds()
        if gap > self.max_live_gap_s:
            self.max_live_gap_s = gap
            self.gap_overlaps_downtime = in_downtime

    def live_window_contains(self, at: datetime) -> bool:
        return (
            self.first_in is not None
            and at >= self.first_in
            and (self.first_post is None or at <= self.first_post)
        )

    @property
    def lead_s(self) -> float | None:
        if self.start is None or self.first_seen is None:
            return None
        return (self.start - self.first_seen).total_seconds()

    def verdict(self) -> tuple[bool, list[str]]:
        problems = []
        if self.lead_s is None:
            problems.append("no start time")
        elif self.lead_s < REPLAY_PREGAME_LEAD_S:
            problems.append(f"lead {self.lead_s / 60:.0f}m < 30m")
        if not self.pregame_seen:
            problems.append("no pregame")
        if self.first_in is None:
            problems.append("no live polls")
        elif self.max_live_gap_s > REPLAY_MAX_LIVE_GAP_S:
            kind = "down" if self.gap_overlaps_downtime else "gap"
            problems.append(f"live {kind} {self.max_live_gap_s:.0f}s")
        if self.first_post is None:
            problems.append("no final")
        elif self.post_polls < REPLAY_MIN_POST_POLLS:
            problems.append(f"post polls {self.post_polls} < {REPLAY_MIN_POST_POLLS}")
        return (not problems, problems)


def _in_downtime(windows: list[tuple[datetime, datetime]], prev: datetime, at: datetime) -> bool:
    """True when any part of [prev, at] is outside every session window."""
    cursor = prev
    for started, ended in windows:
        if ended <= cursor:
            continue
        if started > cursor:
            return True                 # uncovered stretch starting at cursor
        cursor = max(cursor, ended)
        if cursor >= at:
            return False
    return cursor < at


def coverage_report(store: Store, league: str | None, date: str | None) -> list[dict]:
    windows = store.session_windows()
    summary_by_league: dict[str, dict[str, int]] = {}
    reports = []
    for stream_league, date_param in store.scoreboard_streams(league):
        if date and date_param != date:
            continue
        if stream_league not in summary_by_league:
            summary_by_league[stream_league] = store.summary_counts(stream_league)

        events: dict[str, _EventCoverage] = {}
        parsed_cache: dict[str, dict[str, dict]] = {}
        prev_ok: datetime | None = None
        for requested_at, http_status, body_hash in store.iter_stream(stream_league, date_param):
            if http_status != 200:
                continue
            if body_hash not in parsed_cache:
                parsed_cache[body_hash] = _parse_events(store.fetch_body(body_hash))
            snapshot = parsed_cache[body_hash]
            for event_id, info in snapshot.items():
                cov = events.setdefault(event_id, _EventCoverage(event_id))
                cov.observe(requested_at, info["state"], info["start"], info["label"])
            if prev_ok is not None:
                down = _in_downtime(windows, prev_ok, requested_at)
                for cov in events.values():
                    if cov.live_window_contains(requested_at) and cov.live_window_contains(prev_ok):
                        cov.observe_gap(prev_ok, requested_at, down)
            prev_ok = requested_at

        for cov in events.values():
            cov.summary_polls = summary_by_league[stream_league].get(cov.event_id, 0)
            ok, problems = cov.verdict()
            reports.append(
                {
                    "league": stream_league,
                    "date": date_param,
                    "event_id": cov.event_id,
                    "label": cov.label,
                    "start": cov.start.isoformat() if cov.start else None,
                    "lead_min": round(cov.lead_s / 60) if cov.lead_s is not None else None,
                    "pregame": cov.pregame_seen,
                    "max_live_gap_s": round(cov.max_live_gap_s),
                    "post_polls": cov.post_polls,
                    "final": cov.first_post is not None,
                    "summary_polls": cov.summary_polls,
                    "replay_grade": ok,
                    "problems": problems,
                }
            )
    return reports


def print_report(reports: list[dict]) -> None:
    if not reports:
        print("no scoreboard streams found")
        return
    current = None
    for row in reports:
        head = (row["league"], row["date"])
        if head != current:
            current = head
            print(f"\n{row['league']}  {row['date']}")
            print(
                f"  {'event':<22}{'lead':>6}{'pre':>5}{'gap':>6}{'post':>6}"
                f"{'fin':>5}{'summ':>6}  verdict"
            )
        lead = f"{row['lead_min']}m" if row["lead_min"] is not None else "?"
        verdict = "REPLAY-GRADE" if row["replay_grade"] else ", ".join(row["problems"])
        print(
            f"  {row['label']:<22.22}{lead:>6}{'y' if row['pregame'] else '-':>5}"
            f"{row['max_live_gap_s']:>5}s{row['post_polls']:>6}"
            f"{'y' if row['final'] else '-':>5}{row['summary_polls']:>6}  {verdict}"
        )

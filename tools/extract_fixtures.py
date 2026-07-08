"""Extract real ESPN event fixtures from the sample store into backend/testdata/.

Fixtures are single event objects (not whole scoreboards), one per game state,
taken from live-captured payloads so backend tests exercise exactly what ESPN
serves. Regenerate any time with:  python tools/extract_fixtures.py

Fixtures are grouped per league; each group scans that league's 200-responses
newest-first and keeps the first event matching each named predicate.
"""

import json
import sqlite3
import sys
import zlib
from pathlib import Path
from typing import Callable

REPO_ROOT = Path(__file__).resolve().parents[1]
DB = REPO_ROOT / "data" / "espn" / "espn.db"
TESTDATA = REPO_ROOT / "backend" / "testdata"

# A predicate sees the whole event and its first competition.
Predicate = Callable[[dict, dict], bool]


def _description(target: str, extra: Callable[[dict], bool] = lambda c: True) -> Predicate:
    """Match on soccer's `status.type.description`, with an optional extra check."""
    return lambda _ev, c: c["status"]["type"].get("description") == target and extra(c)


def _is_numeric(value) -> bool:
    """True when `value` parses as a number — i.e. it is ESPN's conditionId code,
    not human-readable condition text (see the weather-swap bug)."""
    if value is None:
        return False
    try:
        float(value)
        return True
    except (TypeError, ValueError):
        return False


def _mlb_state(_ev: dict, c: dict) -> str:
    return c["status"]["type"]["state"]


def _mlb_short_detail(c: dict) -> str:
    return c["status"]["type"].get("shortDetail", "")


def _pregame_swapped(ev: dict, c: dict) -> bool:
    # Swapped orientation: displayValue holds the numeric conditionId code.
    w = ev.get("weather")
    return _mlb_state(ev, c) == "pre" and bool(w) and _is_numeric(w.get("displayValue"))


def _pregame_normal(ev: dict, c: dict) -> bool:
    # Normal orientation: displayValue holds the condition text.
    w = ev.get("weather")
    return (
        _mlb_state(ev, c) == "pre"
        and bool(w)
        and bool(w.get("displayValue"))
        and not _is_numeric(w.get("displayValue"))
    )


def _live_inning(ev: dict, c: dict) -> bool:
    prefix = _mlb_short_detail(c).split()[:1]
    return _mlb_state(ev, c) == "in" and prefix and prefix[0] in ("Top", "Mid", "Bot", "End")


def _rain_delay(ev: dict, c: dict) -> bool:
    return _mlb_state(ev, c) == "in" and _mlb_short_detail(c).startswith("Rain")


def _final(ev: dict, c: dict) -> bool:
    return _mlb_state(ev, c) == "post"


# (league, output subdir, [(fixture name, predicate)])
GROUPS: list[tuple[str, str, list[tuple[str, Predicate]]]] = [
    (
        "fifa.world",
        "soccer",
        [
            ("pregame", _description("Scheduled")),
            ("first_half", _description("First Half")),
            ("halftime", _description("Halftime")),
            (
                "second_half_stoppage",
                _description(
                    "Second Half",
                    lambda c: "+" in c["status"]["displayClock"] and bool(c.get("details")),
                ),
            ),
            ("full_time", _description("Full Time")),
        ],
    ),
    (
        "mlb",
        "mlb",
        [
            ("pregame", _pregame_swapped),
            ("pregame_weather_normal", _pregame_normal),
            ("live_inning", _live_inning),
            ("rain_delay", _rain_delay),
            ("final", _final),
        ],
    ),
]


def extract_group(
    conn: sqlite3.Connection, league: str, targets: list[tuple[str, Predicate]]
) -> dict[str, dict]:
    found: dict[str, dict] = {}
    rows = conn.execute(
        """
        SELECT b.body FROM responses r JOIN bodies b ON b.hash = r.body_hash
        WHERE r.league = ? AND r.http_status = 200
        ORDER BY r.epoch DESC
        """,
        (league,),
    )
    for (blob,) in rows:
        if len(found) == len(targets):
            break
        body = json.loads(zlib.decompress(blob))
        for event in body.get("events", []):
            competitions = event.get("competitions")
            if not competitions:
                continue
            competition = competitions[0]
            for name, predicate in targets:
                if name not in found and predicate(event, competition):
                    found[name] = event
    return found


def main() -> int:
    conn = sqlite3.connect(f"file:{DB.as_posix()}?mode=ro", uri=True)
    complete = True
    for league, subdir, targets in GROUPS:
        out = TESTDATA / subdir
        out.mkdir(parents=True, exist_ok=True)
        found = extract_group(conn, league, targets)
        for name, _predicate in targets:
            if name not in found:
                print(f"MISSING {league} fixture {name!r} — collect more data first")
                complete = False
                continue
            path = out / f"{name}.json"
            path.write_text(json.dumps(found[name], indent=2), encoding="utf-8")
            print(f"wrote {path}")
    conn.close()
    return 0 if complete else 1


if __name__ == "__main__":
    sys.exit(main())

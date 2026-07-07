"""Extract real ESPN event fixtures from the sample store into backend/testdata/.

Fixtures are single event objects (not whole scoreboards), one per game state,
taken from live-captured payloads so backend tests exercise exactly what ESPN
serves. Regenerate any time with:  python tools/extract_fixtures.py
"""

import json
import sqlite3
import sys
import zlib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DB = REPO_ROOT / "data" / "espn" / "espn.db"
OUT = REPO_ROOT / "backend" / "testdata" / "soccer"

# (fixture name, status.type.description to match, extra predicate)
TARGETS = [
    ("pregame", "Scheduled", lambda c: True),
    ("first_half", "First Half", lambda c: True),
    ("halftime", "Halftime", lambda c: True),
    # prefer a second-half event in stoppage time with scoring details
    (
        "second_half_stoppage",
        "Second Half",
        lambda c: "+" in c["status"]["displayClock"] and c.get("details"),
    ),
    ("full_time", "Full Time", lambda c: True),
]


def main() -> int:
    conn = sqlite3.connect(f"file:{DB.as_posix()}?mode=ro", uri=True)
    OUT.mkdir(parents=True, exist_ok=True)
    found: dict[str, dict] = {}
    rows = conn.execute(
        """
        SELECT b.body FROM responses r JOIN bodies b ON b.hash = r.body_hash
        WHERE r.league = 'fifa.world' AND r.http_status = 200
        ORDER BY r.epoch DESC
        """
    )
    for (blob,) in rows:
        if len(found) == len(TARGETS):
            break
        body = json.loads(zlib.decompress(blob))
        for event in body.get("events", []):
            competitions = event.get("competitions")
            if not competitions:
                continue
            competition = competitions[0]
            description = competition["status"]["type"].get("description")
            for name, target, predicate in TARGETS:
                if name not in found and description == target and predicate(competition):
                    found[name] = event
    conn.close()

    for name, target, _ in TARGETS:
        if name not in found:
            print(f"MISSING fixture for {target!r} — collect more data first")
            continue
        path = OUT / f"{name}.json"
        path.write_text(json.dumps(found[name], indent=2), encoding="utf-8")
        print(f"wrote {path}")
    return 0 if len(found) == len(TARGETS) else 1


if __name__ == "__main__":
    sys.exit(main())

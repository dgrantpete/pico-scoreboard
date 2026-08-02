"""Replay bundles: self-contained `.espnbundle` files exported from the store.

A bundle is a zip holding one captured (league, game-day) stream —
`manifest.json` (offsets + hashes + coverage verdicts) plus hash-deduped
`bodies/<sha256>.json` members — so the deployed mock replays a great game
with no database anywhere near it. Export is gated on `coverage`'s
replay-grade predicate: a bundle you can make without `--force` is a bundle
worth demoing.
"""

import json
import zipfile
from collections import OrderedDict
from datetime import datetime, timezone
from pathlib import Path

from .coverage import coverage_report
from .mockdata import MockError

BUNDLE_VERSION = 1
_BODY_CACHE_SIZE = 16


def export_bundle(store, sport: str, league: str, date: str, out_path: Path,
                  force: bool = False) -> Path:
    """Write one (league, date) capture to `out_path`. Refuses when no event
    is replay-grade unless `force`."""
    reports = coverage_report(store, league, date)
    if not reports:
        raise MockError(f"no scoreboard stream for {league} {date}")
    grade = [r for r in reports if r["replay_grade"]]
    for row in reports:
        verdict = "REPLAY-GRADE" if row["replay_grade"] else ", ".join(row["problems"])
        print(f"  {row['label']:<24.24} {verdict}")
    if not grade and not force:
        raise MockError(
            f"{league} {date}: no replay-grade event — capture a full game or use --force"
        )

    rows = [
        (requested_at, body_hash)
        for requested_at, http_status, body_hash in store.iter_stream(league, date)
        if http_status == 200
    ]
    if not rows:
        raise MockError(f"no 200 scoreboard polls for {league} {date}")
    capture_t0 = rows[0][0]
    scoreboard = [((at - capture_t0).total_seconds(), h) for at, h in rows]

    event_ids = {str(r["event_id"]) for r in reports}
    summaries: dict[str, list] = {}
    for event_id in sorted(event_ids):
        records = [
            ((at - capture_t0).total_seconds(), h)
            for at, status, h in store.iter_summary_stream(league, event_id)
            if status == 200
        ]
        if records:
            summaries[event_id] = records

    manifest = {
        "version": BUNDLE_VERSION,
        "sport": sport,
        "league": league,
        "date": date,
        "capture_t0": capture_t0.isoformat(),
        "exported_at": datetime.now(timezone.utc).isoformat(),
        "source": "espn-postgres-v2",
        "scoreboard": scoreboard,
        "summaries": summaries,
        "coverage": reports,
    }

    hashes = {h for _off, h in scoreboard}
    hashes.update(h for records in summaries.values() for _off, h in records)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(out_path, "w") as zf:
        zf.writestr(
            "manifest.json",
            json.dumps(manifest, indent=2),
            compress_type=zipfile.ZIP_STORED,
        )
        for body_hash in sorted(hashes):
            zf.writestr(
                f"bodies/{body_hash}.json",
                store.fetch_body(body_hash),
                compress_type=zipfile.ZIP_DEFLATED,
            )
    size = out_path.stat().st_size
    print(f"wrote {out_path} ({size / 1e6:.1f} MB, {len(hashes)} distinct bodies)")
    return out_path


class BundleReader:
    """ReplaySource over an .espnbundle zip: manifest parsed once, bodies
    random-accessed with a small LRU so a 256 MB VM never holds a day in RAM."""

    def __init__(self, path: Path):
        if not path.exists():
            raise MockError(f"bundle not found: {path}")
        self._zip = zipfile.ZipFile(path)
        try:
            manifest = json.loads(self._zip.read("manifest.json"))
        except (KeyError, ValueError) as exc:
            raise MockError(f"unreadable bundle manifest in {path}: {exc}")
        if manifest.get("version") != BUNDLE_VERSION:
            raise MockError(
                f"bundle {path} is version {manifest.get('version')!r}; "
                f"this code reads version {BUNDLE_VERSION}"
            )
        self.capture_t0 = datetime.fromisoformat(manifest["capture_t0"])
        self.scoreboard = [(float(off), h) for off, h in manifest["scoreboard"]]
        self.summaries = {
            event_id: [(float(off), h) for off, h in records]
            for event_id, records in manifest.get("summaries", {}).items()
        }
        self._cache: OrderedDict[str, bytes] = OrderedDict()

    def body(self, key: str) -> bytes:
        cached = self._cache.get(key)
        if cached is not None:
            self._cache.move_to_end(key)
            return cached
        body = self._zip.read(f"bodies/{key}.json")
        self._cache[key] = body
        if len(self._cache) > _BODY_CACHE_SIZE:
            self._cache.popitem(last=False)
        return body

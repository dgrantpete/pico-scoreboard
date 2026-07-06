"""One-shot import of the legacy espn_data_samples collections into the unified store.

Each legacy row's body is integrity-checked against its stored MD5 before being
re-hashed with sha256 and inserted. Verification (row counts, distinct-hash
parity, random byte-exact spot checks) runs after the import; the caller should
only delete the legacy directory when this module reports success.
"""

import hashlib
import random
import sqlite3
from pathlib import Path

from .db import Store

LEGACY_SOURCES = [
    # (db path relative to the legacy dir, sport, league, source tag)
    ("espn_collection/scoreboard_responses.db", "baseball", "mlb", "legacy:espn_collection"),
    ("mlb_collection/scoreboard_responses.db", "baseball", "mlb", "legacy:mlb_collection"),
    ("nba_collection/scoreboard_responses.db", "basketball", "nba", "legacy:nba_collection"),
]

BATCH_SIZE = 100
SPOT_CHECK_SAMPLES = 25


def migrate(store: Store, legacy_dir: Path, *, dry_run: bool = False) -> bool:
    """Import all legacy DBs; returns True only if every check passes."""
    existing = store.count_source("legacy:%")
    if existing and not dry_run:
        print(f"refusing to run: store already has {existing} legacy rows (one-shot import)")
        return False

    ok = True
    for rel_path, sport, league, source in LEGACY_SOURCES:
        db_path = legacy_dir / rel_path
        if not db_path.exists():
            print(f"FAIL {source}: missing {db_path}")
            ok = False
            continue
        ok &= _import_one(store, db_path, sport, league, source, dry_run=dry_run)

    if ok and not dry_run:
        for rel_path, _, _, source in LEGACY_SOURCES:
            ok &= _spot_check(store, legacy_dir / rel_path, source)
    print("migration " + ("OK" if ok else "FAILED"))
    return ok


def _open_legacy(db_path: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True)


def _import_one(
    store: Store, db_path: Path, sport: str, league: str, source: str, *, dry_run: bool
) -> bool:
    legacy = _open_legacy(db_path)
    try:
        total, distinct = legacy.execute(
            "SELECT COUNT(*), COUNT(DISTINCT body_hash) FROM responses"
        ).fetchone()
        imported = 0
        md5_mismatches = 0
        cursor = legacy.execute(
            "SELECT timestamp, epoch, date_param, http_status, max_age, body_hash,"
            " body, headers FROM responses ORDER BY id"
        )
        for row in cursor:
            timestamp, epoch, date_param, http_status, max_age, legacy_md5, body, headers = row
            body_bytes = body.encode("utf-8")
            if hashlib.md5(body_bytes).hexdigest() != legacy_md5:
                md5_mismatches += 1
                continue
            if not dry_run:
                store.insert_response(
                    sport=sport,
                    league=league,
                    date_param=str(date_param),
                    requested_at=timestamp,
                    epoch=epoch,
                    http_status=http_status,
                    max_age=max_age,
                    body=body_bytes,
                    headers_json=headers,
                    source=source,
                )
            imported += 1
            if not dry_run and imported % BATCH_SIZE == 0:
                store.commit()
        if not dry_run:
            store.commit()

        if dry_run:
            print(f"dry-run {source}: {total} rows, {distinct} distinct bodies,"
                  f" {md5_mismatches} md5 mismatches")
            return md5_mismatches == 0

        migrated = store.count_source(source)
        migrated_distinct = store.count_distinct_for_source(source)
        ok = md5_mismatches == 0 and migrated == total and migrated_distinct == distinct
        print(
            f"{'OK  ' if ok else 'FAIL'} {source}: {migrated}/{total} rows,"
            f" {migrated_distinct}/{distinct} distinct bodies, {md5_mismatches} md5 mismatches"
        )
        return ok
    finally:
        legacy.close()


def _spot_check(store: Store, db_path: Path, source: str) -> bool:
    """Byte-compare random legacy bodies against what the store returns for them."""
    legacy = _open_legacy(db_path)
    try:
        ids = [row[0] for row in legacy.execute("SELECT id FROM responses")]
        failures = 0
        for legacy_id in random.sample(ids, min(SPOT_CHECK_SAMPLES, len(ids))):
            body, epoch = legacy.execute(
                "SELECT body, epoch FROM responses WHERE id = ?", (legacy_id,)
            ).fetchone()
            body_bytes = body.encode("utf-8")
            sha = hashlib.sha256(body_bytes).hexdigest()
            stored = store.get_body(source=source, epoch=epoch, body_hash=sha)
            if stored != body_bytes:
                failures += 1
        checked = min(SPOT_CHECK_SAMPLES, len(ids))
        print(f"{'OK  ' if not failures else 'FAIL'} {source}: spot check"
              f" {checked - failures}/{checked} byte-exact")
        return failures == 0
    finally:
        legacy.close()

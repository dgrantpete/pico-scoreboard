"""Unified SQLite store for raw ESPN scoreboard responses.

All SQL in the package lives here. Bodies are content-addressed by sha256
and stored zlib-compressed (level 6); `responses` rows reference them by
hash, so repeated identical payloads are stored once.
"""

import hashlib
import sqlite3
import zlib
from pathlib import Path

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS bodies (
    hash TEXT PRIMARY KEY,      -- sha256 hex of raw (uncompressed) body bytes
    body BLOB NOT NULL,         -- zlib.compress(raw, 6)
    size INTEGER NOT NULL       -- uncompressed byte count
);
CREATE TABLE IF NOT EXISTS responses (
    id           INTEGER PRIMARY KEY,
    sport        TEXT NOT NULL,
    league       TEXT NOT NULL,
    date_param   TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    epoch        REAL NOT NULL,
    http_status  INTEGER NOT NULL,
    max_age      INTEGER,
    body_hash    TEXT NOT NULL REFERENCES bodies(hash),
    headers      TEXT NOT NULL,
    source       TEXT NOT NULL DEFAULT 'live'
);
CREATE INDEX IF NOT EXISTS idx_responses_stream ON responses(league, date_param, epoch);
CREATE INDEX IF NOT EXISTS idx_responses_hash ON responses(body_hash);
"""


class Store:
    """Owns the single SQLite connection; constructed once per process and injected."""

    def __init__(self, path: str | Path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self.path)
        self._conn.execute("PRAGMA journal_mode = WAL")
        self._conn.execute("PRAGMA busy_timeout = 30000")
        self._conn.executescript(_SCHEMA)
        self._conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '1')"
        )
        self._conn.commit()

    def close(self) -> None:
        self._conn.close()

    def commit(self) -> None:
        self._conn.commit()

    def insert_response(
        self,
        *,
        sport: str,
        league: str,
        date_param: str,
        requested_at: str,
        epoch: float,
        http_status: int,
        max_age: int | None,
        body: bytes,
        headers_json: str,
        source: str = "live",
    ) -> str:
        """Insert one response without committing; returns the body's sha256."""
        body_hash = hashlib.sha256(body).hexdigest()
        known = self._conn.execute(
            "SELECT 1 FROM bodies WHERE hash = ?", (body_hash,)
        ).fetchone()
        if known is None:
            self._conn.execute(
                "INSERT OR IGNORE INTO bodies (hash, body, size) VALUES (?, ?, ?)",
                (body_hash, zlib.compress(body, 6), len(body)),
            )
        self._conn.execute(
            "INSERT INTO responses (sport, league, date_param, requested_at, epoch,"
            " http_status, max_age, body_hash, headers, source)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (sport, league, date_param, requested_at, epoch, http_status, max_age,
             body_hash, headers_json, source),
        )
        return body_hash

    def iter_bodies(
        self,
        league: str,
        *,
        http_status: int = 200,
        distinct: bool = True,
        source_like: str = "%",
    ):
        """Yield raw (decompressed) body bytes for a league."""
        if distinct:
            sql = (
                "SELECT body FROM bodies WHERE hash IN ("
                " SELECT DISTINCT body_hash FROM responses"
                " WHERE league = ? AND http_status = ? AND source LIKE ?)"
            )
        else:
            sql = (
                "SELECT b.body FROM responses r JOIN bodies b ON b.hash = r.body_hash"
                " WHERE r.league = ? AND r.http_status = ? AND r.source LIKE ?"
                " ORDER BY r.epoch"
            )
        for (blob,) in self._conn.execute(sql, (league, http_status, source_like)):
            yield zlib.decompress(blob)

    def count_source(self, source_like: str) -> int:
        return self._conn.execute(
            "SELECT COUNT(*) FROM responses WHERE source LIKE ?", (source_like,)
        ).fetchone()[0]

    def count_distinct_for_source(self, source: str) -> int:
        return self._conn.execute(
            "SELECT COUNT(DISTINCT body_hash) FROM responses WHERE source = ?", (source,)
        ).fetchone()[0]

    def get_body(self, *, source: str, epoch: float, body_hash: str) -> bytes | None:
        """Raw body bytes for one specific response row, or None if absent."""
        row = self._conn.execute(
            "SELECT b.body FROM responses r JOIN bodies b ON b.hash = r.body_hash"
            " WHERE r.source = ? AND r.epoch = ? AND r.body_hash = ?",
            (source, epoch, body_hash),
        ).fetchone()
        return zlib.decompress(row[0]) if row else None

    def league_stats(self) -> list[tuple]:
        """Per-(sport, league) summary rows for the status subcommand.

        `changed` counts polls whose body differs from the previous poll of the
        same (league, date_param) stream — computed here, never stored.
        """
        return self._conn.execute(
            """
            SELECT sport, league,
                   COUNT(*),
                   COUNT(DISTINCT body_hash),
                   COUNT(DISTINCT date_param),
                   MIN(requested_at), MAX(requested_at),
                   SUM(http_status != 200),
                   SUM(changed)
            FROM (
                SELECT *,
                       body_hash != LAG(body_hash) OVER (
                           PARTITION BY league, date_param ORDER BY epoch
                       ) AS changed
                FROM responses
            )
            GROUP BY sport, league ORDER BY sport, league
            """
        ).fetchall()

    def body_totals(self) -> tuple[int, int, int]:
        """(unique bodies, raw bytes, stored/compressed bytes)."""
        return self._conn.execute(
            "SELECT COUNT(*), COALESCE(SUM(size), 0), COALESCE(SUM(LENGTH(body)), 0)"
            " FROM bodies"
        ).fetchone()

"""Postgres store for raw ESPN responses (schema v2). All SQL lives here.

Bodies are content-addressed by sha256 of the raw bytes and stored
zlib-compressed; `responses` rows reference them by hash, so identical
payloads store once. Provenance is structural: every response row carries the
`collector_sessions` FK of the service run that fetched it, and each session
row records the effective targets config it ran with — "what was being
collected at time T" is a query, not a guess.

Connection model: one `Store` = one connection, single-threaded caller
(the collector's scheduler, or one CLI invocation). `heartbeat()` is the
exception — it is called from the service's heartbeat thread and opens a
short-lived connection per beat, because psycopg connections are not
thread-safe and a beat per minute doesn't justify a pool.
"""

import hashlib
import zlib
from datetime import datetime

import psycopg
from psycopg.types.json import Jsonb

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    key   text PRIMARY KEY,
    value text NOT NULL
);
CREATE TABLE IF NOT EXISTS bodies (
    hash text PRIMARY KEY,          -- sha256 hex of raw (uncompressed) body bytes
    body bytea NOT NULL,            -- zlib.compress(raw, 6)
    size integer NOT NULL           -- uncompressed byte count
);
CREATE TABLE IF NOT EXISTS collector_sessions (
    id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at     timestamptz NOT NULL,
    last_heartbeat timestamptz NOT NULL,
    ended_at       timestamptz,
    end_reason     text CHECK (end_reason IN ('shutdown', 'reload', 'crash')),
    hostname       text NOT NULL,
    version        text NOT NULL,
    targets        jsonb NOT NULL
);
CREATE TABLE IF NOT EXISTS responses (
    id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    session_id   bigint NOT NULL REFERENCES collector_sessions(id),
    target       text NOT NULL,
    endpoint     text NOT NULL CHECK (endpoint IN ('scoreboard', 'summary')),
    sport        text NOT NULL,
    league       text NOT NULL,
    event_id     text,
    date_param   text,
    requested_at timestamptz NOT NULL,
    http_status  integer NOT NULL,
    max_age      integer,
    body_hash    text NOT NULL REFERENCES bodies(hash),
    headers      jsonb NOT NULL,
    CHECK ((endpoint = 'scoreboard') = (date_param IS NOT NULL)),
    CHECK ((endpoint = 'summary') = (event_id IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS idx_responses_stream
    ON responses (league, endpoint, date_param, requested_at);
CREATE INDEX IF NOT EXISTS idx_responses_event
    ON responses (league, event_id, requested_at) WHERE event_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_responses_hash ON responses (body_hash);
"""

# Change-count window: consecutive polls of the same stream. Scoreboard
# streams are (league, date_param); summary streams are (league, event_id) —
# endpoint in the partition keys them apart (the NULL columns are constant
# within each).
_STREAM_PARTITION = "PARTITION BY league, endpoint, date_param, event_id ORDER BY requested_at"


class Store:
    def __init__(self, dsn: str):
        self._dsn = dsn
        self._conn = psycopg.connect(dsn)

    def close(self) -> None:
        self._conn.close()

    def commit(self) -> None:
        self._conn.commit()

    def reconnect(self) -> None:
        """Recover from a dropped connection (Postgres restart, network blip).
        The in-flight transaction is lost; callers treat that as one failed
        poll and lean on their existing backoff."""
        try:
            self._conn.close()
        except Exception:
            pass
        self._conn = psycopg.connect(self._dsn)

    # -- schema & sessions (service write path) ----------------------------

    def ensure_schema(self) -> None:
        self._conn.execute(_SCHEMA)
        self._conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', '2')"
            " ON CONFLICT (key) DO NOTHING"
        )
        self._conn.commit()

    def sweep_orphan_sessions(self) -> int:
        """Close sessions a crashed process left open; their last heartbeat is
        the honest end of coverage."""
        cur = self._conn.execute(
            "UPDATE collector_sessions"
            " SET ended_at = last_heartbeat, end_reason = 'crash'"
            " WHERE ended_at IS NULL"
        )
        self._conn.commit()
        return cur.rowcount

    def start_session(self, *, hostname: str, version: str, targets: list[dict]) -> int:
        cur = self._conn.execute(
            "INSERT INTO collector_sessions"
            " (started_at, last_heartbeat, hostname, version, targets)"
            " VALUES (now(), now(), %s, %s, %s) RETURNING id",
            (hostname, version, Jsonb(targets)),
        )
        session_id = cur.fetchone()[0]
        self._conn.commit()
        return session_id

    def heartbeat(self, session_id: int) -> None:
        """Thread-safe: own short-lived connection (see module docstring)."""
        with psycopg.connect(self._dsn) as conn:
            conn.execute(
                "UPDATE collector_sessions SET last_heartbeat = now() WHERE id = %s",
                (session_id,),
            )

    def end_session(self, session_id: int, reason: str) -> None:
        self._conn.execute(
            "UPDATE collector_sessions"
            " SET ended_at = now(), last_heartbeat = now(), end_reason = %s"
            " WHERE id = %s AND ended_at IS NULL",
            (reason, session_id),
        )
        self._conn.commit()

    # -- ingest ------------------------------------------------------------

    def insert_response(
        self,
        *,
        session_id: int,
        target: str,
        endpoint: str,
        sport: str,
        league: str,
        event_id: str | None,
        date_param: str | None,
        requested_at: datetime,
        http_status: int,
        max_age: int | None,
        body: bytes,
        headers: dict,
    ) -> str:
        """Insert one response without committing; returns the body's sha256.
        The existence pre-check keeps duplicate (unchanged) bodies from being
        shipped over the wire on every poll."""
        body_hash = hashlib.sha256(body).hexdigest()
        known = self._conn.execute(
            "SELECT 1 FROM bodies WHERE hash = %s", (body_hash,)
        ).fetchone()
        if known is None:
            self._conn.execute(
                "INSERT INTO bodies (hash, body, size) VALUES (%s, %s, %s)"
                " ON CONFLICT (hash) DO NOTHING",
                (body_hash, zlib.compress(body, 6), len(body)),
            )
        self._conn.execute(
            "INSERT INTO responses (session_id, target, endpoint, sport, league,"
            " event_id, date_param, requested_at, http_status, max_age, body_hash, headers)"
            " VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)",
            (session_id, target, endpoint, sport, league, event_id, date_param,
             requested_at, http_status, max_age, body_hash, Jsonb(headers)),
        )
        return body_hash

    # -- analysis reads ----------------------------------------------------

    def iter_bodies(
        self,
        league: str,
        *,
        endpoint: str = "scoreboard",
        http_status: int = 200,
        distinct: bool = True,
    ):
        """Yield raw (decompressed) body bytes for a league."""
        if distinct:
            sql = (
                "SELECT body FROM bodies WHERE hash IN ("
                " SELECT DISTINCT body_hash FROM responses"
                " WHERE league = %s AND endpoint = %s AND http_status = %s)"
            )
        else:
            sql = (
                "SELECT b.body FROM responses r JOIN bodies b ON b.hash = r.body_hash"
                " WHERE r.league = %s AND r.endpoint = %s AND r.http_status = %s"
                " ORDER BY r.requested_at"
            )
        for (blob,) in self._conn.execute(sql, (league, endpoint, http_status)):
            yield zlib.decompress(blob)

    def league_stats(self) -> list[tuple]:
        """Per-(sport, league, endpoint) summary rows for the status subcommand."""
        return self._conn.execute(
            f"""
            SELECT sport, league, endpoint,
                   COUNT(*),
                   COUNT(DISTINCT body_hash),
                   COUNT(DISTINCT date_param),
                   MIN(requested_at), MAX(requested_at),
                   COUNT(*) FILTER (WHERE http_status <> 200),
                   COUNT(*) FILTER (WHERE changed)
            FROM (
                SELECT *,
                       body_hash <> LAG(body_hash) OVER ({_STREAM_PARTITION}) AS changed
                FROM responses
            ) t
            GROUP BY sport, league, endpoint ORDER BY sport, league, endpoint
            """
        ).fetchall()

    def latest_bodies_per_league(self) -> list[tuple[str, str, datetime, int | None, bytes]]:
        """Newest 200-status scoreboard body per (sport, league):
        (sport, league, requested_at, max_age, raw decompressed body)."""
        rows = self._conn.execute(
            """
            SELECT t.sport, t.league, t.requested_at, t.max_age, b.body
            FROM (
                SELECT sport, league, requested_at, max_age, body_hash,
                       ROW_NUMBER() OVER (
                           PARTITION BY sport, league ORDER BY requested_at DESC
                       ) AS rn
                FROM responses WHERE http_status = 200 AND endpoint = 'scoreboard'
            ) t JOIN bodies b ON b.hash = t.body_hash
            WHERE t.rn = 1
            """
        ).fetchall()
        return [(s, lg, at, m, zlib.decompress(blob)) for s, lg, at, m, blob in rows]

    def body_totals(self) -> tuple[int, int, int]:
        """(unique bodies, raw bytes, stored/compressed bytes)."""
        return self._conn.execute(
            "SELECT COUNT(*), COALESCE(SUM(size), 0), COALESCE(SUM(octet_length(body)), 0)"
            " FROM bodies"
        ).fetchone()

    # -- coverage reads ----------------------------------------------------

    def scoreboard_streams(self, league: str | None = None) -> list[tuple[str, str]]:
        """(league, date_param) pairs with scoreboard rows, newest day first."""
        sql = (
            "SELECT league, date_param FROM responses WHERE endpoint = 'scoreboard'"
            + (" AND league = %s" if league else "")
            + " GROUP BY league, date_param ORDER BY date_param DESC, league"
        )
        params = (league,) if league else ()
        return self._conn.execute(sql, params).fetchall()

    def iter_stream(self, league: str, date_param: str):
        """(requested_at, http_status, body_hash) for one scoreboard stream,
        in poll order — duplicates included (gap math needs every poll)."""
        return self._conn.execute(
            "SELECT requested_at, http_status, body_hash FROM responses"
            " WHERE league = %s AND endpoint = 'scoreboard' AND date_param = %s"
            " ORDER BY requested_at",
            (league, date_param),
        )

    def iter_summary_stream(self, league: str, event_id: str):
        """(requested_at, http_status, body_hash) for one event's summary
        stream, in poll order."""
        return self._conn.execute(
            "SELECT requested_at, http_status, body_hash FROM responses"
            " WHERE league = %s AND endpoint = 'summary' AND event_id = %s"
            " ORDER BY requested_at",
            (league, event_id),
        )

    def fetch_body(self, body_hash: str) -> bytes:
        row = self._conn.execute(
            "SELECT body FROM bodies WHERE hash = %s", (body_hash,)
        ).fetchone()
        return zlib.decompress(row[0])

    def summary_counts(self, league: str) -> dict[str, int]:
        """Per-event summary poll counts for a league (event ids are unique
        across days, so no date constraint is needed)."""
        rows = self._conn.execute(
            "SELECT event_id, COUNT(*) FROM responses"
            " WHERE league = %s AND endpoint = 'summary' GROUP BY event_id",
            (league,),
        ).fetchall()
        return dict(rows)

    def session_windows(self) -> list[tuple[datetime, datetime]]:
        """(started_at, effective end) per session — an open session's
        heartbeat is its honest frontier."""
        return self._conn.execute(
            "SELECT started_at, COALESCE(ended_at, last_heartbeat)"
            " FROM collector_sessions ORDER BY started_at"
        ).fetchall()

    def recent_sessions(self, limit: int = 5) -> list[tuple]:
        """(id, started_at, last_heartbeat, ended_at, end_reason, hostname,
        version, target names) newest first, for the status footer."""
        return self._conn.execute(
            """
            SELECT id, started_at, last_heartbeat, ended_at, end_reason, hostname, version,
                   (SELECT COALESCE(array_agg(t->>'name'), '{}')
                    FROM jsonb_array_elements(targets) AS t)
            FROM collector_sessions ORDER BY started_at DESC LIMIT %s
            """,
            (limit,),
        ).fetchall()

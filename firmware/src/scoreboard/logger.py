"""
Logging for the Pico Scoreboard: RAM ring buffer + bounded flash persistence.

Three layers, each surviving the failure of the one above it:

1. **RAM ring** — always works, zero dependencies. `error()`/`debug()` record
   into pre-allocated slots and print to the console. Safe to call from
   either core (a lock guards only the slot-index bookkeeping).
2. **Flash** — `/logs/current.log`, written by `flush_to_flash()` (Core 0
   only). Flushes are bounded: on ERROR records (rate-limited to one per
   5 minutes), hourly heartbeat when there are unflushed entries, and at
   deliberate moments (watchdog starvation, reboot, Core-0 crash). Healthy,
   quiet operation performs zero flash writes. Every boot rotates
   current → `/logs/previous.log`, so the prior session's story survives
   exactly one reboot.
3. **USB** — when the network (or all of Core 0) is dead, read the files
   directly: `mpremote cat :/logs/previous.log` (or current.log). When
   Core 0 has crashed out, the script has usually exited, so the REPL is
   *more* available in that state, not less.

Level ownership: `config.json`'s `log.level` is the source of truth; Config
pushes changes here via `set_level()`. The module-level `level` int is read
directly by hot paths to guard expensive f-string builds; `error()`/`debug()`
also check it, so event-frequency callers just call them unguarded.
"""

import os
import time
import _thread

NONE = 0
ERROR = 1
DEBUG = 2

_LEVEL_NAMES = {ERROR: "E", DEBUG: "D"}

# Current log level. Config is the source of truth and pushes changes via
# set_level(); default matches Config's default until it loads.
level = DEBUG

# --- RAM ring -----------------------------------------------------------

_SLOTS = 200
_MAX_MSG = 200

# Ring slots mutated in place: [seq, unix_ts, level, msg]. seq strictly
# increases (masked to stay a small int); slot index = seq % _SLOTS.
_entries = [[0, 0, NONE, ""] for _ in range(_SLOTS)]
_next_seq = 1
_lock = _thread.allocate_lock()

# --- Flash persistence state ---------------------------------------------

LOG_DIR = "/logs"
CURRENT_LOG = LOG_DIR + "/current.log"
PREVIOUS_LOG = LOG_DIR + "/previous.log"

_FLUSH_MIN_INTERVAL_MS = 5 * 60_000     # ERROR-triggered flush rate limit
_HEARTBEAT_INTERVAL_MS = 60 * 60_000    # flush-if-dirty heartbeat

_boot_ms = time.ticks_ms()
_last_flush_ms: int | None = None       # None = never flushed this boot
_flushed_seq = 0                        # highest seq included in last flush
_error_since_flush = False


def set_level(new_level: int) -> None:
    """Set the active log level (called by Config on load/update)."""
    global level
    level = new_level


def _record(lvl: int, msg: str) -> None:
    global _next_seq, _error_since_flush
    if len(msg) > _MAX_MSG:
        msg = msg[:_MAX_MSG]
    with _lock:
        slot = _entries[_next_seq % _SLOTS]
        slot[0] = _next_seq
        slot[1] = time.time()
        slot[2] = lvl
        slot[3] = msg
        _next_seq += 1
    if lvl == ERROR:
        _error_since_flush = True


def error(msg: str) -> None:
    """Record + print an ERROR-level message (no-op below ERROR level)."""
    if level >= ERROR:
        _record(ERROR, msg)
        print(msg)


def debug(msg: str) -> None:
    """Record + print a DEBUG-level message (no-op below DEBUG level)."""
    if level >= DEBUG:
        _record(DEBUG, msg)
        print(msg)


def _snapshot(since_seq: int) -> tuple[list, int]:
    """Copy entries with seq > since_seq, oldest first. Returns (rows, latest_seq)."""
    with _lock:
        latest = _next_seq - 1
        start = latest - _SLOTS + 1
        if start < 1:
            start = 1
        if since_seq + 1 > start:
            start = since_seq + 1
        rows = []
        for seq in range(start, latest + 1):
            slot = _entries[seq % _SLOTS]
            if slot[0] == seq:  # guard against a torn wrap during snapshot
                rows.append([slot[0], slot[1], slot[2], slot[3]])
    return rows, latest


def entries_since(since_seq: int) -> tuple[list, int]:
    """Entries newer than since_seq for the /api/logs endpoint."""
    return _snapshot(since_seq)


# --- Flash persistence ----------------------------------------------------

def rotate_boot_log() -> None:
    """Rotate current -> previous. Call ONCE, first thing at boot."""
    try:
        os.mkdir(LOG_DIR)
    except OSError:
        pass  # already exists
    try:
        os.remove(PREVIOUS_LOG)
    except OSError:
        pass  # nothing to remove
    try:
        os.rename(CURRENT_LOG, PREVIOUS_LOG)
    except OSError:
        pass  # no current.log from the previous boot


def flush_to_flash() -> bool:
    """
    Rewrite the whole ring to /logs/current.log (bounded, ~25KB max).

    Core 0 only — never call from the display thread. Returns True on
    success. Failures are printed but never raised: logging must not be
    able to crash the thing it's observing.
    """
    global _last_flush_ms, _flushed_seq, _error_since_flush
    rows, latest = _snapshot(0)
    try:
        with open(CURRENT_LOG, "w") as f:
            for seq, ts, lvl, msg in rows:
                f.write(f"{ts} {_LEVEL_NAMES.get(lvl, '?')} {msg}\n")
        _last_flush_ms = time.ticks_ms()
        _flushed_seq = latest
        _error_since_flush = False
        return True
    except OSError as e:
        print(f"[LOG] flash flush failed: {e}")
        return False


def maybe_flush() -> bool:
    """
    Flush to flash if policy says it's due. Called periodically from a
    Core 0 task. Policy:
    - an ERROR was recorded since the last flush, and >= 5 min have passed
      since that flush (first error of the boot flushes immediately);
    - or there are unflushed entries and >= 1 hour since the last flush
      (heartbeat, measured from boot if never flushed).
    """
    with _lock:
        has_new = (_next_seq - 1) > _flushed_seq
    if not has_new:
        return False

    now = time.ticks_ms()
    if _error_since_flush and (
        _last_flush_ms is None
        or time.ticks_diff(now, _last_flush_ms) >= _FLUSH_MIN_INTERVAL_MS
    ):
        return flush_to_flash()

    heartbeat_ref = _last_flush_ms if _last_flush_ms is not None else _boot_ms
    if time.ticks_diff(now, heartbeat_ref) >= _HEARTBEAT_INTERVAL_MS:
        return flush_to_flash()

    return False

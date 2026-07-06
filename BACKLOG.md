# BACKLOG (ephemeral — delete me when empty)

> Working doc, not documentation. This is the **authoritative source of truth for
> what we work on next**. Add new "we should fix X someday" findings here instead
> of leaving TODOs in code. Remove items as they land; delete the file when it's
> empty.

## Firmware

2. **OTA firmware updates** — update friends' devices without manual flashing.
   Likely shape: backend hosts versioned firmware bundles (manifest + files
   with hashes); firmware checks on boot/daily, downloads changed files to a
   staging dir, verifies hashes, atomically swaps, reboots. Keep a fallback so
   a bad update can't brick (previous version retained + boot-success marker).
   Needs design: signing, partial vs full updates, interaction with
   `tools/build.py` `.mpy` output.
3. **Logging overhaul + log visibility** — audit all log messages for
   clarity/consistency (many are cryptic; "API ERROR" on the panel with no
   trail is the pain point). Likely shape: small pre-allocated RAM ring buffer
   of recent log lines + `GET /api/logs` + a simple log view in the webapp;
   richer error detail carried into `set_error` (status code, endpoint,
   exception type). Decide per-subsystem levels vs the current global level.
4. **Captive portal reliability** — DNS task hardening landed; observe. If
   still flaky: add OS-probe-specific responses (Android `/generate_204`,
   Apple `hotspot-detect.html`, Windows `connecttest.txt`) before considering
   splitting the setup portal into its own tiny page. Splitting for *page
   complexity* alone is unlikely to fix detection (detection happens
   pre-page-load).
6. **Local time / `utc_offset` use** — fetched from `/time` and currently
   unused; needed when a clock display lands (NBA).
7. **Watchdog hang detection** — the watchdog catches display-thread *crashes*
   only, not hangs; consider a frame-counter heartbeat.
8. **Per-char `get_ch` memoryview allocs** in text rendering — acceptable churn
   today; revisit if GC stutter is ever observed now that per-frame string
   building is gone.
9. **Poller `set_error` message quality** — `str(e)[:20]` is cryptic on the
   panel; map common failures to friendly text. (Overlaps with item 3.)
10. **Auto-brightness tuning** — re-tune `LUX_MIN`/`LUX_MAX`/curve now that the
    light diffuser sits over the sensor and reduces readings.
11. **Auto-brightness algorithm vs config brightness** — rethink the dual-lerp
    `apply_preference` relationship between ambient response and the user's
    brightness setting.
12. **Multiple `Button` instances per PIO block** — the skip/lock feature runs
    two on PIO1; verify program-offset reuse and deinit ordering on hardware,
    and document the pattern in `button.py` if anything surprising shows up.
13. **Button UX refinement** — replace MK1 text toasts with proper indicators
    (loading animation on skip, persistent lock icon while rotation is locked).
17. **Watch for fragmentation OOMs after manual-GC removal** — all manual
    `gc.collect()` calls are gone (status endpoint + per-request in
    api_client) so genuine memory pressure surfaces honestly. MicroPython
    auto-collects on allocation failure, so total-exhaustion OOMs self-heal;
    the residual risk is a *fragmentation* failure of a large contiguous
    allocation (most likely the TLS buffers on connection re-establishment).
    Note: the binary wire format (removed JSON dict-tree churn) and the
    aiohttp 304 keep-alive fix (TLS reconnects no longer scheduled) have
    both landed — re-baseline the memory sawtooth before judging. If
    `MemoryError` shows up: first try `gc.threshold()` (proactive amortized
    collection — one principled global dial), and only as a last resort a
    single documented collect before the TLS handshake. Also consider
    surfacing `micropython.mem_info(1)`-style fragmentation stats in the
    future /api/logs work (item 3).

## Backend

14. **Per-device API keys** — comma-separated key list in backend config →
    `HashSet` lookup; enables revoking one friend's device. ~20 lines.

## Frontend

15. **UI full audit** — alignment oddities, dead redirects, fragile UI↔API
    interactions; decide what the config-only webapp should look like
    long-term.
16. **`+page.svelte` component split** — 1400 lines → per-card components.

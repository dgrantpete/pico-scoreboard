# BACKLOG (ephemeral — delete me when empty)

> Working doc, not documentation. This is the **authoritative source of truth for
> what we work on next**. Add new "we should fix X someday" findings here instead
> of leaving TODOs in code. Remove items as they land; delete the file when it's
> empty.

> Ops note: device logs live in a RAM ring (`#/logs` in the webapp,
> `GET /api/logs`) and flush to flash on errors. When the network is dead,
> read them over USB: `mpremote cat :/logs/previous.log` (or `current.log`).
>
> Flashing: `python tools/build.py flash` reboots the device into safe mode
> first (no Core 1 thread — mpremote hangs otherwise, micropython#13476).
> Manual escape hatch when the device is wedged: hold Button A (GPIO 10)
> while power-cycling — the app skips startup and the REPL is free.

## Firmware

2. **OTA follow-ups** — the core OTA shipped 2026-07-07. All drills have
   now run on hardware (2026-07-07): end-to-end self-update, corrupt
   `/ota_staging` discarded, and a real `ota.recover()` (the 512 KB
   partition bump orphaned the old image; the device re-downloaded and
   booted unattended). `/app_version` is in `/api/status` + settings UI.
   Remaining:
   - *littlefs files are outside OTA scope by design*: `main.py`,
     `ota.py`, `config.json` only update via USB flash. Fine while rare;
     if they start churning, consider having the ROMFS image carry
     canonical copies that early-boot syncs to littlefs (with version
     guard).
   - *"Update available" indicator* in the settings UI (compare device
     sha against the backend manifest from the browser).
   - Full firmware-image OTA still blocked upstream (RP2350 A/B needs QMI
     address-translation in MicroPython; track micropython#17544).
   - *Safe-mode sentinel compat fallback*: `tools/build.py` still accepts
     the fs-probe heuristic for devices whose deployed `main.py` predates
     the `_SAFE_MODE` sentinel (2026-07-07); drop the fallback once every
     device has been flashed at least once.
4. **Captive portal reliability** — DNS task hardening landed; observe. If
   still flaky: add OS-probe-specific responses (Android `/generate_204`,
   Apple `hotspot-detect.html`, Windows `connecttest.txt`) before considering
   splitting the setup portal into its own tiny page. Splitting for *page
   complexity* alone is unlikely to fix detection (detection happens
   pre-page-load).
6. **Local time / `utc_offset` use** — fetched from `/time` and currently
   unused; needed when a clock display lands (NBA).
10. **Auto-brightness tuning** — re-tune `LUX_MIN`/`LUX_MAX`/curve now that the
    light diffuser sits over the sensor and reduces readings.
11. **Auto-brightness algorithm vs config brightness** — rethink the dual-lerp
    `apply_preference` relationship between ambient response and the user's
    brightness setting.
12. **Multiple `Button` instances per PIO block** — the skip/lock feature runs
    two on PIO1; verify program-offset reuse and deinit ordering on hardware,
    and document the pattern in `button.py` if anything surprising shows up.
13. **Persistent lock indicator** — the lock/unlock icon toasts landed
    2026-07-09 (transient, centered), but a *persistent* subtle indicator
    while rotation or league lock is engaged (small corner glyph?) is still
    open — right now nothing on screen says the board is locked after the
    toast fades.
17. **Re-baseline heap behavior after `gc.threshold(48*1024)`** — the
    threshold landed 2026-07-07 (main.py; calibrated from the measured
    ~4 KB/s churn → collect every ~12 s). Watch a game-day session: free
    memory should never grind near zero anymore, and TLS reconnects
    (~33 KB contiguous) should stop being fragmentation-exposed. If
    collections are too frequent/rare, tune toward 64 KB. History: heap
    457 KB, live set ~270-340 KB; pre-glyph-table churn was ~80 KB/s with
    GC every ~1.4 s and free bottoming at 1.9 KB.

18. **IPv6 reachability** — the device advertises an IPv6 AAAA over mDNS
    but the web server binds IPv4-only; IPv6-first clients eat a ~2 s stall
    per connection (confirmed: HTTP over IPv6 fails outright). Either bind
    dual-stack (if MicroPython's lwip supports an AF_INET6 listener) or
    suppress the AAAA. Low priority now that the socket-leak half of the
    "site unreachable" failure is fixed (microdot `connection_timeout`,
    landed 2026-07-06 — verified live: 4 pinned sockets kill inbound
    accepts, the 60 s reaper recovers without a reboot). Known limit worth
    remembering: lwip accepts only ~4 concurrent inbound sockets, so a
    burst of abandoned connections can still cause an up-to-60 s brownout.

19. **Brightness loop fixed-point math** — ~15-20 boxed floats per 200 ms
    tick (EMA, log map, ramp, dual-lerp) ≈ 2-3 KB/s of churn. Convert the
    pipeline to integer math (e.g. brightness in 0-1000, lux in milli-lux
    LUT for the log map). Small win; do alongside/after item 8. Pairs
    naturally with the re-tune in items 10/11.

21. **aiohttp: stop building the decoded header dict per response** — only
    `etag` and `content-length` are ever read; `_request` decodes/splits
    every header line into a dict (~50 small allocs per poll). Let
    `_get_header` scan the raw `_headers` bytes list instead. Minor
    (~0.5 KB/s), do opportunistically.

22. **Custom firmware: deferred levers** — the custom build itself landed
    2026-07-06 (v1.28.0 submodule + `firmware/board/PICO2W_SCOREBOARD`:
    ROMFS 256 KB, BT off ≈ +20 KB RAM, `MEMP_NUM_TCP_PCB=16`). Still on
    the table for later:
    - *mbedTLS `MBEDTLS_SSL_IN_CONTENT_LEN` 16 K → 8 K*: ~8 KB heap per
      live TLS connection, but servers can't be forced to send smaller
      records (fly.io doesn't negotiate MFL) — needs validation against
      the real backend before shipping; keep 16 K fallback.
    - *mbedTLS flash trims*: drop TLS 1.0/1.1, PSK, SECP*K1 curves
      (flash-only win, low priority).
    - *`mpy-cross -O2` for release builds*: strips asserts, keeps line
      numbers (which our log tracebacks need — never -O3).
    - *Firmware A/B OTA*: blocked upstream, see item 2.

28. **MLB poller: skip games whose detail endpoint 404s** — observed
    2026-07-07: ESPN had no summary for one scheduled game (401816055);
    the backend 404s and the poller retried it every poll interval for
    its whole rotation slot before moving on. Harmless but wasteful, and
    the display presumably shows nothing useful for that slot. Drop a
    game from rotation after N consecutive 404s (it can re-enter on the
    next list refresh).

25. **Reduce webapp HTTP connection churn** — every poll is a fresh TCP
    connection (Microdot has no server-side keep-alive), so the status
    (5 s) + logs (3 s) polls churn ~0.5 conn/s through a ~4-socket pool,
    and the browser's parallel fetches cause connect-phase drops.
    Escalation ladder: (a) *combined poll endpoint*
    (`GET /api/dashboard?since=` returning status + new log entries in
    one response) — halves connections, trivial on both sides, do first;
    (b) *WebSocket push* for logs/status (vendor microdot's websocket.py;
    firmware task pushes deltas on change; frontend reconnect logic) —
    biggest reduction, but each open WS pins one socket permanently —
    viable once the custom firmware's `MEMP_NUM_TCP_PCB=16` is flashed
    (landed in the board config 2026-07-06); the 60 s
    `connection_timeout` reaper must exempt WS (ping/pong keepalive
    instead).

26. **Event-loop latency instrumentation** — to attribute the "device did
    not respond in time" stalls to specific work: a tiny Core 0 task
    sleeps 50 ms and logs when actual wakeup overshoots by >100 ms
    (`ticks_diff`), plus max stall per minute. Correlate spikes with the
    existing request logs (TLS reconnects, flash flushes, config saves).
    ~10 lines; answers "what blocks the loop" with data instead of
    guesses. Note: TLS crypto itself is atomic C — the fix for it is
    fewer requests (item 25's combined backend endpoint also halves the
    poller's TLS round-trips per cycle), not finer yielding.

## Soccer (end-to-end wiring landed 2026-07-09; remaining polish)

Landed 2026-07-09: soccer wire encodings in `wire.rs` (clock as elapsed
seconds u16, floor-minute convention matching ESPN's displayClock) +
firmware `soccer.py` parsers cross-checked by `wire_format_check.py`;
backend serves all three states with struct negotiation (`SoccerGame::Final`
carries scores + scorer strings); `GamePoller` (`scoreboard/poller.py`)
merges MLB + configured soccer leagues into one live-first rotation with
league-namespaced logo keys and the soccer stale-clock guard; config gained
`sports.mlb.enabled` + `sports.soccer.leagues`; a second preview golden pins
the soccer live frame.

32. **Soccer live variant pick** — soccer-A ("phase ledger", default) vs
    soccer-B ("clock + phase stacked") vs soccer-C ("broadcast corners")
    are all in the preview gallery; lock the winner into
    `screen_geometry.SOCCER_LIVE_VARIANT` after a gallery review (same
    ritual as the 2026-07-07 pregame/final picks). Possible polish items
    after the pick: goal score-flash (NFL-era `should_flash` pattern),
    red-card count chips, aggregate/penalty shootout states (ESPN
    descriptions for ET/shootout not yet observed — backend warns and
    degrades to in-play; extra-time periods 3/4 already render as "ET"
    with 105/120 stoppage bases).

33. **Sports config hot-reload** — the Sports settings card landed
    2026-07-09 but league changes are reboot-required (the poller builds
    its `LeagueSource` list once at startup; the frontend raises the
    existing reboot dialog). If reboots annoy: rebuild sources inside
    `GamePoller._refresh_lists` when the config-derived key list changes
    (reset per-source caches/etags, drop a vanished league lock, rebuild
    rotation).

34. **Soccer + UX on-hardware shakedown** — first live match day with a
    real device: the extrapolated clock across 30 s polls (drift should
    re-anchor invisibly), halftime flip, goal ticker color, commentary
    flash cadence (does a line per poll feel chatty?), cross-league
    rotation with MLB on the same slate, long-press feel (800 ms
    threshold), spinner smoothness on the panel, and heap headroom with
    the per-league list polls + per-live-game summary fetches.

## Backend

14. **Per-device API keys** — comma-separated key list in backend config →
    `HashSet` lookup; enables revoking one friend's device. ~20 lines.

## Frontend

23. **Settings-page render jank** — during the 2026-07-06 audit, Chrome's
    CDP screenshots of the settings page intermittently timed out
    ("renderer frozen") and the page occasionally painted blank/partial.
    No console errors. Persisted after the cleanup (buffered sliders,
    visibility-gated polling), so likely CDP-screenshot-specific; profile
    long tasks if a real user ever reports it.

24. **Page serving stalls under concurrent load** — observed 2026-07-06
    evening (4 live games rotating + browser tab + parallel curls): the
    51 KB bundle usually serves in 1-3 s, but occasionally stalls
    mid-transfer indefinitely (34 KB then nothing). Same lwip
    constraint family as item 18 (~4 sockets, small pbuf/memory pools) —
    concurrent inbound connections can starve an in-flight send. Bounded
    now by microdot's 60 s connection reaper. If it annoys: instrument
    with a serve-duration log line, consider raising
    `Response.send_file_buffer_size` (2 KB → 8 KB, fewer awrite
    round-trips), or serve with `Connection: close` semantics sooner.

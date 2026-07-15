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
> Deploys as ROMFS by default (build.config.json `flash_release`); pass
> `--dev` for a littlefs deploy — costs ~100 KB of heap (GC thrash), only
> for quick iteration.
> Manual escape hatch when the device is wedged: hold Button A (GPIO 10)
> while power-cycling — the app skips startup and the REPL is free.

## Cross-sport consistency overhaul follow-ups (2026-07-15)

The overhaul itself landed (backend Mlb prefix + shared helpers + generic
/{sport}/{league}/games routes; firmware wire.py split, table-driven poller,
mlb_live naming + Core-0 pre-build, per-sport variant keys; soccer venue +
knockout wire change — see commits 53217ae..bc71e57). Remaining threads:

50. **mlb_live frame-health A/B** — the Core-1 pre-build shipped with
    device-REPL microbenchmarks (per-glyph 2-59 ms/frame vs strip blits
    0.3-0.7 ms; dark-color conversion 98 us + 64 B garbage per call) but
    the on-device before/after in `mlb_live` mode needs a live daytime MLB
    game. Baseline: worst=70 ms; post-ship soccer-mode windows 68-80 ms /
    slow 0-9 (GC-driven, unchanged renderers). Watch `[DISPLAY] health`
    during the next MLB slate.
51. **WNBA as the 4th sport** — 9,420-body corpus already collected and
    modeled (schema/presence/spec/discover under data/espn/generated/,
    incl. a pooled nba+wnba discover for contract comparison). The
    conventions from the overhaul are exactly what it lands on: registry
    rows in poller/SportsCard, its own module pair, variant keys.
52. **League display names are triplicated** — frontend SportsCard,
    firmware soccer.LEAGUE_NAMES, backend espn/league.rs (casing already
    drifts: "Premier League" vs "PREMIER LEAGUE"). Single source or
    codegen.
53. **Soccer `attendance`** — 100%-present in the corpus for every state;
    add to the wire only when a screen design wants it.
54. **tools/espn optimizations** (from the pipeline audit):
    - Staleness stamp: corpus fingerprint (max epoch + distinct count per
      league) written into every generated artifact; `validate`/`spec`
      warn when the DB has newer bodies. The committed artifacts were 9
      days stale when the soccer knockout gap surfaced.
    - Incremental inference: memoize per-body-hash contributions so
      schema/discover stop re-inflating the full ~10 GB corpus every run
      (discover currently makes two full passes).
    - Idle-league row bloat: ~49% of `responses` rows are off-season
      empty-scoreboard duplicates; store only transitions (or prune) and
      index `responses(source)`.

## Firmware

2. **OTA follow-ups** — the core OTA shipped 2026-07-07; the full drill
   suite re-ran 2026-07-14 ahead of gifting devices: 3× end-to-end forced
   update cycles (incl. on-demand via the new `POST /api/check-update`),
   corrupt `/ota_staging` discarded, `/ota_dev` rollback guard (both the
   endpoint and the +120s auto-check paths), and the new crash-loop
   self-heal (`/boot_fails` ≥ 5 consecutive non-power-cycle boots →
   forced `ota.recover()`; verified on console: download, apply, reset,
   counter cleared at the healthy point). Failed checks now retry hourly
   instead of daily. Remaining:
   - *littlefs files are outside OTA scope by design*: `main.py`,
     `ota.py`, `config.json` only update via USB flash. Fine while rare;
     if they start churning, consider having the ROMFS image carry
     canonical copies that early-boot syncs to littlefs (with version
     guard).
   - ~~"Check for updates" button~~ — SHIPPED 2026-07-14 (StatusCard:
     button + up-to-date/installing/updated states; dropped responses
     treated as "probably updating" with a status poll).
   - Full firmware-image OTA still blocked upstream (RP2350 A/B needs QMI
     address-translation in MicroPython; track micropython#17544).
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
    while rotation lock or the league filter (menu-applied, 2026-07-15) is
    engaged (small corner glyph?) is still open — right now nothing on
    screen says the board is locked/filtered after the toast fades or the
    menu closes.
35. **Toast-dim golden** — the icon-toast frame dim + fade ladder
    (2026-07-11) has two implementations of `display._dim_frame` (viper on
    device, pure Python in the preview) that must stay mask-identical, and
    no golden exercises a toast frame. Add a `live-toast-locked` golden to
    pin the CPython half; the viper half needs one on-hardware eyeball.

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
    (2026-07-11 note: an apparent "burst kills the listener" episode was
    misdiagnosed — the real cause was the OTA rollback reboot, item 36.
    A ~40-request status burst at 0.5 req/s was handled fine.)

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
    (reset per-source caches/etags, prune vanished keys from the league
    filter — `_build_rotation` already self-heals a fully-vanished filter —
    and rebuild rotation; MenuController shares the boot-static `sources`
    list, so a rebuild must reach it too).

34. **Soccer + UX on-hardware shakedown** — first live match day with a
    real device: the extrapolated clock across 30 s polls (drift should
    re-anchor invisibly), halftime flip, goal ticker color, commentary
    flash cadence (does a line per poll feel chatty?), cross-league
    rotation with MLB on the same slate, long-press feel (800 ms
    threshold), spinner smoothness on the panel, and heap headroom with
    the per-league list polls + per-live-game summary fetches.

36. ~~OTA rolls back unpublished local builds~~ — FIXED 2026-07-14 with
    the `/ota_dev` marker: `build.py flash --release` compares the
    deployed sha against the published manifest and writes (mismatch) or
    removes (match) `/ota_dev`; dev/littlefs deploys always write it.
    `ota.check_and_stage()` refuses to update while it exists;
    `ota.recover()` clears it (a broken dev deploy heals to the published
    app). Drilled on hardware both ways. `ota.enabled` is back to true.
    Still open (folded into item 2's littlefs note): a compat guard for
    the main.py↔ROMFS seam — don't publish images older than the fleet's
    littlefs main.py expects.

37. ~~Safe-mode request/sentinel reliability~~ — ROOT-CAUSED and FIXED
    2026-07-12: `sys.exit()` from main.py is a FORCED EXIT to the rp2
    port (`ports/rp2/main.c`: `PYEXEC_FORCED_EXIT` → soft reboot), which
    re-runs main.py — so every safe-mode entry consumed its trigger,
    "exited", soft-rebooted, and re-entered the app. Safe mode had never
    once held; the fs-probe fallback (removed same night) was what let
    flashes through historically. Both `sys.exit()` sites in main.py now
    raise a non-SystemExit exception instead (halts to the REPL with the
    `_SAFE_MODE` sentinel intact — the printed traceback is by design).
    Verified on hardware: sentinel probe returned SAFE and a full
    release flash ran through the sentinel path. Butt-covering drill for
    a rainy day: `Button A` held now parks in safe mode instead of
    soft-reset-looping (the "dark panel while held" symptom).

38. **Early-boot display splash — exonerate or fix, then re-add** — the
    `_early_display_show()` splash calls were removed from main.py's
    safe-mode/OTA paths while it was suspected of hard-faulting; the
    actual villain was item 37's soft-reset loop (each loop iteration
    tore down the panel before the splash could show, so the splash may
    be entirely innocent). If the boot splashes are still wanted: test
    `_early_display_show()` from a REPL on hardware, and only ever call
    it AFTER the safe-mode halt decision (the escape hatch must never
    depend on display bring-up).

39. **Play-flash stutter — RESOLVED, plus a characterized residual**
    (2026-07-12, measured via the temporary `[MEMPROF]` display-thread
    instrumentation and a local mock-ESPN + local-backend repro rig):
    - FIXED: play/commentary text wider than the 640 px strip pool fell
      back to the per-glyph draw path, blowing the 50 ms frame budget on
      ~half of all frames (worst 170 ms, ~10 FPS effective — and since
      scroll motion rides the frame rail, the text visibly scrolled at
      half speed). Fix: `_PLAY_POOL` grown to 2048 px (≥ the wire
      format's 255-char string cap, +5.5 KB heap) so no legal text can
      overflow it, `fit_play_text()` at commit as the belt, and the
      renderer's glyph-fallback branches deleted (strip is an
      invariant). Full text now scrolls at full speed; A/B-benchmarked
      in the mock rig with the same 172-char line: budget-blown frames
      95-104 per 10 s (glyph) → 9-15 (strip) — indistinguishable from
      short-text windows; slow frames 206-389/min → 0-31; full 20 FPS.
    - RESIDUAL (open): a metronomic ~1.1/s frame overrun (~60-75 ms —
      exactly one dropped frame per second), content-independent, and it
      SURVIVES Core 0 death (measured on a bare Core-1 render loop with
      asyncio dead). Prime suspect: cyw43/lwip ~1 Hz housekeeping vs the
      heap lock that MEMPROF's per-frame gc.mem_alloc() walk also takes
      — the profiler may amplify or partly cause it. Discriminators for
      next session: sample mem_alloc every Nth frame (over should scale
      down if the sampler collides), then MEM_PROFILE=False + a 50 ms
      `slow` threshold; also try WiFi PM mode. Barely visible; decide
      if it's worth chasing once the profiler comes back out.
    - REMOVE when done: `MEM_PROFILE` + the `[MEMPROF]` sampler in
      display.py's run_display_thread, and the mock rig notes (the mock
      lives in the session scratchpad, nothing in-repo).

## NBA (end-to-end wiring landed 2026-07-12; validation remaining)

Landed 2026-07-12, both slices: inbound (`backend/src/nba/` types +
transforms on the `status.type.state` DU, `LivePhase` from
`status.type.description`, clock as display string only — a stop-clock
can't be extrapolated; 7 live-captured fixtures + transform tests;
combined spec promoted, genuinely mlb+nba+world-cup) and outbound
(`nba/handler.rs` + `/basketball/nba/games[/{id}]` routes + OpenAPI
entries; `wire.rs` NBA encodings + goldens, cross-checked by
`wire_format_check.py`; firmware `nba.py` parser, `sports.nba.enabled`
config (default off) + poller source + play-flash commit; `nba_live`
screen (single design, soccer-A silhouette widened for 3-digit
scores, HT/END accent breaks, sub-minute warning clock) and the final
screen reused sport-agnostically (quarter columns, "T" totals,
"F/OT"); preview scenarios + an `nba-live` golden; NBA toggle in the
settings Sports card). Same change unified the outbound enum style
(all sports now MLB-style newtype variants) and moved
`Record`/`GameListEntry`/`GameState`/`parse_start_time` to
`shared/`/`espn/`. NBA logos were already served by the generic route.

41. **NBA corpus is playoff-only — validate on live games next
    season** — the April 2026 corpus never showed overtime (max
    `status.period` is 4). Types tolerate period ≥ 5 and growing line
    scores (synthetic Rust test + `nba-overtime`/`nba-final-ot`
    preview scenarios), and an unknown live description warns and
    degrades to in-play. When games return (October): re-run
    `schema`/`spec`/`validate --league nba`, regenerate fixtures,
    check whether ESPN uses an OT-specific description that
    `parse_live_phase` should map, and give the new screens the same
    on-hardware shakedown soccer got (item 34) — rotation with other
    sports, play-flash cadence, clock staleness across 30 s polls.

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

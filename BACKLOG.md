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
    rows in poller/SportsCard, its own module pair, variant keys. Plus,
    from the consistency pass (2026-07-19): prefixed wire symbols
    (`encode_<sport>_game`, `<SPORT>_FLAG_*`), `{Sport}PregameTeam`
    naming, shared enums/handlers from `shared/`, testdata nested per
    ESPN league slug, and a `_TABLES`/`_ACTIVE` registry key per screen.
52. **League display names are triplicated** — frontend SportsCard,
    firmware soccer.LEAGUE_NAMES, backend espn/league.rs (casing already
    drifts: "Premier League" vs "PREMIER LEAGUE"). Single source or
    codegen.
53. **Soccer `attendance`** — 100%-present in the corpus for every state;
    add to the wire only when a screen design wants it.
57. **spleen-5x8 accent glyphs are ASCII stand-ins** — the Latin-1 font
    support (2026-07-15) renders real accents in unscii 8/16, but the
    spleen-5x8 asset ships blank placeholder bitmaps for its whole accented
    range (upstream too), so `tools/compile_fonts.py` remaps those entries
    to the base letter's record ("Peña" -> "Pena" in spleen only). If it
    ever grates: draw the ~60 accented glyphs into a patched BDF (lowercase
    accents fit 5x8; caps are the hard part) — the build picks up real
    bitmaps automatically, no code change.
56. **1px edge-gap migration for the game screens** — owner rule (2026-07-15):
    nothing draws in row 0, row 63, col 0, or col 127 (the panel has
    unreliable edge pixels — garbage-colored LEDs surfaced at one edge).
    The league menu complies; the game/pregame/final layouts still touch
    the edges (e.g. logos at x=0, pregame INFO_TIME ends at col 127, final
    line-score bands). Inset the geometry tables + Regions when touching
    each screen next.

55. **CONDITIONAL — time-sync one-shot failure** (only act if it recurs):
    all displayed times/dates vanished once (2026-07-15) for a full uptime
    after a webapp-triggered restart, then cleared on the next reboot.
    Structural suspect: `_sync_time_from_backend` (main.py) runs once per
    boot with a 15 s timeout and NO retry — any transient (DNS settling
    right after re-association, Fly cold start) returns None and the
    poller then omits local times for the entire uptime by design. The one
    archived boot log was inconclusive (`time_sync_ok=True` visible, but
    rotation timing made boot attribution ambiguous), so cause is NOT
    confirmed. If it recurs: reboot rotates the evidence into
    `GET /api/logs/previous` — grep `[TIME]` for the failure exception
    (TimeoutError = slow backend; OSError errno = DNS race; http= = backend).
    Planned fix when confirmed: retry-with-backoff task until a sync
    lands + `poller.set_utc_offset()` so a late sync restores times
    mid-uptime.

54. **tools/espn optimizations** (from the pipeline audit):
    - Staleness stamp: corpus fingerprint (max requested_at + distinct
      count per league) written into every generated artifact;
      `validate`/`spec` warn when the DB has newer bodies. The committed
      artifacts were 9 days stale when the soccer knockout gap surfaced.
    - Incremental inference: memoize per-body-hash contributions so
      schema/discover stop re-inflating the full corpus every run
      (discover currently makes two full passes). Now reads the v2
      Postgres store on the NUC.
    - ~~Idle-league row bloat~~ SUPERSEDED by the 2026-07-19 v2 store:
      there is no `source` column anymore, and every poll row is kept
      deliberately — `coverage`'s gap accounting needs poll-cadence
      facts, and body dedup bounds the cost.

59. **Delete the dead `mlb_layout.aseprite` slices** — the MLB live migration
    moved every text-slot rectangle into `screen_geometry._MLB_LIVE`, so 16
    slices in `mlb_layout.aseprite` are now dead: `away_logo`, `home_logo`,
    `away_score`, `home_score`, `inning`, `ball_label`, `ball_values`,
    `strike_label`, `strike_values`, `out_label`, `out_values`,
    `pitcher_label`, `pitcher_name`, `batter_label`, `batter_name`,
    `play_text`. Keep `first_base`/`second_base`/`third_base` (base-marker
    art anchors). Until they're deleted in the Aseprite GUI, editing any of
    those slices silently does nothing (the firmware no longer reads them);
    `compile_layout.compile_all` then auto-removes the stale generated
    modules. GUI-trivial owner action — do NOT use the aseprite-io transpiler
    harness (known silent-drop bug vs. a 2-minute manual delete).

60. **espn `ui` viewer → Postgres (fast-follow, do next)** — `ui.py`/`ui.html`
    were kept in-tree but the `ui` subcommand was unregistered when the store
    moved to Postgres (2026-07-19; they still speak the old sqlite Store API —
    if this item slips more than a couple of weeks, delete them instead).
    Port: (a) each request opens a psycopg connection from `ESPN_DB_URL`
    (tools/espn/.env), replacing `Store(readonly=True)`; (b) dashboard keeps
    league_stats / latest_bodies_per_league / body_totals (already ported in
    db.py) and gains a collector-health panel from `collector_sessions`:
    running?, heartbeat age, current session's `targets` jsonb, recent
    sessions with end_reason; the LIVE badge switches from "db recently
    written" to "heartbeat fresh AND latest body has a state=in event";
    (c) artifacts endpoints stay filesystem-based over `data/espn/generated`;
    (d) add a coverage panel rendering `coverage.py` output (reuse the
    module, don't reimplement in SQL); (e) re-register the subcommand. The
    viewer stays a local dev tool — NOT part of the compose stack.

61. **Archive import into Postgres + extract_fixtures port** —
    `data/espn/espn.db` is frozen locally (v1 schema, free-text `source`;
    288,150 rows through 2026-07-20 00:22 UTC), and NFL/NBA capture DBs from
    the old machine sit in the desktop folder (NBA possibly already folded
    into the frozen db — verify by row counts; NFL almost certainly not).
    Import all of them into the v2 store: synthesize one `collector_sessions`
    row per archive file/source (hostname `archive`, version `import:<name>`,
    targets `[]`) so provenance stays a session FK — no `source` strings
    return; scoreboard endpoint only, `date_param` preserved; body dedup by
    hash makes re-runs idempotent. Then port `tools/extract_fixtures.py`
    (raw sqlite3 today) to the v2 store, then delete the frozen sqlite.
    Until this lands, `schema`/`discover`/`validate` see only the new corpus
    — regenerating artifacts for existing sports needs this import or fresh
    capture.

62. **Collector/mock ops odds and ends (consciously deferred)** —
    ~~bundle exports / fake-ESPN staging replay~~ SHIPPED 2026-07-19
    (`python -m tools.espn mock` / `bundle`, Fly staging pair
    `pico-scoreboard-api-staging-dgrantpete` → private
    `pico-mock-espn-dgrantpete`; see tools/espn README). Remaining follow-ups
    from that leg: re-export a genuinely replay-grade bundle once a full
    pregame→final capture lands (the baked mlb_20260719 bundle was --force'd,
    mid-game start) and swap it into infra/fly/mock.staging.yml; consider an
    HTTP scenario-control page on the mock for phone-driven demos. Still
    deferred: pg_dump backup cadence for the NUC pgdata volume; log shipping
    (docker logs suffices); binding the published 5432 to specific
    LAN+WireGuard addresses if the exposure posture changes (note: Docker's
    published ports bypass host firewall rules — relevant to homelab).
    Known-benign: at host reboot the collector's final
    session-close write can lose the race against postgres shutdown, so that
    session sweeps as `crash` with `ended_at = last_heartbeat` (≤60 s early)
    on next boot — accounting stays honest, not worth fighting dockerd's
    unordered shutdown.

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
    2026-07-16 update: plaintext polling (see item 22) removes the ~21 KB
    standing TLS buffers AND the 33 KB-contiguous reconnect spike from the
    polling path entirely — TLS allocs now happen only at boot (time sync)
    and the daily OTA check. The 48 KB threshold was calibrated around
    those spikes; re-baseline with that input gone (likely room to relax
    upward → fewer collections).

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
    - ~~mbedTLS `MBEDTLS_SSL_IN_CONTENT_LEN` 16 K → 8 K~~ — SUPERSEDED
      2026-07-16 by plaintext polling (owner decision: score polling runs
      over plain HTTP with no API key; OTA keeps TLS + key). Research
      findings preserved in case TLS polling ever returns: per-connection
      buffers are 16,717 B in + 4,429 B out on the GC HEAP (mbedtls_calloc
      → m_tracked_calloc), held for the connection's life — and the
      aiohttp fork keeps ONE polling connection alive, so this was a
      standing cost, not a spike. Fly/rustls ignores MFL (probed with
      s_client); RFC 8449 record-size-limit needs TLS 1.3 (disabled).
      Measured records from Fly: routine API ≤ ~4.5 KB (cert-chain flight
      3,602 B), but `/app/image` streams full 16 K records — so an 8 K
      buffer requires Range-chunked OTA downloads (backend 206 support +
      ota.py sequential 4 K ranges over one keep-alive conn) plus a
      board-level `MBEDTLS_USER_CONFIG_FILE` override (deferred
      target_compile_definitions on `micropy_lib_mbedtls`). Fully designed
      and viable; just no longer needed.
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
    poller's round-trips per cycle), not finer yielding.
    2026-07-16: polling moved to plain HTTP (item 22) — TLS handshake
    stalls now only possible at boot (time sync) and the daily OTA check;
    if the stalls persist after that ships, TLS is exonerated.

## Soccer (end-to-end wiring landed 2026-07-09; remaining polish)

Landed 2026-07-09: soccer wire encodings in `wire.rs` (clock as elapsed
seconds u16, floor-minute convention matching ESPN's displayClock) +
firmware `soccer.py` parsers cross-checked against the Rust golden bytes;
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
entries; NBA wire encodings + goldens, cross-checked against the
firmware parser; firmware `nba.py` parser, `sports.nba.enabled`
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

## Football (end-to-end wiring landed 2026-07-18; live validation remaining)

Landed 2026-07-18 as a full sibling sport (NFL + NCAAF, soccer-style
multi-league): backend `football/` module + `FootballLeague` registry rows +
wire encodings with goldens; firmware `football.py` parser (cross-pinned to
the Rust bytes), `football_live` screen (broadcast
corners over the sprite field strip — endzones palette-tinted to team
colors, Core-0-projected scrimmage/first-down perspective lines, ball at
the LOS, timeout bars, possession arrow, red-zone warning colors; first
game screen born edge-rule-compliant, item 56 unaffected), shared
pregame/final reuse (NCAAF rank line rides the pitcher slot, venue rides
the weather slot), poller/config/menu/frontend registry rows, preview
scenarios + two goldens. `football_layout.aseprite` was generated from the
archived legacy art via the aseprite-io harness (repos/aseprite-io-feasibility,
`examples/gen_football_layout.rs`); the archive zip is deleted — the
.aseprite is now the only source of truth.

58. **Football corpus is empty until preseason — validate on live games
    (Aug 2026)** — no live NFL/NCAAF bodies exist off-season, so the
    transforms ship against synthetic fixtures modeled on the excavated
    pre-rewrite ESPN shapes. When preseason starts (~Aug 7): confirm the
    `yardLine` possession-relative semantics (THE highest-risk assumption —
    the abs-ball mirror math in `set_football_live` is excavated from
    working code but never validated live; flagged in the backend
    `validate_situation` doc comment), displayClock string forms, live
    status descriptions ("End of Period" vs "End of Quarter" alias),
    curatedRank shape, timeouts presence timing, and last-play id/text
    cadence. Capture real fixtures (tools/extract_fixtures.py), swap the
    synthetics in backend/testdata/football/, re-run tools/espn
    schema/spec/validate for nfl+ncaaf, and give the screens the same
    on-hardware shakedown soccer/NBA got (items 34/41): rotation with
    other sports, field/ball render on the physical panel, play-flash
    cadence. NCAAF deliberately polls ESPN's default Top-25 slate
    (doc-commented in the handler) — revisit only if the rotation wants
    more games.

## Rust firmware rewrite (firmware-rs)

63. **`hub75`'s RGB565→bitplane pack is 76 % of a drawn frame** — measured on
    silicon by the Phase 3 app shell's frame probe (2026-08-08,
    `firmware-rs/BUDGET.md` "Core 1: measured frame times"): `load_rgb565` +
    `flip` costs a flat **5.25 ms** regardless of content, against 0.48-1.96 ms
    for the entire render path. Total worst frame is 7.4 ms of a 50 ms budget,
    so **there is nothing to fix today** — logged because if frame time ever
    has to come down, this is where it is, and the intuition that "drawing is
    the expensive part" is wrong by 3×. ~96 cycles/pixel at 150 MHz for eight
    bitplanes plus a gamma lookup; the obvious levers are a wider inner loop
    and doing the pack in the same pass as the gamma LUT. Note the measurement
    is XIP-placement sensitive (5.07 vs 5.25 ms across two builds differing by
    120 B), so benchmark any change against a rebuild of its own baseline.

    **Duty updated by task #17 (2026-08-08), verdict unchanged.** 60 FPS does
    not make the pack slower — it makes it run three times as often. Per drawn
    frame it is still a flat 5.25 ms; as a share of core 1's wall time on a
    screen that draws every frame it went from ~10 % to **~31 %**, and the worst
    total frame from 15 % of budget to **44 %** (7.41 ms of 16.67). There is
    still nothing to fix: 9.26 ms of margin is not tight. This stays the first
    place to look if headroom is ever needed, and it is now the *only* place
    worth looking — the entire render path is under 2 ms.

    **Implemented, pending on-device measurement (task #20, 2026-08-09).**
    Tier 1: the pack loop is RAM-resident (`.data.hub75_pack`, ~0.6 KiB in the
    app image), which also retires this item's XIP-variance warning. Tier 2:
    fused gamma+bitspread tables — one u64 per raw 5/6-bit channel carrying
    all eight positioned plane bits, three tables plus a shift for the bottom
    lanes, 1 KiB, derived inside the driver so `set_gamma` cannot forget to
    rebuild them. The old pack survives verbatim as
    `crates/hub75/tests/reference/mod.rs`, with property tests pinning
    byte-identical output over all 65,536 RGB565 values and five gamma tables.
    Tier 3 (wide stores) was measured and *declined*: 1.62× on the host, where
    stores dominate, but the u32 transpose keeps four u64s live and regresses
    thumbv8m codegen to 94.5 instr/pixel-pair against the shipped 78 — the
    decision table lives in `crates/hub75/benches/pack.rs`. Projection from
    the instruction ratio (~165 → 78 instr/pair, calibrated on the measured
    192 cycles/pair): **~2.1–2.6 ms** against the ≤2 ms finish line, close
    enough that the frame probe decides. If it lands above the line, the next
    lever is tier-4 dirty-region packing, deliberately still in this item.

64. **`hub75-diag` still links without flip-link** — the app got the
    stack-overflow guard in Phase 3 (`firmware-rs/app/.cargo/config.toml`, two
    lines, plus `cargo install flip-link` which CI now does). The bench binary
    was left alone on purpose: one shallow task, 429.9 KiB of slack, and it is
    another task's working tree. Copy the two lines and re-measure its BUDGET
    breakdown whenever that tree is quiet.

65. **Report the `cyw43` scan bug upstream** — found on the bench 2026-08-08.
    `ScanOptions::nprobes` is an `Option<u16>`; `Control::scan` turns `None`
    into `!0u16` and widens it into `ScanParams::nprobes`, which is a `u32`
    standing in for the firmware's `int32` field whose "use the default"
    sentinel is `-1`. The chip therefore receives `nprobes = 65535`, rejects
    it, and ends the scan in about a millisecond having found nothing — which
    is indistinguishable from "there are no networks here", so the failure
    reads as an empty neighbourhood rather than as a bug. Reproduced on cyw43
    0.7.0 / Pico 2 W: `None` finds 0 every time, `Some(2)` finds 36 in 710 ms.
    The firmware works around it in `net::wifi::scan` by always passing
    `Some(2)`. The upstream fix is one line — either widen the sentinel or make
    the field `Option<u32>` — and it is worth filing because every embassy user
    who takes `ScanOptions::default()` has a scan that silently returns
    nothing.

66. **Advertise the captive portal with DHCP option 114 (RFC 8910)** — the
    AP-mode DHCP server deliberately leaves `captive_url` unset. Modern clients
    (iOS 14+, Android 11+, Windows) will follow that option straight to a setup
    page instead of guessing from probe results, which is strictly better than
    the DNS-lie-plus-redirect dance. It is not sent today because RFC 8910's
    pointer is supposed to lead to an RFC 8908 JSON API, and a client that
    follows it and finds HTML can end up worse off than one that falls back to
    probing. Do this with task #10, which owns the HTTP surface: serve
    `/api/captive-portal` returning `{"captive": true, "user-portal-url": …}`
    and set `options.captive_url` to it in `net::dhcp_server`.
    **Not done in task #10** (2026-08-08): it is an improvement on the
    MicroPython behaviour rather than parity with it, and the parity release
    should not ship a captive-portal mechanism the old firmware never had and
    the soak has never exercised. The HTTP half is now a ten-line route.

67. **Bench-validate the AP-mode captive redirect from a phone** — the
    station-mode half of the `Host` check is validated on hardware (foreign
    `Host` → 404, which is also the MicroPython bug fixed), but the setup-mode
    `302` to `http://<ap ip>/#/setup` has only host tests behind it: firing it
    needs the test client associated to the device's own AP, which takes the
    developer's machine off its network. Do it once during Phase 3's soak with
    a phone — join the setup SSID, confirm the OS opens the setup page by
    itself. That is the client the redirect exists for and the only one whose
    behaviour matters.

68. **The gamma LUT rebuild runs inside a core-1 frame (27.5 ms)** — a
    `PUT /api/config` that changes gamma costs 256 `libm::pow` calls on core 1,
    measured at 27,562 µs, which is over half a 50 ms frame. It fits today (no
    overrun recorded, 20.0 FPS held) and it happens only on an explicit config
    save, so it is not urgent. If a frame ever gets slower, the fix is to build
    the LUT on core 0 and send the finished 256-byte table instead of the
    `Gamma` value — the driver already stores the LUT, so it is a change of
    what crosses the seam, not of the driver.

    **CLOSED by task #17, 2026-08-08**, and the frame did get shorter: 60 FPS
    makes the budget 16.7 ms, so 27.6 ms went from "over half a frame" to a
    guaranteed overrun on every gamma save. Fixed exactly as written above —
    `hub75::gamma::GammaTable` is a mode plus its finished 256 bytes,
    `DisplayUpdate` carries one, and `Hub75Driver::set_gamma` takes one. There
    is deliberately **no** entry point left that hands the driver a bare `Gamma`
    to expand, so the cost cannot come back by accident; the message is 256
    bytes wider and core 1's share is a `copy_from_slice`.

69. **A station that loses its association never comes back** — observed
    2026-08-08 on the Rust bench unit during task #10: after roughly half an
    hour of idle uptime the device stopped answering entirely — no HTTP, and
    **no ARP reply**, which is what makes it a link/stack failure rather than
    an application one (two wedged HTTP sockets would still leave the device
    pingable). Meanwhile core 1 was rendering at a steady 20.0 FPS with no
    errors, and `net::watch_link` logged nothing at all in a 75 s window: no
    `wait_config_down` transition fired, so embassy-net still believed its
    IPv4 configuration was up while the radio was off the network. A
    `probe-rs reset` had it serving again in 6 s.

    This is the documented consequence of `net::watch_link` deliberately not
    being a reconnect loop (`main.py` has none either, and task #9 recorded the
    choice), so it is not a regression — but the MicroPython firmware's answer
    was the watchdog, and that is task #12's. **It blocks the one-week soak in
    task #13**: a unit that silently falls off the network and keeps drawing is
    exactly what a soak is supposed to catch, and there is nothing to catch it
    with yet. Two things are needed, and the second is not optional: the
    watchdog feeder, and a liveness signal that notices *this* failure — a
    frame counter will not, because core 1 is perfectly healthy throughout.
    Candidates: fail the health gate when `stack.config_v4()` has been `None`
    past a threshold, or when the poller has had no successful fetch in N
    intervals. Worth reproducing first with a long `probe-rs attach` capture to
    find out whether cyw43 reports the disassociation at all — if it does not,
    that is an upstream bug worth filing alongside item 65.

    **The second candidate now exists** (task #11): `poller::health()` returns
    the consecutive-failure streak and seconds since the last successful poll,
    and `supervise::liveness` already logs both every 10 s, so the number is
    visible before anything depends on it. `Health`'s docs carry the gate #12
    should use and the argument for it — `since_success_s > 3 × poll_interval`
    **or** `streak >= MAX_FAILURES`, not `streak > 0`, because one failed poll
    is a backend restart and rebooting over it is worse than a stale score.
    Both halves are needed: the streak alone cannot tell a poller that is
    failing from one that has stopped ticking, and a task that has stopped is
    what a watchdog is for.

    **CLOSED by task #12, 2026-08-08.** `supervise::watchdog` feeds the hardware
    watchdog only while `FRAME_SEQ` advances *and* that gate passes, and starves
    it deliberately otherwise. Drilled on the bench by pointing `api.url` at a
    closed port: starvation at 91 s of uptime, hardware reset, and the next boot
    served the reason at `/api/logs/previous`. Two things came out of building
    it that are worth carrying forward:

    - **A watchdog reset used to be indistinguishable from a power cut.** The
      ring log is RAM. So the feeder writes a breadcrumb to a `.uninit` RAM cell
      *before* it stops feeding, and the next boot promotes it to flash — the
      cell survives a watchdog reset, which the drill demonstrates.
    - **The watchdog is still opt-in and still defaults off**, which bounds the
      one pathology below. Task #13's soak has to turn it on, or the soak is not
      testing the thing that unblocks it.

70. **The health gate cannot tell a dead network from a dead backend** — the
    known cost of closing item 69, worth fixing before the watchdog is ever
    defaulted on. With `watchdog.enabled`, a backend outage longer than
    `3 × poll_interval` (90 s by default) resets the device, and keeps resetting
    it every ~100 s until the backend returns. MicroPython showed the error
    screen and sat there. The device recovers on its own either way, so this is
    a nuisance rather than a hazard, and it only affects units whose owner
    deliberately enabled the watchdog.

    The fix is to gate on *reachability* rather than on poll success: record a
    timestamp whenever the backend answers at the HTTP layer **at all**,
    including a 500, because an answer proves the link works — and let only that
    clock feed the "backend unreachable" half. A poll that fails to decode, or
    fails with an HTTP status, would then keep the device alive; only a
    transport failure (DNS, connect, timeout) would eventually starve it. That
    is a small change in `poller.rs` where errors are recorded, plus one more
    atomic in `Health`. Left undone because the gate as specified in `Health`'s
    docs is the one that was reviewed and agreed, and changing it quietly while
    implementing it would have been the wrong way round.

    **CLOSED 2026-08-08, approved after the failure recurred live.** The gate
    now keys on `Health::since_answer_s` — seconds since anything answered at
    the HTTP layer — and the failure streak is no longer an input to it. The
    streak still counts and still raises the error screen at `MAX_FAILURES`; it
    just never starves a watchdog.

    **"Answer" means the HTTP layer, not TCP, and that was the one real design
    question.** A refused connection is in principle equally good evidence of a
    live link. It is not usable evidence here, for two reasons found by
    measurement rather than argument: `api_client`'s own comment from task #11
    records that embassy-net answers `ConnectionReset` for a refused connect and
    an exhausted socket pool alike, so `Transport::Connect` already cannot tell
    them apart — and the task #12 drill that pointed `api.url` at a *closed
    port* produced `Timeout`, not a connect error at all. A gate keyed on
    "refused" would therefore have starved in precisely the case it was written
    to exempt. Against the deployed backend the distinction is nearly vacuous
    anyway, because a dead app behind Fly's edge answers 502. The residual — an
    `api.url` typed to a reachable-but-refusing address reads as link death — is
    a misconfiguration, is visible on the error screen, and the watchdog is
    opt-in.

    Drilled in both directions with the streak deliberately on the wrong side
    each time: 404s from the real backend reached a streak of **19**, nearly 4×
    `MAX_FAILURES`, and were fed throughout with no reset; an unroutable address
    starved at a streak of **2**, well below it. Transcripts in PARITY.md.

72. **`probe-rs run` reports "Exception" on a device that is running fine** —
    a bench-workflow trap that cost an hour during BACKLOG 70 and will cost the
    next person the same unless it is written down. Symptoms: `probe-rs run`
    prints `Firmware exited unexpectedly: Exception`, then
    `UNWIND: Tried to unwind RegisterRule at CFA = None`, then a backtrace
    naming plausible-but-unrelated code (a football renderer frame and a
    reqwless `unreachable!` in one case, both innocent). Killing a backgrounded
    `probe-rs` mid-operation seems to make it more likely, and it leaves the
    core halted afterwards, so the device really is dead *until the next flash*
    — which is what makes the false positive convincing.

    **The oracle is HTTP, not probe-rs.** `curl /api/status` answered within ten
    seconds on a build that `probe-rs run` had just declared crashed, twice. Two
    corroborating checks that cost nothing: the boot line
    `supervise: stored record: …` will still name the *old* breadcrumb if no new
    panic was recorded, and a genuine panic always leaves one — so
    "probe-rs says Exception but the breadcrumb has not changed" means the
    firmware did not panic. Prefer `probe-rs attach` for observation and reserve
    `probe-rs run` for flashing; never background it and never `TaskStop` it.


73. ~~**Better auto-brightness algorithm**~~ — owner request, 2026-08-08.
    **CLOSED by task #19, 2026-08-09**, design approved by the owner after the
    old-curve hardware sweep was banked as the parity baseline. The gate held:
    nothing landed until there was a recorded "before".

    What shipped is a new pipeline, not a tuning pass —
    `crates/scoreboard-input/src/brightness.rs`, written up in PARITY.md's
    *Post-parity divergences*. Lux → EMA → log curve to a **perceptual** `B` in
    [0,1] → additive clamped bias `B + (pref−50)/50` → asymmetric ramp on `B`
    (1.5 s up, 8 s down) → `duty = 0.05 + 0.95·B³`. Three of this item's five
    avenues are in it: the perceptual mapping (a cube, from CIE L\* and
    Stevens, rather than CIE's own piecewise form — the knee is below the duty
    floor, so it would be arithmetic nobody could see), the asymmetric
    response, and, indirectly, the hunting complaint: the ramp asymmetry plus
    the EMA leave nothing fast enough to hunt, so no deadband was added and no
    hysteresis state exists to get stuck in.

    The real reason it was worth doing was none of the five, and only became
    obvious once the numbers were on the bench: **the dual lerp's knob strength
    depended on the room.** Preference 25 sat 0.475 below auto at 300 lux and
    0.143 below it at 9 lux — the same slider, a 3.3× different amount of
    change — so a setting found in a lit room quietly lost two thirds of its
    authority after dark. An additive bias in perceptual space is the same step
    everywhere it does not clamp, and that property is host-tested.

    **Still deliberately absent, and no longer worth an item:** auto-ranging
    integration time and the Vishay high-lux correction polynomial. Both only
    matter above ~1,900 lux, where the sensor saturates, and the curve has been
    flat since 300 lux — they would change a number nothing reads. The
    VEML7700 gain table stays pinned including its two wrong-vs-datasheet
    cells; the driver is untouched by this change.

    Bench work outstanding (task #19, after the owner flashes): re-run the
    thumb/flashlight sweep and a 0/25/50/75/100 preference ladder against the
    banked baseline.

74. **Config storage write-granularity (owner asked: "minimize flash wear —
    can we drop JSON?")** — analysis 2026-08-08; the wear half is already
    solved *by construction* and the item exists so nobody re-solves it.
    The store is `sequential-storage`, embedded Rust's append-only flash map
    — the thing the owner half-remembered (its MicroPython cousin is
    littlefs's dynamic wear leveling). A save never rewrites in place: it
    appends to the current 4 KB page; a page is erased only when the 980 KB
    / 245-page region wraps. The math: a 942 B JSON document = ~4 saves per
    page ≈ 980 saves per full-region cycle; at the flash's 100 K
    erase-cycle rating that is ~10⁸ saves, i.e. **~27,000 years at ten
    config changes a day**. JSON's cost is bytes-per-append, not rewrites —
    and it buys the schema-evolution story (serde defaults *are*
    `config.py`'s deep merge; task #12's decision), where a packed format
    (postcard) would make every added field a versioned migration to gain a
    ~4× improvement on a meter that reads "geological". The genuinely
    interesting refinement if write volume ever grows (e.g. sticky prefs
    written per button press): **per-field keys** — the map is built for
    many small records, so a brightness tweak would append ~16 B instead of
    942 B (~60×). Trade-offs to work through at that point: boot assembles
    the config from N keys, `reset-network` becomes key deletion rather
    than document rewrite, and the one-flash-write-per-PUT-batch property
    needs restating per key.

85. **The firmware does not answer ICMP echo** — found 2026-08-09 while
    verifying mDNS: `ping scoreboard.local` resolves the name (mDNS working)
    but the echo times out; MicroPython's lwIP replied. Name resolution and
    HTTP are unaffected, so this is cosmetic — but ping is exactly the "is it
    up?" check a guest tries first, and a timeout reads as "down". smoltcp
    can answer echo; check what embassy-net needs (an ICMP socket or a
    feature) and wire it. One evening, low priority.

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


---

## Phase 4 (OTA integration) — what it left open

75. **The DFU hash has never been timed on hardware, and one design turns on
    it.** `scoreboard_ota::verify` does not use embassy-boot's
    `verify_and_mark_updated` because that call hashes through a hardcoded
    two-byte buffer inside a single blocking call, and the bootloader's 8 s
    watchdog cannot be disarmed — an overrun is a reboot loop on every update.
    The app hashes with a 4 KB buffer instead and logs
    `ota: hashed N bytes of DFU in M ms` at ERROR. **Drill day reads M**
    (`firmware-rs/DRILL.md` step 1). If it is well under 8,000 ms the design
    has margin and the note stands as recorded; if it is close, the boot
    sequence needs a second look, because 4 KB is already ~30× fewer flash
    reads than the path that was rejected.

76. **Swap time at a real image size is a budget, not a measurement.** The
    spike measured ~5 s with *mostly-erased* 1.5 MB partitions and said time
    scales with content. The shipping image is 1.06 MB of programmed flash, so
    BUDGET.md carries 35–70 s of dark panel as an estimate. Drill day replaces
    it with a number.

77. **The 600 s confirm deadline has only ever been reasoned about.** The
    health gate confirms a trial image on the weaker evidence — boots, renders,
    provisioned — if no backend answer has arrived in ten minutes, because an
    unconfirmed image is *armed*: it reverts at the next unrelated power cut,
    days later, and the owner sees an unexplained downgrade. Nothing has ever
    exercised the path. `DRILL.md` step 6.

78. **`/fw/*` sends its API key in cleartext.** Unavoidable given SPEC §8's
    removal of device TLS, and mitigated by `APP_FW_API_KEY` being a *different*
    key from the MicroPython fleet's — so the exposure cannot reach `/app/*`,
    and what it buys is a download that is worthless without the signing key.
    Worth revisiting only if the artifacts ever stop being freely downloadable
    in principle, which they are today.

79. **No NSEC for AAAA.** `scoreboard_portal::mdns` answers A and stays silent
    on AAAA rather than sending a negative response saying "I own this name and
    have no IPv6". Correct behaviour is silence-then-retry from the resolver;
    a strictly-conforming responder would save a fraction of a second on the
    first lookup. Related to item 18.

80. **No known-answer suppression in the mDNS responder.** A query may carry
    records the asker already holds, and RFC 6762 §7.1 says a responder should
    stay quiet if its copy is not materially fresher. Skipped: the cost is one
    redundant 66-byte datagram per query for one record, against parsing the
    answer section of every query on a busy multicast group.

81. **The signing key exists in exactly one place.** `backend/.fw-signing-key`,
    gitignored, on one machine. Losing it means every deployed unit needs a
    physical flash before it can ever be updated again — the public half is
    compiled into each image. It needs a backup somewhere outside this
    repository, and rotating it later is a two-release dance
    (`app/src/ota/key.rs` documents the order).

82. **`publish-fw` does not gate on a clean tree.** It warns and stamps
    `-dirty` into the version, which is visible in `/api/status` and in every
    request log line, but it will still publish. Deliberate — a bench cycle
    against the staging channel should not need a commit — and worth
    reconsidering if `--channel stable` ever runs from anything but CI.

83. **A finer toast dim ladder is now affordable** — deferred by task #17,
    2026-08-08, after the 60 FPS change turned out *not* to need it. The fade
    steps every 50 ms off the wall clock, not once per frame, so it was never
    frame-coupled: 60 FPS shows the same four rungs (7/8, 3/4, 5/8, 1/2) for
    three frames each instead of one, over the same 150 ms in and 150 ms out.
    `tests/screens.rs::the_toast_fade_takes_the_same_time_at_any_frame_rate`
    pins that, and `toast::FADE_STEP_MS`'s docs say why the coincidence looks
    like coupling.

    What *is* newly available is resolution: 150 ms is now nine frames rather
    than three, so a finer ladder would actually be seen. The ceiling is the
    dim's arithmetic, not the frame rate. `Canvas::dim` is a masked shift-add —
    `w>>1` plus optional `w>>2` and `w>>3` — which spans 1/2 to 7/8 in eighths.
    Adding a `w>>4` term doubles that to eight rungs in sixteenths (the mask is
    `0x0861`, by the same per-field derivation as the existing three); a `w>>5`
    term is where it stops, because at 1/32 the mask is `0x0020` and only the
    6-bit green channel survives, so the frame would tint as it faded.

    Eight rungs over eight frames is 133 ms against today's 150, which is the
    natural landing spot. **The cost is a deliberate pixel divergence from the
    MicroPython baseline**: `toast_lock` and `toast_spinner` are in the parity
    corpus at `t0`, where rung 0 would go from 7/8 to 15/16, and the goldens
    come from a firmware that will never grow the rung. So this needs an
    accepted-divergence class in `parity_frames.rs` and a PARITY.md verdict
    entry, which is why it is not a drive-by.

84. **60 FPS narrowed the flip/load tearing window, and nobody has looked at
    the panel while it was narrow** — found by task #17's audit, 2026-08-08,
    computed rather than observed. `Hub75Driver::flip` swaps the framebuffer
    pointer, but the data-control DMA keeps scanning the old buffer until the
    next panel frame boundary, and the old buffer is what the next
    `load_rgb565` writes into. Safe while the gap between a flip and the next
    load exceeds one refresh period: at 120 Hz that is 8.3 ms against 48 ms of
    gap at 20 FPS and **15 ms at 60 FPS**. Still clear by 6.7 ms, and the
    margin was 40 ms.

    Two things follow. First, at 60 FPS the condition fails if the configured
    `target_refresh_rate` drops below roughly 66 Hz — which is a value the
    config accepts and the settings form offers, where at 20 FPS it would have
    taken about 20 Hz to get there. Second, the whole thing is arithmetic from
    a comment: nobody has put a camera on the panel at 60 Hz refresh and 60 FPS
    render to see whether the tear is visible or whether the BCM scan makes it
    moot. Do that at drill day — it is one config PUT and a look — and if it
    tears, the fix is a refresh-rate floor in `scoreboard-config` rather than
    anything in the driver.

86. **The games list carries no team abbreviations, which costs the crest
    warmer one detail fetch per game** — found by task #18, 2026-08-09. A crest
    is keyed by `{league key}/{abbreviation}`, and `scoreboard-wire`'s list
    format is `u8 version`, `u8 count`, then per game `u8 state` + a
    length-prefixed id. So the rotation cannot say which crests a game it has
    not shown will need, and the idle warmer has to fetch that game's *detail*
    to find out before it can warm anything.

    The cost is bounded and paid once — `prefetch::WarmIndex` remembers the
    abbreviations, and the poll loop's own commits fill it for free, so the
    probe is skipped entirely for games the board has already displayed — but
    it is one request per game per slate epoch that a richer list would not
    need, and it halves how fast a cold board converges.

    The fix is a wire v3 whose list entries carry both abbreviations: ~10 extra
    bytes per game, backend `list::encode` and the firmware decoder in the same
    crate (they are defined together so they cannot drift), and the warmer's
    probe deletes outright along with roughly half of `prefetch`. **It is
    deliberately not done here**: `WIRE_VERSION = 2` is documented as frozen
    because a device in the field decodes it, so bumping it is a coordinated
    backend-and-firmware release, not a refactor. Worth doing the next time the
    format opens for another reason — and if it does, note that `Slate` would
    then want the abbreviations on `SlateEntry`, which is ~3.2 KB across
    `MAX_SLATE`, against the ~744 B the warmer's index spends today (computed
    from the layout, not isolated in the symbol table — it is inside the poller
    arena). The index is the cheaper home unless the list already carries them.

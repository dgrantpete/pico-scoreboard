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

2. **OTA firmware updates** — update friends' devices without manual flashing.
   Likely shape: backend hosts versioned firmware bundles (manifest + files
   with hashes); firmware checks on boot/daily, downloads changed files to a
   staging dir, verifies hashes, atomically swaps, reboots. Keep a fallback so
   a bad update can't brick (previous version retained + boot-success marker).
   Needs design: signing, partial vs full updates, interaction with
   `tools/build.py` `.mpy` output.
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
13. **Button UX refinement** — replace MK1 text toasts with proper indicators
    (loading animation on skip, persistent lock icon while rotation is locked).
17. **Watch for fragmentation OOMs after manual-GC removal** — all manual
    `gc.collect()` calls are gone (status endpoint + per-request in
    api_client) so genuine memory pressure surfaces honestly. MicroPython
    auto-collects on allocation failure, so total-exhaustion OOMs self-heal;
    the residual risk is a *fragmentation* failure of a large contiguous
    allocation (most likely the TLS buffers on connection re-establishment).
    **Measured 2026-07-06**: heap 457 KB total, live set ~340 KB, free
    bottoms out at **1.9 KB** just before each auto-collect (every ~1.4 s
    during at-bats at ~80 KB/s churn) — the heap spends much of its life
    near-full, which is exactly the exposure a TLS reconnect (~33 KB
    contiguous) doesn't want. After item 8 lands and churn drops ~10×:
    set `gc.threshold()` (collect after a fixed allocation amount, e.g.
    ~48 KB) so free memory never grinds against zero — one principled
    global dial, cheap once churn is low. Re-baseline afterward.
    **Post-fix numbers (2026-07-06 afternoon, glyph tables landed)**:
    intrinsic churn ~4 KB/s (was ~80), GC every ~28 s (was ~1.4 s) — a
    threshold around 48-64 KB would collect every ~12-16 s and keep free
    memory from ever grinding near zero. Pair with item 20's buffer shrink.

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

22. **Frozen modules (custom MicroPython build)** — the biggest live-set
    lever: module bytecode (~100-150 KB of the ~340 KB live set) moves to
    flash via a custom firmware image with the app frozen in. Big workflow
    change (reflash = full firmware image; interacts with OTA item 2 —
    frozen code can't be OTA-updated file-by-file). Only worth it if
    headroom gets tight after items 8/17/20.

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

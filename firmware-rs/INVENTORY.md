# MicroPython Firmware Behavior Inventory

Compiled 2026-08-07 from firmware/src at commit ae14e35, as the parity reference
for the Rust rewrite (see SPEC.md). Appendix B's checklist derives from this.
File:line references are to the MicroPython tree at that commit.


**Scope:** 15,697 lines of first-party Python across 78 files. `firmware/micropython/` is a vendored upstream MicroPython submodule — ignore it. `firmware/board/PICO2W_SCOREBOARD/` is an out-of-tree board definition (ROMFS enabled, Bluetooth off, lwIP TCP pools raised); `firmware/dist/*.uf2` are build outputs.

**Target hardware:** Pi Pico 2 W (RP2350, `armv7emsp`), 128×64 HUB75 panel, VEML7700 lux sensor, 2 buttons, (rotary encoder wired but unused in production).

---

## 0. Generated vs. hand-written (CLAUDE.md claim verified against .gitignore + generators)

| Path | Status | Generator |
|---|---|---|
| `scoreboard/layout/**` (29 modules) | **Generated**, gitignored, **no `__init__.py` at all** (namespace package) | `tools/compile_layout.py` |
| `scoreboard/fonts/*.py` (`unscii_8`, `unscii_16`, `spleen_5x8`) | **Generated**, gitignored | `tools/compile_fonts.py` |
| `scoreboard/fonts/__init__.py` (515 lines) | **Hand-written, tracked** — the `FontWriter` API | — |
| `index.html.gz` | Generated (`bun run build` in `frontend/`) | `tools/build.py` |
| `config.json` | Gitignored local secrets (real WiFi creds + API key live here on-device) | hand-created |
| `lib/hub75/native/*.mpy`, `lib/hub75/effects/*.mpy`, `lib/miqro/**.mpy` | Precompiled **native ARM** `.mpy`, no source in repo | external |
| everything else | Hand-written, tracked | — |

`git ls-files firmware/src/scoreboard/fonts/` returns exactly one file (`__init__.py`); `git ls-files firmware/src/scoreboard/layout/` returns nothing.

Tracked source-of-truth assets: `firmware/assets/fonts/{spleen-5x8.bdf, unscii_8.pcf, unscii_16.pcf}`, `firmware/assets/layout/{football_layout.aseprite, mlb_layout.aseprite, toast_lock_closed.png, toast_lock_open.png, toast_spinner.png}`.

### Generated font module shape (Rust consumer contract)
Exposes `HEIGHT: int`, `GLYPHS: tuple` (224 entries indexed `codepoint - 32`, covering 32..255), `DEFAULT` (the `'?'` glyph). Each entry is a ready-to-blit 4-tuple `(memoryview, width, HEIGHT, framebuf.MONO_HLSB)`. Backing data is two `bytes` blobs: a record heap (`u16 LE width`, then `ceil(w/8) * HEIGHT` bytes of MONO_HLSB rows, **MSB = leftmost pixel**) and a 225-entry `u16 LE` offset index where slot 0 is DEFAULT and `0xFFFF` means absent. Height uniform per font, width per glyph. Blank Latin-1 glyphs in spleen are remapped at compile time to ASCII stand-in records (`é`→`e`, `ñ`→`n`). In Rust: `&'static [u8]` heap + `&'static [u16]` index + `struct Glyph { bits, width }`, `row_stride = (width + 7) / 8`.

### Generated layout module shape
Three shapes by source kind:
- **Slice** → coordinates only: `X, Y, WIDTH, HEIGHT`. 18 of 29 modules.
- **`__relative` layer / plain PNG** → sprite only: `KEY, WIDTH, HEIGHT, data (FrameBuffer), palette (RGB565 FrameBuffer)`.
- **`__absolute` layer** → both (sprite plus authoritative `X`/`Y`).

`KEY` is `0xF81F` (63519) whenever the sprite has transparency, `-1` otherwise — **the same value for paletted and RGB565 sprites, because MicroPython's `framebuf.blit()` applies the palette lookup BEFORE the key comparison.** That is the load-bearing detail for a Rust blitter. Format is auto-selected to minimize `pixel_bytes + palette_bytes` across MONO_HLSB / GS2_HMSB / GS4_HMSB / GS8 / RGB565, and the bit-packing conventions disagree between MicroPython formats and must be matched exactly: MONO_HLSB MSB = leftmost; **GS2_HMSB puts the leftmost pixel in the LOW 2 bits** (`shift = (x%4)*2`); GS4_HMSB puts the leftmost pixel in the HIGH nibble; GS8 = 1 B/px; RGB565 = LE.

---

## 1. Top level

### `main.py` — 1,258 lines

Boot sequence, strict order:

**1. Safe-mode escape hatch (`main.py:47-132`) — MUST stay first.** Triggers: Button A (GPIO 10, active-low) held at power-up, or a `/update` flag file written by `tools/build.py flash` and consumed here. Rationale (`:34-45`): once the Core 1 display thread starts, `mpremote` filesystem commands hang (micropython#13476) and soft reset can wedge TinyUSB via a spinlock held by the dying core (micropython#8494) — flashing a *running* scoreboard is inherently unreliable.

Critical at `:123-132`: sets `_SAFE_MODE = True` (positive proof for `build.py`, which shares the namespace via `mpremote exec`) then **raises `RuntimeError` rather than `sys.exit()`** — `SystemExit` from `main.py` is a forced exit to the rp2 port that *soft-reboots and re-runs `main.py`*, which with the trigger already consumed silently defeated safe mode every time (2026-07-11).

`_early_display_show(lines)` (`:73-115`) is a one-shot panel bring-up using framebuf's built-in 8x8 font (amber title, white body, ≤16 chars/line). **Currently not called on either path** — both the safe-mode and OTA-apply splashes are disabled pending an early-boot hard fault (BACKLOG 38; see `:131` and `:152-153`).

**2. `ota.apply_staged()` (`main.py:154`)** — before *any* app import.

**3. Crash-loop self-heal / boot-fail counter (`main.py:164-205`).** File `/boot_fails`, limit 5. `machine.reset_cause() == machine.PWRON_RESET` → delete the counter (a human power-cycled; not a crash loop). Otherwise **read, increment, write at `:189-196`**; at `:197-202` a count ≥ 5 removes the file and calls `ota.recover()`. Cleared by `_clear_boot_fails()` (`:169-177`), called from `main()` at `:1166` once the app reaches an interactive state. Whole block wrapped in `try/except` — a failure in the counter must never block a boot.

**4. App import guard (`main.py:212-239`).** All app imports in one `try`; `ImportError` → `ota.recover()` → if it returns, raise `RuntimeError` (again deliberately not `sys.exit()`, which would loop recovery forever).

**5. `gc.threshold(48 * 1024)` (`main.py:249`).** Post-ROMFS churn is ~4 KB/s, so this trades a collection every ~12 s for headroom for the ~33 KB contiguous TLS buffers wanted on reconnect (calibrated 2026-07-06).

**6. Display init (`main.py:1207-1245`).** Order matters: seed `screen_geometry` variant selectors / dividers / scroll speed from config **before** Regions are built (`:1220-1222`), then `init_display(config)`, register driver + regions with `state`, `update_ui_colors`, I2C + LightSensor, commit startup step 1, then spawn Core 1.

**7. `asyncio.run(main(...))`** with a last-resort supervisor (`:1251-1258`): on any exit or exception → log, `logger.flush_to_flash()`, `time.sleep(10)`, `machine.reset()`. The 10 s sleep keeps mpremote interruptible and throttles flash writes if a crash loop develops.

**Task inventory (`main()`, `:1088-1185`):**

| Task | Condition | Cadence |
|---|---|---|
| `log_flush_task` | always | 30 s → `logger.maybe_flush()` |
| `auto_brightness_loop` | always | 200 ms (`brightness.TICK_MS`) |
| `run_dns_server(ap_ip)` | setup mode only | event-driven |
| `poller.run()` | station mode only | `poll_interval_seconds`, wakeable |
| `ota_check_task` | station mode only | 24 h / 1 h after failure / on-demand |
| `button_input_loop` | station mode + buttons OK | 50 ms |
| `watchdog_feeder` | `config.watchdog_enabled` | `timeout_ms // 4` |
| `app.start_server(port=80)` | always | infinite retry loop, 5 s backoff |

**WiFi state machine (`start_station_mode`, `:628-750`).** 3 attempts, each preceded by full `reset_wlan()` (disconnect → **deinit** → 1 s → `active(True)` → 1 s → `config(pm=0xa11140)` → 0.5 s). Per attempt: `rp2.country('US')`, `network.hostname(device_name)`, scan (reports found count + target-SSID visibility), connect, poll at 0.5 s:
- status `-3` LINK_BADAUTH → `app.setup_reason = "bad_auth"`, break to next attempt (`:708-712`)
- status `-1` LINK_FAIL within first 5 s, up to 2× → re-issue `wlan.connect()` in the same attempt (`:715-720`)
- reaching status `2` LINK_NOIP grants **+15 s** over `config.connect_timeout_seconds` (`:723-726`)
- success requires `isconnected()` **and** a valid non-`0.0.0.0` IP (`:733-742`)

Startup step display is **monotonic** — `set_startup_step` never moves backward, so retries read as "Retry n/3" plus attempt dots (`:660-663`).

**Captive portal (`:496-558`).** `get_my_hosts()` = `{device_name.local, device_name, ap_ip}`. `GET /` and the `/<path:path>` catch-all compare the `Host` header (port stripped): legitimate → serve/404; hijacked → `302` to `http://<ap_ip>/#/setup`. SPA served from `/index.html.gz` (littlefs, dev) or `/rom/index.html.gz` (ROMFS, release), whichever exists (`_find_index`, `:303-315`), gzip, ETag = first 8 bytes of SHA-1 as hex (`:319-336`), `Cache-Control: max-age=<config>`; `If-None-Match` match → 304.

**Watchdog (`:785-825`).** Hardware `machine.WDT`, opt-in (once armed it cannot be disarmed and would reboot ~timeout after mpremote interrupts the script). Feeds at `timeout_ms // 4`. **Deliberately stops feeding** — after `logger.flush_to_flash()` — when `health.healthy` is False (Core 1 crashed) or `health.frame_seq` hasn't advanced (Core 1 hung). Armed only *after* the blocking network phase; boot/WiFi is unprotected by design.

**Buttons (`:998-1085`).** PIO1 (PIO0 belongs to HUB75), SM0 = A (GPIO 10, skip), SM1 = B (GPIO 22, lock). Init failure is non-fatal → `(None, None)`.

`_PressTracker` (`:1024-1061`), `_LONG_PRESS_MS = 800` (`:1021`): **short press fires on the RELEASE edge** (only if the hold stayed under threshold); **long press fires mid-hold** the moment the threshold passes, checked per 50 ms poll — because `button.py` emits no events while a button is steadily held. Consuming the long press clears `_press_ms` so the release can't also fire short. Edges are detected against the previous *debounced* state, so a swallowed sub-debounce blip surfaces as two same-state events and produces no edge.

Routing via `MenuController`: menu closed — A short = next game, A long = next league, B short = rotation lock, B long = open league menu. Menu open — A short = move cursor, B short = toggle/DONE, B long = apply + close.

**On-demand OTA seam (`:258-300`).** `app.request_ota_check` is an **attribute seam** so ROMFS app and littlefs `main.py` version independently: an old ROMFS app never calls it; a new ROMFS app on an old `main.py` sees it absent and answers `unsupported`. `request_ota_check()` is synchronous by design (blocks ~1-2 s for the manifest) but only *signals* `ota_check_task` — download/reboot stays single-owner so a concurrent daily check can't double-stage. `_kick_ota_check()` (`:267-274`) sleeps 2 s first: setting the event directly woke the task on the next tick and its synchronous download froze the loop before microdot flushed the HTTP response (client timeout observed 2026-07-14).

**`ota_check_task` (`:839-908`).** Waits up to 120 s for boot traffic to settle (or an on-demand signal). 24 h delay when healthy, **1 h after a failed check**. Feeds the WDT by hand through a `tick` callback during the blocking download. Progress callback commits only on **percent changes** (~100 commits/image vs one per 4 KB chunk) and lazily enters `updating` mode so the daily "already current" check never touches the screen. On success: log, flush, 5→1 countdown with **blocking `time.sleep(1)`** — with no `await` between the first `updating` commit and `machine.reset()`, the poller can never repaint. On exception: restore `prev_mode` if the download had started.

**Auto-brightness (`:911-995`).** `LightSensor` wraps VEML7700 with re-init retries every 15 ticks (3 s) while unavailable, and logs only failing→recovered *transitions* (a broken sensor would flood the ring at 5 Hz). `auto_brightness_loop` is the **sole owner of `driver.set_brightness()`**, re-reads `config.brightness` every tick, and assumes a bright room (`BRI_MAX`) when no reading has ever landed rather than dimming to the floor.

**Time sync (`:453-488`).** `GET {api_url}/time` under a 15 s `wait_for`; sets `machine.RTC()` from `timestamp` (**RTC stays UTC**); returns `utc_offset` seconds. Returns `None` on failure — and `None` is explicitly distinguished from a legitimate `0`; the poller then omits local start times rather than show a wrong-tz one.

### `ota.py` — 413 lines

**Dependency-free by design.** Lives on littlefs (never in ROMFS), uses only built-ins (`socket, ssl, json, hashlib, vfs, machine, os, time, network`). Must keep working when ROMFS — and therefore the entire app including vendored aiohttp — is corrupt. Logs via bare `print` (`:53-56`) because `scoreboard.logger` may not be importable in the recovery paths where this matters most.

**Identity model.** App identity = SHA-256 of its ROMFS image.
- `/app_version` — sha of the image currently in the partition
- `/ota_staging` — downloaded candidate (littlefs has ~2.4 MB free)
- `/ota_pending` — flag file containing the staged image's expected sha
- `/ota_dev` — written by `build.py` when it flashes an image whose sha isn't the published manifest. **While present, `check_and_stage` refuses to update** — which would otherwise be a *rollback* to the older published image (the 2026-07-11 dark-panel incident).

**Lifecycle — stage at runtime, APPLY AT EARLY BOOT:**
1. `check_and_stage()` (`:247-300`) — manifest GET; if sha differs, stream to `/ota_staging` in 4 KB chunks, verify length against manifest `size` **and** SHA-256 against `sha256`, write `/ota_pending`, return True. Sha mismatch → remove staging, raise.
2. `apply_staged()` (`:303-359`) — early boot only. **Idempotent**: re-verifies the staged sha first, so power loss mid-write just re-applies next boot. Gets the partition via `vfs.rom_ioctl(2, 0)`, checks the image fits, erases `ceil(size/block_size)` blocks (`ioctl(6, block)`), writes them (padding the final partial block with `0xFF` since flash was just erased), writes `/app_version`, removes staging + pending.
3. `recover()` (`:366-413`) — raw `network.WLAN` connect from `/config.json`, **removes `/app_version` and `/ota_dev`** to force a re-download (a broken dev deploy heals to the published app), stage, apply, reset. Backoff 10 s doubling to a 300 s cap. Never returns except via `machine.reset()` or an unrecoverable config error.

**Transport (`_https_get`, `:105-137`).** Raw `socket` + `ssl.wrap_socket(server_hostname=host)`, HTTPS **required** (`ValueError` otherwise), 30 s timeout, `HTTP/1.0`. Headers: `X-Api-Key`, `X-Ota-Context` (`"check"`/`"recover"`), `X-App-Version`, plus metadata.

**Request-metadata contract (`:64-102`).** `X-Ota-Proto: 1` plus best-effort `X-Mpy` (`sys.implementation._mpy`), `X-Firmware`/`X-Machine` (`os.uname()`), `X-Device-Id` (`machine.unique_id()` hex), `X-Romfs-Bytes`. Cached after first build. Documented as "spoken by every device FOREVER once flashed" (this file is littlefs, USB-only) so a future backend can route a mixed fleet — per-ABI images, staged rollouts, pinning a known-bad sha — without devices updating first. Every field best-effort; metadata must never break an update.

`fetch_manifest` (`:223-244`) drains until `Content-Length` or EOF because **TLS reads may return partial data**. Unknown JSON keys are ignored so the backend may extend the manifest freely.

### `hardware_diagnostic.py` — 233 lines
Standalone bring-up tool deployed **in place of** `main.py`. Not imported by anything. Single-core, no threads, ~20 FPS. Exercises display + VEML7700 + rotary encoder (GPIO 2/3/4, PIO1) + both buttons, reusing production `Config`, `init_display`, and `scoreboard.brightness` math so the auto-brightness curve is validated on real hardware. Encoder drives a 0-100 preference (encoder button resets to 50 = pure auto); VEML7700 re-inits after 5 consecutive read failures and retries init every 3 s while absent. **The only consumer of `lib/rotary_encoder.py`.**

### `config.json`
Live device config (gitignored). Sections: `network` (ssid, password, device_name, connect_timeout_seconds), `api` (url, key), `display` (poll_interval_seconds, brightness, game_rotation_seconds, variants, scroll_speed_px_per_sec, show_dividers, data_frequency_khz, target_refresh_rate, gamma, blanking_time_ns), `sports` (mlb.enabled, nba.enabled, football.leagues, soccer.leagues), `colors`, `log.level`, `server.cache_max_age_seconds`, `watchdog` (enabled, timeout_ms), `ota.enabled`. Full defaults at `config.py:27-99`.

---

## 2. `scoreboard/` modules

### `__init__.py` — 10 lines
`APP_NAME = "pico-scoreboard"`; re-exports `Config`, `ScoreboardApiClient`, `ApiError`.

### `api_client.py` — 255 lines
Async HTTP client for game data. Deps: `aiohttp` shim, `wire`, `config`, `logger`.

- **Scheme downgrade (`:94`):** `config.api_url` is `https://` but the client rewrites it to `http://` for score polling. A persistent TLS polling connection holds ~21 KB of mbedTLS record buffers for its whole lifetime plus a handshake stall on every reconnect. These routes are unauthenticated backend-side, so **no API key is sent** (a cleartext key would leak). OTA stays TLS-only — its manifest sha is the code-integrity root.
- **Single 4 KB pre-allocated response buffer (`:27`, `:99-101`)**, returned as an **aliasing memoryview valid only until the next request**. Sized ~3.5× the largest body (24×24 RGB565 logo = 1,152 B).
- **Concurrency guard (`:103-129`):** `_request_in_flight` raises `RuntimeError` rather than silently corrupting a response mid-parse. One in-flight request per client.
- **Hard 15 s timeout on every request (`:32`, `:122`)**; on timeout the session is **closed** so the next request reconnects cleanly.
- **ETag (`_get_game_list_inner`, `:209-231`):** scans headers **case-insensitively** (`k.lower() == "etag"`, `:216-218`) because the aiohttp shim's header dict is case-preserving. Returns the **raw value including quotes** so the caller echoes it verbatim as `If-None-Match` — the backend does a strict string match and will not recognize a stripped-quote form. A `304` returns `(304, [], etag)` without reading a body.
- `get_game_state(path, parse, tag)` (`:233-255`): `parse` must be **synchronous and must not await**, so the shared buffer can't be overwritten before it reads. `404` → `None`; other 4xx/5xx → `ApiError`.
- Error bodies are always JSON regardless of `Accept` (`:59-70`). Struct Accept header: `application/x-scoreboard-struct`.

### `api_routes.py` — 144 lines
Mounted at `/api` (`main.py:493`). Full surface:

| Method | Path | Semantics |
|---|---|---|
| GET | `/api/config` | `config.raw` (whole merged dict) |
| PUT | `/api/config` | `config.update_many(json)` — **one flash write**; `CadenceError` → `400 {'error':'invalid_cadence'}`. Then applies live: `colors`→`update_ui_colors`; `display.data_frequency_khz`, `target_refresh_rate`, `gamma`, `blanking_time_ns`, `variants`→`update_screen_variants` (rebuilds Regions), `show_dividers`, `scroll_speed_px_per_sec`. Returns `config.raw`. |
| GET | `/api/status` | network status dict (3 shapes: ap/station/unknown) |
| GET | `/api/logs?since=<seq>` | **NDJSON stream**, one `[seq, ts, level, msg]` per line, via a sync generator (avoids one large on-device body). Clients tail-follow with the last line's seq. |
| GET | `/api/logs/previous` | `send_file(/logs/previous.log)`, 404 if absent |
| POST | `/api/check-update` | delegates via `getattr(request.app, 'request_ota_check', None)`; `501 {'status':'unsupported'}` when absent |
| POST | `/api/reboot` | `_delayed_reboot()` — 1 s sleep, flush log, `machine.reset()` |
| POST | `/api/reset-network` | clears `network.ssid`/`password` → setup mode next boot |

Status dict (`main.get_network_status`, `:390-450`) has shapes `ap` / `station` / `unknown`, all carrying `memory_used`, `memory_free`, `flash_used`, `flash_free` (via `os.statvfs`), `app_version`. AP adds `setup_mode`, `setup_reason` (`no_network_configured` | `connection_failed` | `bad_auth`), `configured_ssid` (only for failure reasons), `ap_ip`, `ap_ssid`. Memory stats **deliberately do not `gc.collect()` first** — observing memory must not change behavior; expect a sawtooth.

### `brightness.py` — 64 lines
Pure functions, no state. `LUX_MIN=2.0`, `LUX_MAX=300.0`, `BRI_MIN=0.05`, `BRI_MAX=1.0`, `EMA_ALPHA=0.08`, `TICK_MS=200`, `RAMP_PER_SECOND=0.2`, `RAMP_STEP=0.04`.
- `smooth_lux(c, r)` = `c + 0.08*(r-c)`
- `lux_to_ambient(lux)` = `t = ln(max(lux,2)/2)/ln(150)` clamped [0,1] → `0.05 + t*0.95`
- `ramp(c, t)` = rate-limit ±`RAMP_STEP`
- `apply_preference(ambient, pref)` = dual lerp: `pref<=50` → `BRI_MIN + (pref/50)*(ambient-BRI_MIN)`; `>50` → `ambient + ((pref-50)/50)*(BRI_MAX-ambient)`. 0 = min, 50 = pure auto, 100 = max.

`RAMP_STEP` is *derived* from `TICK_MS` so ramp speed stays in real units if the tick rate changes.

### `config.py` — 488 lines
Deep-merge of `/config.json` over `_DEFAULTS` (`:27-99`). **Never raises** — `Config()` is constructed at import time in `main.py`, so a corrupt file must not brick boot; it silently falls back to defaults.

Cross-key invariant `_validate_cadence` (`:124-130`): `poll_interval_seconds < game_rotation_seconds` strictly, so the inner poll for the current game fires at least once before rotation advances. Violated in the stored file → both keys reset to defaults with a logged complaint (`:190-195`). `update_many` (`:241-276`) validates the pair **as it will exist after the merge**, so a jointly-valid pair can't be rejected for arriving in the wrong key order, and does **one** flash write for the batch.

Defensive accessors: `screen_variants` → `{}` for non-dict; `football_leagues`/`soccer_leagues` filter to non-empty strings, `[]` for garbage; `scroll_speed_px_per_sec` degrades to 20; `watchdog_timeout_ms` clamped to `[2000, 8300]` (RP2350 WDT max ~8.3 s; floor keeps the feeder under ~2×/sec); `gamma` maps `{"type": "srgb"|"power"|"none"}` → `gamma.SRGB()` / `gamma.Power(value)` / `None`.

`log_level` cached as a plain `int` (recomputed on load/update) because it's checked before every log statement including hot paths. Config is source of truth and pushes to `logger.set_level()`. Note `:10-12`: `import scoreboard.logger as logger` (module-path, not `from scoreboard import logger`) because config is imported during the package's `__init__` — binding by full path is safe regardless of partial-init state.

### `display.py` — 2,056 lines (largest)
`DISPLAY_WIDTH=128`, `DISPLAY_HEIGHT=64`, `FRAME_MS=50` (20 FPS). All render functions are pure readers.

**`Region` (`:206-238`)** — `framebuf.FrameBuffer` sub-view over a rect with stride = parent width, so writes **clip to bounds automatically** (no manual masking). Nestable. RGB565.

**`Regions` (`:240-399`)** — every text slot pre-allocated on Core 0 at init. Fixed: startup (title/step/operation/detail), idle, no_games, setup (5 lines), error (title + 4 lines), 5 menu rows, sport-neutral `play_text`. Variant regions built per sport×screen key from `screen_geometry`, with keys whose active tables are the *same object* sharing one built dict (`:338-346`). `rebuild_variant_regions()` publishes with a single attribute store so the display thread sees old or new, never half-built. `update_for_qr(w,h)` (`:358-399`) narrows setup lines whose y-range intersects the QR to end 4 px before its left edge; lines fully below stay full width. Both **Core 0 only**.

**`LogoPool` (`:496-585`)** — 8 pre-allocated 24×24 RGB565 buffers, LRU eviction, `Accept: image/x-rgb565`, `background_color=000000`. Repeated allocation would fragment the heap. **One sequential caller only** — LRU bookkeeping is mutated across an `await`. A cache re-check after the fetch guards same-key double-fetch (`:563-567`); interleaved different-key callers unsupported.

**Toasts.** Kinds `TOAST_TEXT` (bottom strip), `TOAST_LOCK`, `TOAST_UNLOCK`, `TOAST_SPINNER` (centered overlay). `TOAST_DISPLAY_MS=1500`, `TOAST_STICKY_MAX_MS=20000` (belt against a bug stranding one; requests hard-cap at 15 s).

Icon toasts dim the frame via `_dim_frame` (`:124-157`), **`@micropython.viper`** on `ptr32`: each 32-bit word holds two RGB565 pixels; factor `k/8` = sum of masked shifts `(w>>1)&m1 [+ (w>>2)&m2] [+ (w>>3)&m3]`, each mask clearing bits that would bleed across R5/G6/B5 boundaries after that shift (`m1` also clears the arithmetic shift's bit-31 sign extension). **Masks must be built through variables** (`:129-136`) — full 32-bit literals overflow MicroPython's 31-bit small int and box to objects viper can't combine with native ints, and a one-line `(a<<16)|b` is constant-folded by mpy-cross into that same boxed object. A CPython branch exists for the preview and must stay mask-identical.

Fade ladder `_FADE_TERMS = ((1,1),(1,0),(0,1),(0,0))` → 7/8, 3/4, 5/8, 1/2 (held); fade-in 0→3 from toast start, fade-out 2→0 after expiry, one step per 50 ms frame.

Spinner (`:834-851`): 12 dots on a radius-12 ring, one revolution per 1000 ms, head in 1/256ths of a dot step, `_SPINNER_TRAIL = 10`. Gap dots get their palette entry set to `KEY` so the blit skips them. **Palette-index inversion at import (`:827-831`)**: `gen_toast_icons.py` bakes dot *k*'s color so its RGB565 value equals `k+1`, while `compile_layout.py` assigns indices in row-major first-seen order — the firmware inverts the compiled palette once at import rather than hardcoding a permutation. Contract drift raises at import, not mid-render.

**Sprite palette mutation** — the one place shared module state is written on the render path. `_draw_count_dots` (`:402-425`) and `_draw_base_markers` (`:469-489`) tint and **restore in a `finally`** so a throwing blit can't leave later frames pulsed; `_draw_football_field` (`:1472-1488`) likewise for endzone tints. Defaults captured at import (`:430-432`). Spinner/lock palettes are *not* restored (single owner, every entry unconditionally rewritten each frame) and are instead registered in `SCRATCH_PALETTE_ENTRIES`.

**`pulse(now_ms, period_ms)` (`:63-72`)** — integer triangle wave in [0,256]. Integer-only; the old `sin()` version churned heap because **MicroPython floats are heap-allocated**.

**Renderers.** Mode→renderer table `:1733-1747`, uniform 8-arg signature, adapters drop unused rails. Modes: `startup, idle, no_games, setup, error, updating, mlb_live, pregame, final, soccer_live, soccer_final, nba_live, football_live`. Unknown → `render_idle`.

`render_frame` (`:1694-1727`) — **the menu preempts the mode dispatch entirely**: while `state.menu.active` the menu IS the frame. Rotation, poll commits and toasts continue underneath, invisible — toast drawing lives inside the bypassed mode renderers, so suppression is structural, not special-cased.

**Two time rails (rule: a stall STRETCHES motion but CONSUMES waiting).**
- **Wall rail** (`now_ms`): event windows/durations — toast lifetime, play-flash visibility, the soccer match clock, menu marquee. A GC-stalled frame still counts against these.
- **Frame rail** (`view_elapsed_ms` / `play_elapsed_ms`): advances exactly `FRAME_MS` per rendered frame, latched to Core 0's epoch stamps by `LoopState.advance_and_latch`. All continuous motion — scroll offsets, count-dot pulse, pregame cycle — rides these, so a stalled frame holds position instead of jumping. Under perfect pacing the rails are identical.

`_draw_soccer_clock` (`:1224-1279`) is explicitly on the **wall** rail: the match clock is real time, not motion, so a stalled frame must consume match time. Displayed minute = `anchor_s + ticks_diff(now, anchor_ms) // 1000`, floor minutes matching ESPN's `displayClock` exactly. At/below the period base renders `"45'"`; past it, holds the base and counts added minutes `"45+2'"` in warning color (extra capped at 99). Breaks render `"HT"` (base 45) or `"BREAK"`. Composite centering uses a fixed glyph advance (`_CLOCK_CHAR_W`) since all fonts are fixed-width.

**Bottom-strip priority** on every live screen: **toast > play/commentary flash > sport content** (MLB pitcher/batter, soccer last event, football field strip; NBA has nothing).

**Play flash has no glyph fallback by design** (`:987-995`, repeated per live renderer): `fit_play_text` plus the wire-cap-sized pool make the strip an invariant. Glyph-looping a long line measured >50 ms/frame — halving the frame rate and with it the visible scroll speed (2026-07-12).

**Football field** (`:1429-1509`): endzone palette indices discovered **at import by color value** (`_football_palette_index`, `:1440-1448`) because `compile_layout` assigns indices in first-seen order, which an art edit can reorder. Missing placeholder raises at import (Core 0), never mid-render. Perspective lines 2 px wide, first-down yellow wins where they meet, and **Core 1 only draws precomputed segments** — Core 0 does all projection.

**Core 1 mutation contract (`:1761-1817`) — read before any port.** Four permitted buckets:
1. **`LoopState`** — ALL cross-frame state (pacing, frame rail + epoch latches, render-skip memo, telemetry). Exactly one instance, local to `run_display_thread`, **never passed into `render_frame` or below**. Mechanical audit rule: the name `ls` must not appear below `render_frame`, so renderers structurally *cannot* touch cross-frame state.
2. **Registered scratch** (`scratch_buffers()` / `SCRATCH_PALETTE_ENTRIES`) — write-before-read within a single draw call. `tools/preview` **poisons all registered scratch with sentinels before every frame**, so a violation fails goldens deterministically. Currently `_base_pal`, `_CYCLE_OUT`, `writer._palette_buf`, `writer._int_digits`, plus three palette entry ranges.
3. **Draw targets** — framebuffer and Region views.
4. **`ThreadHealth.frame_seq`** — the single deliberately cross-core counter; it stays on `ThreadHealth` *because* it is cross-core (LoopState's safety argument is thread confinement).

Documented violation shapes: a module-level memo updated from a renderer; scratch read before write (the `_cycle_phase` early-return bug this contract came from); threading `LoopState` into a render function; allocating/formatting on Core 1.

**`run_display_thread` (`:1914-2056`).** Constant 20 FPS, **deadline-based pacing**: each iteration targets `deadline + FRAME_MS` and the sleep absorbs however long the frame took — the old sleep-after-render drifted wall-time scroll math into uneven pixel steps. An overrun **re-anchors** rather than bursting ("a display must never fast-forward"). `health.frame_seq` bumps every tick (rendered *or* skipped) so the watchdog can distinguish hung from quiet.

Static skip: `_STATIC_MODES = ('idle','no_games','error','startup','updating')` re-render only on a new commit, with no toast active or fading and the menu down. Whole loop body wrapped in `try/except` logging at ERROR, guarded by `logger.level >= ERROR` since it can repeat every frame.

`MEM_PROFILE = True` (`:108`) is flagged **TEMPORARY** — per-tick `gc.mem_alloc()` (walks the heap ATB, ~0.5 ms) reported every 10 s. Explicitly never to be read over HTTP (perturbs what's measured) or logged at ERROR (a flash flush from this core would *cause* stutter). Frame-health telemetry reports every 60 s.

### `dns.py` — 102 lines
UDP socket on `0.0.0.0:53`, non-blocking, 50 ms sleep when idle. **The task must never die** — a malformed packet is logged and dropped, never allowed to raise (`:43-48`). Yields (`sleep_ms(0)`) after every packet so a burst can't starve the web server on the same loop.

`_build_dns_response` (`:55-102`) answers **every** query with the AP IP: echoes the transaction ID, flags `0x8180`, counts `QD=1 AN=1 NS=0 AR=0`, copies the question verbatim, appends an answer with name pointer `0xC00C`, type A, class IN, TTL 60, rdlength 4, the IP. Walks length-prefixed labels with bounds checks so a truncated packet raises a clean `ValueError` rather than `IndexError` from a wild read.

### `football.py` 397 / `mlb.py` 363 / `nba.py` 300 / `soccer.py` 369
One contract: fixed numeric section decoded in a **single C-level `struct.unpack_from`**, then a bounds-checked length-prefixed walk over the response memoryview. No intermediate dict tree, no `json`. Models are **plain-attribute value types** treated as immutable after construction — no property descriptors, because the display thread reads these at 20 FPS and attribute access must stay on MicroPython's fast path. Every parser: `check_version`, min-size check, unpack, code-range validation (fail loud), string walk, **exact trailing-byte check** (`if o != end: raise`).

| Sport | Live fmt / size | Pregame fmt / size | Final fmt / size |
|---|---|---|---|
| MLB | `<BBBBBBBHHIIII` / 27 | `<BBHHHHIIIII` / 30 | `<BBBHHIIII` / 23 |
| NBA | `<BBBHHIIII` / 23 | `<BHHHHIIIII` / 29 | `<BBBHHIIII` / 23 |
| Football | `<BBBBBBBBHHIIII` / 28 | `<BHHHHIIIII` / 29 | `<BBBHHIIII` / 23 |
| Soccer | `<BBHHHIIII` / 24 | `<IIIII` / 20 | `<BHHIIII` / 21 |

**MLB.** Live flag `AT_BAT=0x01`. Bases bitfield `first=0x01, second=0x02, third=0x04`. Half code indexes `(TOP, MIDDLE, BOTTOM, END)` singletons. Pregame flags `WEATHER=0x01, AWAY_RECORD=0x02, HOME_RECORD=0x04, AWAY_PROBABLE=0x08, HOME_PROBABLE=0x10`. **Absent numeric fields arrive as 0 and are surfaced as `None`** so the display never renders a fake 0-0 record. Final linescores are **copied out with `bytes(...)`** because the source memoryview aliases the client's reusable buffer.

**NBA.** Clock is ESPN's **display string** (`"10:08"`, `"53.0"` under a minute) — a stop-clock can't be extrapolated (no clock-running signal). Phases `IN_PROGRESS=0, HALFTIME=1, END_OF_PERIOD=2`. `period_name`: Q1-Q4, OT, 2OT… `PregameGame` is **duck-typed to the MLB pregame contract** so the whole MLB pregame pipeline is reused; weather/probables permanently `None`.

**Football.** Live flags `LAST_PLAY=0x01, SITUATION=0x02, POSSESSION_HOME=0x04, RED_ZONE=0x08, TIMEOUTS=0x10`. The situation is **flattened**: absent (bit 1 clear) reads as `SIDE_NONE` with zeroed down/distance/yard_line regardless of what the fixed section carries. Timeouts → `None` when not advertised. Pregame adds `AWAY_RANK=0x04, HOME_RANK=0x08` → display-shaped rank strings ("#3 OHIO STATE") that **ride the probable-pitcher slot**. `parse_game_detail(buf, league)` threads the league display name into the pregame model only (the wire deliberately doesn't carry it — the device knows which endpoint it hit). `LEAGUE_NAMES = {nfl, college-football}`.

**Soccer.** Clock travels as **elapsed seconds** (floor-minute, monotonic across the match: 2nd half starts at 2700), not a display string — the display thread extrapolates from a `ticks_ms` anchor, which a string cannot support. Live flags `BREAK=0x01, EVENT=0x02, EVENT_RED=0x04, EVENT_AWAY=0x08, EVENT_HOME=0x10, COMMENTARY=0x20`. Periods `HALF_FIRST=1 … HALF_SHOOTOUT=5`; stoppage thresholds `_BASE_MINUTES=(45,45,90,105,120)` indexed by `min(period, HALF_ET_SECOND)`. Events: goal / red card only. Flavors `FT_REGULAR/FT_AET/FT_PENALTIES`. `LEAGUE_NAMES = {usa.1: MLS, eng.1: PREMIER LEAGUE, mex.1: LIGA MX, fifa.world: WORLD CUP}`.

Soccer's pregame duck-typing is the most contorted: `venue` carries the **league name**, the **stadium name rides the weather-condition slot**, and the **team abbreviation rides the probable-pitcher slot** (`pregame_team`, `:283-291`) so the lower half of the screen isn't empty.

### `inning_half.py` — 38 lines
Four-variant DU (`Top`, `Middle`, `Bottom`, `End`) as plain classes with module-level **singletons**. Carries no data, so deserialization reuses them (no per-parse allocation) and consumers compare by **identity** (`if half is TOP`).

### `logger.py` — 190 lines
Three layers, each surviving the failure of the one above.
- **RAM ring:** 200 slots × 200-char max, **mutated in place** as `[seq, unix_ts, level, msg]`, slot index = `seq % 200`. Safe from either core — a `_thread.allocate_lock()` guards only slot-index bookkeeping. `_snapshot` guards a torn wrap by re-checking `slot[0] == seq` (`:114`).
- **Flash:** `/logs/current.log`, rotated to `/logs/previous.log` **once at boot** (`main.py:1203`), so the prior session survives exactly one reboot. `flush_to_flash()` rewrites the whole ring (~25 KB max), **Core 0 only**, never raises. Policy (`:165-190`): flush if an ERROR was recorded since the last flush **and** ≥5 min have passed (first error of the boot flushes immediately), or if unflushed entries exist and ≥1 h has passed (heartbeat). **Healthy quiet operation performs zero flash writes.**
- **USB:** read the files directly when the network (or Core 0) is dead.

Levels `NONE=0, ERROR=1, DEBUG=2`. Module-level `level` int read directly by hot paths.

### `menu.py` — 189 lines
`MenuController` owns the **whole** session on Core 0: item list, working checkbox flags, cursor + scroll window, timeout. Everything Core 1 draws is pre-built here — each label rendered to a 1-bit strip **once per open** (`:114-115`), and the visible 5-row window, highlight index and scrollbar thumb computed per publish. `display.render_menu` is a pure reader.

Semantics (user-locked): the checked set is a **session** rotation filter over configured league sources (resets to all-checked on reboot; persisted config still owns which leagues are polled at all). A short = advance cursor (items, then DONE, wrap). B short = toggle / activate DONE. **EVERY exit applies** — DONE, B hold, and the 10 s inactivity timeout. There is deliberately no cancel path. **The last checked league cannot be unchecked** (silent no-op). Toast feedback is unavailable under the menu by design.

`_open()` refuses while `mode == 'updating'`. Thumb geometry (`_TRACK_Y0=1, _TRACK_H=50, _MIN_THUMB_H=4`) computed here so Core 1 draws two rects verbatim, and **must mirror** `display.py`'s menu constants. `_publish()` builds **fresh lists every time** — previously published ones may still be latched by Core 1 (wholesale-replacement contract).

### `poller.py` — 602 lines
All state on the `GamePoller` instance — no module/class-level mutable state. The API client supports one in-flight request, so **one poller owns every configured league** and merges their slates into a single rotation.

**`LeagueSource` (`:86-121`)** — key (`"soccer/usa.1"`, namespaces logo cache slots and rotation identity), sport (variant scoping), log tag, display name, base path, parse callable, `commit_live`/`commit_final`. Pregame is sport-agnostic and needs no slot. Paths `{base}/games`, `{base}/games/{id}`, `{base}/teams/{abbr}/logo`. `sources_from_config` (`:223-239`) order: MLB, NBA, football leagues, soccer leagues (config order).

**Rotation (`:477-535`).** **Live-first across the whole merged slate**: while any listed game in any league is live, only live games rotate; with zero live, finals rotate first then pregames — leagues in configured order, backend order within a league. Only a truly empty merged slate shows `no_games`. The league filter restricts to its sources; **a filter whose whole slate empties falls back to the full rotation** (the filter is kept — its games may return) rather than blanking the board. The currently-shown `(source, id)` **keeps its position** in the rebuilt rotation if still present, else the index resets to 0.

**ETag / list refresh (`:439-475`).** Per-source ETag sent as `If-None-Match` on non-initial refreshes; `304` keeps the cached slate. **A single source failing keeps its cached slate** (a dead league feed must not blank the others); the tick counts as failed — feeding the error screen — only when EVERY source fails.

**Backoff / error screen (`:358-392`).** `MAX_FAILURES = 5`. On reaching it, `set_error("API ERROR", lines)` with `_friendly_error` (`:73-83`): `TimeoutError`→("Timeout","backend not responding"), `ApiError`→(f"HTTP {code}", error), `DeserializeError`→("Bad response", f"{path} {message}"), `OSError`→("Network error", str), else type name. Detail split across up to two 25-char lines plus `failing for {n}m`. Recovery logs the streak length. **There is no exponential backoff** — the sleep is always `poll_interval_seconds`, interruptible by `self._wake`.

**Skip state machine (`:288-317`, `:394-437`).** Poll loop, button hooks and the skip machine all run on Core 0's single asyncio loop, so the skip flags are plain booleans with no locking. A press landing while a skip is armed or in flight is **rejected, not queued**, and dims the visible toast one cycle (`pulse_toast()`). A consumed skip owns the sticky spinner toast for exactly that tick; the `finally` at `:434-437` tears it down on **every** exit path.

`skip_league` (`:537-557`) scans forward cyclically for the first entry of the next distinct league, staying **within** the league filter. Single-league slates degrade to a normal skip. `toggle_lock` toasts non-sticky so a lock toast fired mid-skip survives the skip tick's `clear_toast_if_sticky()`.

**`_poll_current` (`:559-602`).** Every tick re-fetches the current game **including static pre/post screens** — that standing re-poll is what lets a pregame card notice its own pre→in flip mid-view. No flicker on unchanged re-commit because setters only restamp the animation clock when `(mode, game_id)` changes. A `404` (detail `None`) skips the slot and lets the next rotation pick up a fresh list.

**`_flash_play` (`:123-143`)** is the one machinery for every sport's flash line (MLB play, NBA play, football play, soccer commentary). The write buffer's previous `play.id` is carried forward after each commit, so comparing against it detects new lines with **no poller-local state**. Order matters: fit first; text, window and strip must all describe the same string; **the strip must always exist**.

**Soccer stale-clock guard (`:155-167`)** — carries `(game_id, clock_seconds)` from the previous poll and hands the previous value to the setter for the SAME game, so local ticking stops when upstream stops advancing while claiming in-play.

### `screen_geometry.py` — 451 lines
**Convention split:** coordinates registered to sprite *art* live in the Aseprite and reach the firmware through `compile_layout.py`; coordinates registered to *text slots and drawn primitives* live here as plain code.

Tables map a slot name to a 4-tuple `(X,Y,W,H)` (becomes a `Region`) or an `int` scalar (`DIVIDER_X`, `SEPARATOR_Y`, `TIMEOUT_Y`). Variant selection is scoped **per sport × screen** (`mlb_pregame`, `soccer_live`, …). Table sets start as shared references — forking a sport's look means a new dict and repointing that key.

Registry `_TABLES` (`:382-395`) / selection `_ACTIVE` (`:397-410`): all pregame keys → `_PREGAME` (single design "C"); `mlb_final`/`nba_final`/`football_final` → `_FINAL` (A/B/C, default **C**); `soccer_live` → `_SOCCER_LIVE` (A/B/C, default A); single-design tables for `soccer_final`, `mlb_live`, `nba_live`, `football_live`. `set_variants` (`:418-431`) **ignores unknown keys and letters**.

**The scroll-speed constraint (`:50-57`, restated at `fonts/__init__.py:410-411`) is load-bearing:** speeds must evenly divide 20 FPS. The offset is derived from wall time, so S px/s advances S/20 px per frame; a non-integer px/frame (30 → 1.5) is realized by floor math as alternating 1 px and 2 px steps — **every third pixel column is never displayed**, reading as a rhythmic stutter. Legal smooth values: 20, 10, 5, 4, 2, 1; 40 is uniform but coarse. `_SCROLL_SPEEDS = (5, 10, 20, 40)` is the config-accepted set; anything else degrades to 20.

Tunables: `PREGAME_INFO_DWELL_MS=4000`, `PREGAME_SCROLL_PAUSE_MS=1000`, `PREGAME_SCROLL_PX_PER_SEC=20`, `FINAL_LS_PAUSE_MS=1800`, `FINAL_LS_PX_PER_SEC=10` (10 gives uniform 2-frames-per-pixel; 12 showed every pixel but with uneven dwell), `SOCCER_SCROLL_PAUSE_MS=1500`, `PLAY_TEXT=(51,43,76,16)` (fixed, shared by all four live screens).

Football field mapping (`:362-374`): `FOOTBALL_FIELD_YARD0_X=14`, `FOOTBALL_FIELD_LOS_MAX_X=113`, `FOOTBALL_VP_X=63`, `FOOTBALL_PERSP_NUM=10`, `FOOTBALL_PERSP_DEN=63`. Field spans 100 yards at 1 px/yard between goal lines; a line whose bottom endpoint is at x meets the field's top row at `x + (VP_X - x) * 10 // 63`.

Font metrics (all fixed-width): spleen_5x8 = 5×8, unscii_8 = 8×8, unscii_16 = 8×16. Logos 24×24, panel 128×64.

### `state.py` — 1,965 lines (second largest)
**Triple-buffered mailbox (`TripleBufferedState`, `:722-775`).** One writer (Core 0), one reader (Core 1). Three buffers are provably sufficient: one is `latest` (published), one may be `reading` (latched for the frame), the writer gets whichever is neither. **The lock guards only index bookkeeping**; the carry-forward copy (`latest` → new write buffer) happens *outside* it.

`_SEQ_MASK = 0x3FFFFFF` — the commit sequence wraps below MicroPython's small-int limit so incrementing never promotes to a heap-allocated big int. Consumers compare with `!=` only. API: `acquire_display_state()` → `(buffer, seq)`; `get_write_state()` / `commit_state()`.

**Typed state classes (`:103-710`)** — plain data plus `copy_from()`. Stated invariant: adding a field means adding it to `__init__` AND `copy_from` of the same class, side by side.

`StateBuffer`: `mode`, `last_update_ms`, `animation_start_ms`, `startup`, `setup`, `error`, `updating`, `ui_colors`, `mlb_live`, `play`, `pregame`, `final`, `soccer_live`, `soccer_final`, `nba_live`, `football_live`, `toast`, `menu`, `home_logo`, `away_logo`. `play` is **top-level, not per-sport** — the cross-sport flash slot.

**Modes:** `startup, idle, no_games, setup, error, updating, mlb_live, pregame, final, soccer_live, soccer_final, nba_live, football_live`. `final` is shared by MLB/NBA/football (one `FinalView`, distinguished by `variant_key` + `total_label`); soccer has its own `soccer_final`.

**Startup phase (`:782`).** `set_startup_step` is a no-op after `finish_startup()`. Step is **monotonic** (`:826-827`). `finish_startup(target_mode, **kwargs)` (`:850-871`) is the single transition point: clears startup fields then dispatches to `set_setup_mode` / `set_error` / `set_mode`.

**The shared view-identity rule.** Every game-screen setter restarts `animation_start_ms` **only when `(mode, game_id)` changes** (`:1265-1269`, `:1418-1421`, `:1483-1486`, `:1548-1551`, `:1617-1620`, `:1667-1670`, `:1749-1752`). A standing re-poll keeps the scroll/cycle where it is; everything else is rebuilt every call so late data corrections still flow.

**`_StripPool` (`:1131-1154`)** — **ping-pong pair** of 1-bit strip buffers per text slot. Strips are shared by reference across the triple buffers and Core 1 may still be blitting the outgoing view's strip for a frame after a commit, so each rebuild lands in the buffer the outgoing view is NOT using. Two suffice: rebuilds are seconds apart, Core 1 lags by at most one frame. `cap_px` must be a multiple of 8.

Pools (pre-allocated at import): linescore header/away/home 320×8, venue/weather 256×8, away/home pitcher 128×8, **play 2048×16**, event name 128×8, at-bat pitcher/batter 128×8, soccer scorers away/home 320×8. The play pool is sized to the wire format's 255-byte string cap (255 unscii_16 chars × 8 px = 2040 ≤ 2048), so **no legal play text can overflow it and the per-glyph fallback is structurally unreachable**. Costs 8 KB vs 2.5 KB at the old 640 px.

**Team color brightening (`_team_color_to_rgb565`, `:52-65`).** `_TEAM_COLOR_MIN_CHANNEL = 128`. Teams whose primary's brightest channel is darker scale up proportionally. Hue preserved for chromatic colors; pure black → bright gray (128,128,128). Lives in `state.py` so Core 0 setters can pre-brighten without importing display.

**QR generation (`:878-914`).** Lazy `from miqro import QRCode`. Payload `WIFI:T:WPA;S:{ssid};P:{password};;` or `WIFI:T:nopass;S:{ssid};;`. Adds a **4-module quiet zone** by blitting into a larger MONO_HLSB framebuffer. Failure caught and logged; the setup screen just loses the QR.

**Pregame build (`set_pregame`, `:1242-1344`).** Records → strings (empty when not advertised). Local time via `time.gmtime(start_epoch + utc_offset_s)` → `"7:05 PM"`; **omitted entirely when `utc_offset_s is None`**. Date phase `"WED JUL 16"` only when the game's *local* day differs from today's — **self-heals across midnight** because the setter re-runs on every poll. Weather `"72F PARTLY CLOUDY"`, or bare condition without a temperature. Info cycle is parallel `(texts, ends, strips)` where `ends` are cumulative dwell ms; per-phase dwell is `max(PREGAME_INFO_DWELL_MS, pause + scroll + pause)` sized against the actual region width (`:1200-1210`).

**Linescore finals (`:1352-1450`).** Rows are **equal-char-count strings, 3 chars/column** (`"%2d "`), so all three measure identically in the fixed-width font and scroll in lockstep. Missing trailing columns get `" X "` (walk-off convention). A width mismatch logs and pads to the widest rather than crashing. Shared by MLB (innings, `"R"`, `"F/%d"` when >9) and NBA/football (quarters, `"T"`, `_ot_final_text` → FINAL / F/OT / F/2OT).

**Soccer live (`:1534-1606`).** Anchors the clock (`clock_anchor_s` + `clock_anchor_ms`) rather than storing a string. `clock_running` is False during breaks, when the stale guard trips, **and** once a shootout starts. Event line `"GOAL 90'+3'"` / `"RED CARD …"` over the scorer name in the scoring team's brightened color.

**Football live (`:1732-1835`).** All field geometry precomputed on Core 0: `"3RD & 7"` (or `"& GOAL"` when `yard_line + distance >= 100`), the possession arrow x, the yardline→pixel map, both perspective endpoints via `_football_top_x` (`:1720-1729`, integer round-half-away). The yardline convention is flagged in-comment as **needing re-validation on live games** (BACKLOG): ESPN's `yard_line` is possession-relative, away advances left→right, home mirrors. `clock_low` fires only where the clock can end a half (Q2, Q4, OT).

**Runtime config appliers (`:1854-1965`).** `update_ui_colors`, `update_screen_variants` (sets selectors **then** rebuilds Regions), `update_show_dividers`, `update_scroll_speed`, `update_display_frequency`, `update_display_refresh_rate`, `update_display_gamma`, `update_display_blanking_time` (which also **recomputes the refresh rate**). Driver and Regions held as module-level references registered by `main.py`.

### `textfold.py` — 89 lines
Folds codepoints above 0xFF to the fonts' ASCII + Latin-1 repertoire at **wire ingest only** (`wire.read_str`) — the render path never sees this module. Latin-1 names render natively; Latin Extended-A/B diacritics fold to base letters (Jokić→Jokic, Şengün→Sengün); typographic punctuation folds to ASCII; unmapped codepoints pass through as `'?'`.

The 1:1 table is **two parallel const strings, not a dict** (`:25-52`): string literals in a ROMFS-deployed `.mpy` are memory-mapped in place, so the table costs no heap; lookups are a rare-path linear scan. `assert len(_SRC) == len(_DST)` at `:66` — a length drift would silently misfold everything after the drift point, so it fails the import instead. `_MULTI` (`:55-62`) holds the six widening folds (Ĳ→IJ, ŉ→'n, Œ→OE, …→...). `fold_text` returns the string itself (no allocation) when nothing needs folding.

### `wire.py` — 183 lines
`WIRE_VERSION = 2`, `STRUCT_CONTENT_TYPE = "application/x-scoreboard-struct"`. Header 2 bytes: byte0 = version, byte1 = state. States `PRE=0, IN=1, POST=2` — also the per-entry tags in the game list and the ETag tokens. Version mismatch fails loudly (a stray JSON body starts with `{`/`[` and fails the version check immediately).

`read_str` (`:59-80`) is the **single point where every wire string enters the firmware**, so it also does the textfold normalization — with an O(1) fast path: a decoded UTF-8 string is pure ASCII exactly when char count equals byte count. Game ids are folded too.

`parse_game_list` (`:157-183`): version byte, u8 count, then `count` × (u8 state, length-prefixed id), in backend (chronological) order, with an exact trailing-byte check. `dispatch_detail` (`:132-154`) dispatches on the state byte to the sport's three `from_struct` classes, threading `league` into the pregame class only for multi-league sports.

Shared value types: `TeamColors` (primary/alternate packed RGB), `TeamState` (abbr/score/colors), `PregameTeam` (abbr/colors/wins/losses/pitcher), `LastPlay` (id/text). `DeserializeError` carries a byte-offset `path` (`"@29"`) plus a message.


## 3. `lib/`

### `hub75/driver.py` — 1,101 lines. **The section to read closely for the Rust port.**

CPU-free HUB75 driver. Owns one PIO block (both state machines + all program memory), **four DMA channels**, and a pair of double-buffered bitplane buffers. After construction the panel refreshes continuously in hardware — the CPU only calls `load_*` and `flip()`. `COLOR_BIT_DEPTH = 8` → 8 BCM bitplanes → 256 levels/channel.

Production wiring (`display.init_display`, `display.py:601-615`): `row_addressing.Binary(base_pin=Pin(11), bit_count=5)` (32 addresses), `shift_register_depth=128`, OE = GPIO 28, base clock = GPIO 26 (LAT on 27), base data = GPIO 16 (R1..B2 on 16-21), PIO0.

#### The two-state-machine split

**Data SM (`data_program`, `:1046-1057`)** — clocks pixel data into the panel's shift register. Runs at `data_frequency * 2` (each pixel clock needs a rising and a falling edge). Side-set is 2 bits: `CLOCK_ASSERTED=0b01`, `LATCH_ASSERTED=0b10`, `BOTH_DEASSERTED=0b00`. `out_init` = 6 pins (R1,G1,B1,R2,G2,B2), `SHIFT_RIGHT`, autopull at 32.

```
out(y, 32)                     .side(BOTH_DEASSERTED)   # seeded with depth-1
wrap_target()
mov(x, y)                      .side(BOTH_DEASSERTED)   # reload pixel counter
label("write_data")
out(pins, 8)                   .side(BOTH_DEASSERTED)   # 6 data bits (8 consumed)
jmp(x_dec, "write_data")       .side(CLOCK_ASSERTED)    # clock edge on the loop
wait(1, irq, _LATCH_SAFE_IRQ)  .side(BOTH_DEASSERTED)
irq(_LATCH_COMPLETE_IRQ)       .side(LATCH_ASSERTED)    # latch rises here
wrap()
```

Seeded once with `shift_register_depth - 1` via `data_state_machine.put()` (`:1071`). Note `out(pins, 8)` consumes 8 bits per pixel from the 32-bit word but only 6 pins are mapped — that is the bitplane packing (byte-aligned 6-bit words, 4 per 32-bit DMA transfer).

**Address SM (`address_program`, `:1009-1033`)** — walks row addresses, drives OE, owns all BCM timing. Side-set is 1 bit = OE (`OE_ASSERTED=0b0`, `OE_DEASSERTED=0b1`, active-low). `SHIFT_RIGHT` both directions, autopull at 32.

```
jmp("initialize")                       # skip the first OSR discard
label("increment_bitplane")
out(null, 32)                           # discard the stale 'on' word
label("initialize")
increment_bitplane()                    # x = row_address_count
out(isr, 32)                            # ISR = 'off' delay, OSR = 'on' delay (autopulled)
wrap_target()
increment_address()                     # decrement x; write address pins; or fall to bitplane
irq(_LATCH_SAFE_IRQ)                    # tell data SM it may latch
wait(1, irq, _LATCH_COMPLETE_IRQ)       # wait for the latch
mov(y, isr)
label("off_delay_before_enable")
jmp(y_dec, ...)          .side(OE_DEASSERTED)   # blanking before enable
mov(y, osr)
label("on_delay")
jmp(y_dec, ...)          .side(OE_ASSERTED)     # the lit interval
mov(y, isr)
label("off_delay_after_disable")
jmp(y_dec, ...)          .side(OE_DEASSERTED)   # blanking after disable (anti-ghosting)
wrap()
```

#### IRQ handshake
The entire synchronization mechanism. `_LATCH_SAFE_IRQ = 0` (address SM → data SM: row switched, you may latch); `_LATCH_COMPLETE_IRQ = 1` (data SM → address SM: latch has risen). The comment at `:1054-1055` notes the latch triggers on the **rising** edge, so the IRQ can safely fire before the latch is deasserted. After the handshake the two SMs run concurrently for the rest of the row — row time is gated by whichever is slower.

#### Row addressing — three schemes, each generating a different `increment_address` / `increment_bitplane` pair
- **`Binary`** (`:839-873`, what the scoreboard uses). `x` counts **down** from the highest address and is written **inverted** (`mov(pins, invert(x))`) so the panel sees a count up from 0. `address_update_cycles = 2`, `bitplane_transition_extra_cycles = 8`.
- **`ShiftRegister`** (`:875-962`) — clocks a single `1` through an external register. Clock frequency is realized in PIO **delay slots**, max 15 (4 delay bits, since the 5-bit field is shared with 1 side-set pin), so too low a frequency raises `ValueError` reporting the minimum achievable. `address_update_cycles = 1 + 3*(1+delay)`.
- **`Direct`** (`:963-1001`) — one-hot; `x >>= 1` implemented as `mov(osr,x); out(null,1); mov(x,osr)` with `y` borrowed to preserve OSR. `address_update_cycles = 7`, `bitplane_transition_extra_cycles = 12`.

**The 5-bit SET immediate gotcha (`:855-865`, `:880-890`) — a real trap for a port.** PIO's `SET` has a 5-bit immediate, so `bit_count >= 5` cannot load `1 << bit_count` directly: the assembler **silently drops the high bit**, yielding 0, which sends the address SM into an infinite non-displaying loop. The workaround builds the value in ISR: `set(x,1); mov(isr,null); in_(x, 32-bit_count); mov(x,isr)`. The same construction is used for `ShiftRegister` depths > 31, which are therefore **restricted to powers of 2** (explicit `ValueError`).

#### DMA chaining — the read-address-trigger technique
Four channels in two self-perpetuating pairs (`:223-283`).

*Data path:* `_data_buffer_dma` reads the active bitplane buffer (32-bit, `inc_read=True`, `inc_write=False`) into the data SM's TX FIFO, paced by that SM's DREQ, `count = len(buffer)//4`, `chain_to = _data_control_dma`. `_data_control_dma` is a **single 32-bit transfer** from `_active_buffer_address_pointer` (a 1-element `array('I')` holding the buffer address) into `_data_buffer_dma.registers[15]` — **the read-address-trigger alias**, `_DMA_READ_ADDRESS_TRIGGER_INDEX = 15`. Writing that register both sets the read address and **retriggers** the channel. The pair therefore loops forever with zero CPU and re-reads the pointer each cycle.

*Address path:* identical structure — `_address_timing_dma` streams the 16-word `_timing_buffer` (`COLOR_BIT_DEPTH * 2` u32s) into the address SM; `_address_control_dma` rearms it from `_timing_buffer_pointer`.

**`flip()` (`:404-413`) is therefore two stores and nothing else:** toggle the index, write the new address into `_active_buffer_address_pointer[0]`. The control DMA picks it up at the next frame boundary. No tearing, no blocking, no CPU work.

#### BCM bitplane layout
Buffer size = `row_address_count * shift_register_depth * COLOR_BIT_DEPTH` bytes. For this panel: 32 × 128 × 8 = **32,768 bytes per buffer, two buffers = 64 KB**. Pairs of rows (top half + bottom half) are packed into 6-bit words `(R1,G1,B1,R2,G2,B2)` — one bit per channel per bitplane, 8 bitplanes. Conversion happens in native code (`load_rgb888` / `load_rgb565`), with gamma applied per channel during conversion and an optional `row_map` remapping logical pixel chunks to physical shift-register order (`_validate_row_map`, `:568-588`: even length, ≥2, must divide the pixel count evenly, entries in `[0, len)`).

#### Timing buffer / brightness (`_update_timing_buffer`, `:620-640`)
Per bitplane *i*: `brightness_cycle = base_cycles << i` (**that shift IS the binary code modulation weighting**), `on_cycles = int(brightness * brightness_cycle)`, `off_cycles = ((brightness_cycle - on_cycles) // 2) + blanking_cycles`. The off value is **halved because the delay occurs twice per bitframe** (once before enable, once after, to prevent ghosting). Written as interleaved `[off, on]` pairs — hence 16 words for 8 bitplanes. **Brightness is OE duty cycle**, so it doesn't change the refresh rate directly but does bound the achievable maximum.

#### Refresh-rate model
`set_target_refresh_rate` (`:706-785`) binary-searches the integer `base_cycles` whose estimated rate is closest to the target (expanding the upper bound as needed, then comparing candidate vs candidate-1 for the arithmetically closer one). `_estimate_refresh_rate` (`:643-703`) is a genuine cycle model derived from counting the assembly: `ADDRESS_DISPLAY_OVERHEAD_CYCLES=7`, `DATA_HANDSHAKE_OVERHEAD_CYCLES=2`, `DATA_RELOAD_OVERHEAD_CYCLES=1`, `DATA_CYCLES_PER_PIXEL=2`, plus per-addressing-mode `address_update_cycles` and `bitplane_transition_extra_cycles`. Per bitplane, `row_cycles = max(address_display_cycles, data_transfer_cycles) + handshake_cycles` (the SMs run concurrently after the handshake), scaled by `data_clock_ratio = system_freq / (data_freq * 2)`.

#### Other public API
- `load_rgb888(buf)` / `load_rgb565(buf)` → native conversion into the **inactive** buffer (not visible until `flip()`). RGB565 is LE, matching `framebuf.RGB565`, so a FrameBuffer's backing buffer passes directly.
- `clear()` → zero the inactive buffer.
- `set_frequency(hz)` (`:416-442`) — writes the SM clock divider **register directly** (`PIO_BASE + 0x0C8 + sm*0x18`), integer part in bits 31:16, 1/256 fractional in 15:8, **without stopping the state machine**. Deliberately does *not* re-balance refresh timing.
- `set_brightness(f)`, `set_blanking_time(ns)`, `set_gamma(g)`, `sync_system_frequency()`, and properties `row_address_count`, `shift_register_depth`, `data_frequency`, `system_frequency`, `brightness`, `blanking_time`, `refresh_rate`, `gamma`.

**Gamma LUT (`_create_gamma_lut`, `:592-617`)** — 256-byte table. `SRGB`: `x <= 0.04045 ? x/12.92 : ((x+0.055)/1.055)**2.4`. `Power(v)`: `x**v` (`v == 1.0` short-circuits to identity). `None`: identity.

#### `deinit()` (`:289-360`) — subtle, port carefully
A *graceful* stop rather than a forced one, so the DMAs end in a clean state:
1. Reconfigure `_data_buffer_dma` to chain to **itself** (breaking the ping-pong) with `irq_quiet=False` so the IRQ fires exactly when the chain breaks; block on a lock released by that hard IRQ handler.
2. Close the control DMAs **first** (so they can't retrigger), then the others.
3. **Force-set both handshake IRQs** via `PIO_IRQ_FORCE` (`+0x034`) to unblock any SM stuck on a `wait` — after the DMAs close, the address SM may stall on `out` with an empty FIFO and never fire `_LATCH_SAFE_IRQ`, leaving the data SM blocked forever.
4. Clear and poll the TX stall flag in `PIO_DEBUG_FLAGS` (`+0x008`, bit `24 + sm`) until the data SM is confirmed stalled; deactivate both SMs.
5. **Clear the leftover IRQ flags** (`+0x030`) — otherwise the next init's data SM skips its first wait and **every row is offset by one**.

**PIO block index is recovered by regex over `repr(pio)`** (`:46`, `:789-796`) because the MicroPython API doesn't expose it; a Rust port gets this statically. Base addresses `0x50200000` / `0x50300000` / `0x50400000` (PIO2 is RP2350-only). DREQ index = `(pio_block << 3) | (sm & 0b11)`. Nearly every method carries `@micropython.native`.

### `hub75/display.py` — 79 lines
`Hub75Display(framebuf.FrameBuffer)` — RGB565 view with a dedicated `width*height*2` backing buffer. `show()` = `load_rgb565(buffer)` + `flip()`. Width defaults to `shift_register_depth`, `height = row_address_count * 2` (standard indoor-panel assumption). Subclassing `FrameBuffer` makes it drop-in for anything consuming one.

### `hub75/row_addressing.py` (129) / `gamma.py` (35) / `constants.py` (25)
Config value types (`Binary`, `ShiftRegister`, `Direct`; `SRGB`, `Power`) and `COLOR_BIT_DEPTH = 8` plus the `ARCH` computation from `sys.implementation._mpy >> 10 & 0xF`. Note the constants file warns these values are **also passed in as C macros at build time**, so changing them requires a native rebuild.

### `hub75/native/` and `hub75/effects/`
Architecture-dispatched precompiled native modules (`armv6m` = RP2040, `armv7emsp` = RP2350), selected by `ARCH`. `native` exports `load_rgb888`, `load_rgb565`, `clear`, `pack_hsv_to_rgb565`, `pack_hsv_to_rgb888`, `hsv_to_rgb`. `effects` exports `render_plasma_frame`, `render_fire_frame`, `render_spiral_frame`, `render_balatro_frame` — **not used by the scoreboard app** (demo content). `.pyi` stubs document the shared interface. RGB565→RGB888 expansion replicates MSBs into the empty LSBs before gamma. `hub75/benchmarks.py` (349 lines) is a bring-up benchmarking harness, not on the app path.

Only two `native` functions are used by app code: the two `load_*` (via the driver) and `pack_hsv_to_rgb565` (the MLB critical-count dot pulse, `display.py:950`).

### `lib/aiohttp.py` — 349 lines
Vendored from `micropython-lib/python-ecosys/aiohttp` (MIT, © 2023 Carlos Gil) with **four local modifications documented in the header**: WebSocket support removed; `ClientResponse.readinto()` added; TCP/TLS connection reuse + stale-connection detection added; 204/304/HEAD marked body-consumed so keep-alive survives conditional polls.

`readinto(buf)` (`:63-78`) is the contract `api_client` depends on: requires a non-zero `Content-Length` (raises otherwise — **chunked bodies unsupported on this path**), hard-errors if the body exceeds the buffer (**never truncates**), loops on partial reads, and **returns a slice of the caller's buffer** (`buf[:content_len]`) rather than a count. It bypasses `_decode()`, so a gzip body would land compressed. In Rust this becomes `&[u8]` of length n.

Port-relevant deviations from real aiohttp: headers are a **case-preserving dict** with no case-insensitive lookup; redirects follow only 301-303, max 2 hops (**307/308 not followed**); a stale connection triggers **one non-idempotent request replay**; `ssl=None` on an `https:` URL coerces to MicroPython's default TLS context with **no certificate verification**; `data=` on a GET silently rewrites the method to POST; no timeouts (callers wrap in `asyncio.wait_for`); no cookies/auth/proxies/multipart/form encoding.

### `lib/button.py` — 315 lines
PIO debounced button driver, and the one place firmware timing is *reconstructed* rather than measured.

The PIO program (`:38-163`) keeps a down-counting duration in `x` and a debounce countdown in `y`. An edge is accepted **only if `y` reached 0** — i.e. the *previous* state was held for the full window — so an accepted edge fires on the very sample it's seen (**zero added latency**) while the post-edge bounce tail is rejected. Consequence: a press/release shorter than the window is swallowed and surfaces as a **same-state event pair**; consumers must not assume alternation. `saturating_decrement` (`:103-111`) is a Python-level macro emitting a constant-time `reg = max(reg-1, 0)` with a `nop()` shim so both branches are exactly two instructions — the asymmetry was the original timing bug.

Every loop path is 4 instructions × `[3]` delay = 16 cycles; the FIFO word drops the counter LSB so **1 reported tick = 2 iterations = 32 SM cycles**, and the SM clock is `1000 * 32 // tick_period_ms` (32 kHz at the 1 ms default).

FIFO word layout: **bit 31 = raw pin level** (PIO is polarity-agnostic; `active_low` is applied in Python), **bits 30:0 = duration of the *previous* state in ticks**.

`read()` (`:274-306`) **never reads the clock** — timestamps are reconstructed by advancing a private `_anchor_ms` (seeded from `.initial.ticks_ms`) by each event's `duration_ticks * tick_period_ms`. The PIO counter is the only time source after construction. Rollover markers (after ~24.8 days of one unbroken state) are filtered by the unique signature `duration_ticks == 0 and pressed == last_pressed` — unambiguous because the `debounce_reload >= 2` floor guarantees a real event's duration never decodes to 0. **The anchor still advances for filtered markers** (a no-op, duration 0).

`_NO_EVENTS = ()` is a module-level singleton returned **by identity** on both the fast path and the all-filtered path, so steady-state polling allocates nothing — `tools/pio_sim.py` tests this with `is`, not `==`.

RX FIFO depth is 4 events = 2 full press+release cycles. PIO **blocks** when full (events are never dropped) but blocked time is uncounted, skewing subsequent timestamps — so `read()` must be called at least every ~4× `debounce_ms`. The 50 ms production poll is well inside that.

`tools/pio_sim.py` is a cycle-accurate RP2040-semantics PIO simulator that imports the **real** `button.py` under stub modules and asserts all of the above. It's the natural oracle for a Rust port.

### `lib/rotary_encoder.py` — 216 lines
PIO quadrature decoder using a 16-entry **computed-goto jump table** indexed by `[X1, X0, PinA, PinB]` — the low 2 bits of the position counter double as the previous state, so no separate register is needed. Invalid transitions are ignored. `push(noblock)` means a full FIFO silently drops samples rather than stalling. A **DMA channel continuously drains the RX FIFO into a 1-element `array('i')`** (`count = 0xFFFFFFFF`, no increment either side), so `.raw_value` is a plain array read. `.value` = `(raw - baseline + 2) >> 2` (4 edges/detent, `+2` rounds to nearest). Always uses SM0 (claims the whole PIO block). **Used only by `hardware_diagnostic.py` — confirmed no reference in `main.py` or anywhere under `scoreboard/`.**

### `lib/veml7700.py` — 169 lines
I2C ambient light sensor at `0x10`. `VEML7700(i2c=, it=100, gain=1)` at both call sites. Registers `als_conf_0=0x00`, `als_WH=0x01`, `als_WL=0x02`, `pow_sav=0x03`, `als=0x04` (read). `init()` writes config + zeroed thresholds + zeroed power-save and is **idempotent/re-callable** — both consumers use it as the post-error re-init path.

`read_lux()` is a **plain linear scale**: `raw_u16_le * self.gain`, where `gain` is a resolution factor from a lookup table (`it=100, gain=1` → 0.0288 lux/count). **No auto-ranging, no integration-time delay enforcement** (the caller must space reads at least one integration time apart — documented in the docstring), and **no Vishay high-lux correction polynomial**. The `gainValues` table also deviates from datasheet-derived ideal values in several cells, so a Rust port recomputing from first principles will not match it bit-for-bit — worth pinning to the table if brightness parity matters.

### `lib/miqro/` — QR generator
First-party library (`__version__ = "0.1.0"`, sibling repo on this machine). `qrcode.py` (231 lines) is pure-Python orchestration; `__init__.mpy`, `constants.mpy`, `native/{armv6m,armv7emsp}.mpy` are **opaque precompiled native ARM modules** containing all the QR encoding (Reed-Solomon, masking, version selection). `QRCode(data)` exposes `.data` (MONO_HLSB `FrameBuffer`, directly blittable), `.width`, `.height`, `.version`, `.get(x,y)`, `.packed()`, `.print_ascii()`. Class-level grow-only buffer pool shared across instances. A Rust port replaces this wholesale (`qrcodegen` / `qrcode` crate).

### `lib/pio_types.py` (3 lines) + `.pyi` (1,223 lines)
Pure type-checker shim with **no runtime effect** — `from pio_types import *` imports nothing; the PIO assembler names are injected by `@rp2.asm_pio` at decoration time. The `.pyi` is a hand-written, datasheet-accurate RP2040 PIO ISA reference with a type lattice encoding which operands are legal where. **Worth keeping as reference during a Rust PIO port** — e.g. it documents that `jmp x_dec` decrements unconditionally and branches on the pre-decrement value (`pio_types.pyi:584-598`).

### `lib/microdot.py` — 1,574 lines
Vendored Microdot 2.x (Miguel Grinberg). **One documented modification** (`:8-15`, implemented at `:965`, `:1287-1294`): a per-connection timeout (`Microdot.connection_timeout = 60`). A client that opens a connection but never completes a request (browser speculative preconnect, phone sleeping mid-request) or stops draining the response used to park its handler task forever, permanently pinning one of lwIP's few TCP sockets; leaked sockets accumulated over days until inbound connections were silently dropped while the rest of the firmware stayed healthy.

API surface actually used: `Microdot()`, `.mount(subapp, url_prefix=)`, `.get/.post/.put/.route` (including the `<path:path>` converter), `.start_server(port=)`, `Response.send_file_buffer_size` (**overridden to 2048** at `main.py:242`), `send_file(compressed='gzip')`, tuple return form `(body, status, headers)`, sync-generator streaming responses, `request.json`, `request.args`, `request.headers`, `request.app`, plus ad-hoc attributes on the app object (`app.ap`, `app.wlan`, `app.setup_mode`, `app.setup_reason`, `app.request_ota_check`). Unused and droppable in a port: `run()`, before/after-request hooks, `errorhandler`, `abort`, `redirect`, cookies, `patch`/`delete`, form/multipart, SSL server.

---

## 4. Surprising / nonobvious behaviors — the flag list

**Cross-core split.** Core 0 = asyncio (network, poller, buttons, OTA, brightness, web server, **all state mutation**). Core 1 = render loop only, started via `_thread.start_new_thread` (`main.py:753-776`). The handoff is the triple-buffered mailbox. Core 1's only writes outside its own framebuffer / LoopState / registered scratch are `ThreadHealth.healthy` and `.frame_seq`. The normative statement is the mutation contract at `display.py:1761-1817`, which includes a mechanical audit rule (`ls` must not appear below `render_frame`) plus a preview-side tripwire (scratch poisoning between frames).

**MicroPython-specific contortions — decide per item whether to preserve or drop:**
- `@micropython.viper` on `_dim_frame` (`display.py:124-144`) and `rgb565` (`fonts/__init__.py:107-110`); `@micropython.native` on nearly every `Hub75Driver` method.
- Viper 32-bit mask literals **must** be built through variables or they box to objects (`display.py:129-136`).
- Floats are heap-allocated → `pulse()` is an integer triangle wave replacing `sin()`; `_base_marker_colors` and `_football_top_x` are integer-only.
- `str(value)` avoided on the render path → `FontWriter.integer()` with a 5-slot digit scratch.
- Sequence counters masked to `0x3FFFFFF` so incrementing never promotes to a big int (`state.py:719`, `display.py:1963`).
- `textfold`'s parallel const strings exploit ROMFS `.mpy` string literals being memory-mapped in place (zero heap).
- Module-path imports (`import scoreboard.logger as logger`) to survive partial package init.
- `gc.threshold(48 KB)` tuned against measured allocation churn.

**Timing-sensitive sections:**
- The PIO IRQ handshake, and `deinit()`'s stall-flag polling + IRQ-flag clearing (a missed clear offsets every row by one on the next init).
- The button driver's timestamp reconstruction (PIO counter is the only time source; blocked-FIFO time is uncounted).
- Deadline-based 20 FPS pacing that re-anchors rather than bursting.
- The two-rail model — getting a rail wrong makes the soccer clock drift from reality or makes scrolls jump on GC.
- The scroll-speed / frame-rate divisibility constraint.
- `_kick_ota_check`'s 2 s delay so the HTTP response flushes before the blocking download freezes the loop.
- The OTA countdown's deliberate blocking `time.sleep` so nothing can repaint before `machine.reset()`.

**Traps a naive port would hit:**
- `sys.exit()` from `main.py` soft-reboots and re-runs `main.py` on the rp2 port — both the safe-mode block and the import guard raise instead, with comments recording the failures this caused.
- PIO `SET`'s 5-bit immediate silently truncating `1 << 5`.
- `framebuf.blit()` applying the palette **before** the key comparison, which is why `KEY` is `0xF81F` even for paletted sprites.
- The three MicroPython bit-packing conventions disagreeing on bit order between GS2 and GS4.
- The spinner's palette-index inversion (angular order ≠ compiled first-seen order).
- The football endzone palette indices discovered by color value because art edits reorder them.
- Sprite palettes are shared module state mutated per frame — the `try/finally` restores are load-bearing.
- ETag must be echoed **with quotes**; the header lookup must be case-insensitive.
- The response memoryview aliases a shared buffer — linescore bytes must be `bytes(...)`-copied, and parsers must not await.
- `utc_offset = 0` is a valid sync result and must not be conflated with `None`.
- `/ota_dev` exists specifically to prevent an "update" that is actually a rollback.
- Score-polling is plain HTTP by design (and unauthenticated); only OTA is TLS. Don't "fix" this without re-measuring heap headroom.

**Known-soft areas flagged in-source:**
- The football yardline convention is marked as excavated from a pre-rewrite implementation and **needing live-game re-validation** (BACKLOG).
- `MEM_PROFILE = True` is marked temporary (2026-07-11 GC/stutter investigation).
- The safe-mode and OTA-apply splash screens are disabled pending an early-boot hard fault (BACKLOG 38); `_early_display_show` exists but is uncalled.
- `FontWriter.clock()` exists but is uncalled (reserved for NBA).
- `hub75.effects` is entirely unused by the app.

**Existing parity infrastructure worth reusing rather than rebuilding:**
- `tools/preview/` runs the **real** `scoreboard.display.render_frame` on CPython behind framebuf/hub75/time shims, with golden tests and the scratch-poisoning tripwire. Closest existing analogue to the planned Rust golden-image harness. Its `framebuf_shim.py` is pinned bit-for-bit against `compile_layout.py`'s packers by `tests/test_framebuf_shim.py`.
- `tools/pio_sim.py` — cycle-accurate PIO oracle for the button driver.
- `tools/wire_format_check.py` — cross-implementation golden test running the actual firmware parsers under CPython against a from-spec encoder. **Deleted in Phase 0**: `crates/scoreboard-wire` tests both directions over the same goldens, and `backend/testdata/wire/` holds the corpus bytes it used to recompute.

**One incidental finding, not blocking:** `firmware/src/config.json` is gitignored, so the live WiFi password and API key are not in version control — but they are present in the working tree and on every deployed device. Worth confirming the Rust deploy path keeps that file out of any image that gets published or shared.

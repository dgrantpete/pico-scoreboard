# Pixel parity with the MicroPython firmware

Phase 2's exit criterion, and its evidence. Every committed wire fixture is
rendered by both stacks and compared byte for byte:

```
backend/testdata/wire/**.bin
  ├─ scoreboard-wire → scoreboard-model Store → scoreboard-render → hub75 simulator
  └─ scoreboard/{mlb,nba,football,soccer}.py → state.py → display.render_frame
```

**Result: 180 of 180 frames MATCH. 0 ACCEPTED-DIFF, 0 FAIL.**

| | |
| --- | --- |
| Generator | `crates/scoreboard-render/tests/gen_parity.py` |
| Test | `crates/scoreboard-render/tests/parity_frames.rs` |
| Goldens | `crates/scoreboard-render/tests/parity/` (manifest, crest pool, 180 raw RGB565 frames) |
| Diff artifacts | `target/parity-diffs/<case>__t<ms>.png`, written only on a mismatch |

```
py crates/scoreboard-render/tests/gen_parity.py     # regenerate the baseline
cargo test -p scoreboard-render --test parity_frames -- --nocapture
```

## What is being compared

The MicroPython side is the **real shipping firmware**, run on CPython under
`tools/preview`'s shims — the wire parsers, `poller`'s own commit functions
(including the shared play-flash staging), `state.py`'s setters, and
`display.render_frame` behind the preview's scratch poisoning. Nothing is
transcribed or reimplemented; the frames in `tests/parity/frames/` are the
bytes `display.render_frame` produced.

45 cases × 4 time points:

| Group | Cases | Source |
| --- | --- | --- |
| MLB | 4 | `backend/testdata/wire/mlb/` |
| NBA | 7 | `backend/testdata/wire/nba/` |
| Football (NFL + college) | 9 | `backend/testdata/wire/football/` |
| Soccer (`fifa.world`) | 13 | `backend/testdata/wire/soccer/` |
| Static screens | 12 | published by hand — see below |

## How time is pinned

Three clocks have to be fixed or the two stacks are not comparable at all.

* **The commit instant.** Every setter stamps `time.ticks_ms()` into what it
  publishes — `animation_start_ms`, `play.updated_ms`, the soccer clock anchor.
  The virtual clock is parked at 100 000 ms for the whole commit, and the Rust
  side passes that same value as `now_ms`.
* **The two rails.** Each frame is rendered at wall `100 000 + t` with both
  frame rails equal to `t` — the values `FrameRail::advance_and_latch` produces
  under ideal pacing, `t` ms after a commit that changed the view identity.
  Pinning them directly, rather than stepping a loop, is what makes one frame
  reproducible from `t` alone on both sides.
* **Today's date.** `set_pregame` compares the game's local day against
  `time.time()`'s, so an unpinned run would emit a different date line
  tomorrow. `time.time()` is pinned to 2026-03-01 UTC, which the Rust side
  receives as `LocalClock::now_epoch_s`; the UTC offset is US Eastern standard.
  The pinned day is one no fixture kicks off on, so every pregame card renders
  its date phase.

`t ∈ {0, 1500, 4500, 11000}`, chosen to land in different phases of the
animations the screens run:

| t | what it lands in |
| --- | --- |
| 0 | every animation at its start; the opening scroll pause |
| 1500 | past the 1 000 ms play/pregame pause, mid-scroll; soccer clock +1 min |
| 4500 | past the 4 000 ms pregame dwell floor (phase 2) and the 1 800 ms line-score pause |
| 11000 | deep into every scroll, and past the shorter play windows, so the bottom strip falls back from the flash to the sport's own content |

23 of the 45 cases render differently at different time points. That is checked,
not assumed: the test counts them and fails if the number reaches zero, because
a corpus where nothing moved would read as green while every scroll, cycle and
pulse went unverified.

Everything else that could differ is **emitted into the manifest rather than
agreed by hand**: the UI palette comes from `config.Config()`'s defaults, the
layout variants and scroll speed from `screen_geometry`'s module state, and the
crest pixels from `LogoProvider`'s placeholder builder. A change to any of them
moves both sides on the next regeneration instead of silently disagreeing.

### The 60 FPS move did not disturb any of this

Task #17 (2026-08-08) took the Rust render loop to 60 FPS while the MicroPython
firmware it is compared against stays at 20, and **all 180 frames still match
byte for byte with no golden regenerated**. That is not luck, and it is worth
writing down because the obvious expectation is a re-bless:

* The pinned points are **absolute positions on the frame rail**, not frame
  indices. Every renderer takes milliseconds, so `t = 1500` means the same
  picture at any rate — the rate only decides how many frames the loop draws on
  the way there.
* All four points are frame boundaries at 60 FPS as well as at 20 (`1500 ×
  60/1000 = 90` frames exactly, and likewise for 4500 and 11000), so each is a
  picture the loop genuinely passes through rather than one between two frames.
  `tests/time.rs::the_parity_harness_offsets_are_positions_the_rail_actually_visits`
  is that check, so a future rate that broke it would fail rather than quietly
  compare against unreachable frames.
* The manifest pins 20 px/s, which is still legal at 60 FPS (one pixel every
  three frames rather than one per frame). Had it pinned the 40 px/s the config
  used to accept, this section would read very differently: the harness asserts
  the Rust ladder accepts the speed `screen_geometry` runs at, so it would have
  failed loudly rather than drifted.

The claim the harness makes is therefore unchanged and was not weakened: it
still compares pixels, not timing. The one place where 60 FPS *could* have
produced a real divergence is the toast dim ladder, and it did not, because that
ladder is wall-timed rather than one rung per frame — making it finer is
possible now and would be a genuine divergence, which is why it is deferred to
BACKLOG 83 rather than folded in here.

## Static screens

The screens no wire payload reaches are published by hand through the same
setters on both sides. The arguments *are* the fixture, so the two tables
(`gen_parity.py`'s `static_screens`, `parity_frames.rs`'s `publish_screen`)
must agree — the manifest names every case and the test panics on one it does
not know, so a screen added to one side alone fails rather than being skipped.

| Screen | Covered | Note |
| --- | --- | --- |
| `idle`, `no_games` | yes | |
| `startup` | yes | step 2/5 with retry attempt dots |
| `error` | yes | title + two lines |
| `updating_progress`, `updating_countdown` | yes | |
| `setup_no_config`, `setup_bad_auth` | partial | **no QR** — see below |
| `toast_text` | yes | over a live MLB game: exercises the bottom-strip priority (toast displaces the play flash) and the expiry at t=1500 |
| `toast_lock` | yes | sticky icon overlay + whole-frame dim, over a live game |
| `toast_spinner` | yes | sticky; the 1 000 ms rotation lands differently at all four time points |
| `menu` | yes | five rows, checkboxes, scrollbar thumb, and a label too wide for the 112 px row so the marquee actually moves |

The toast overlays are staged over a live MLB screen rather than over idle
because **`render_idle` draws no toast at all** — toast drawing lives inside the
mode renderers. Staged over idle they rendered a plain idle screen on both
stacks and proved nothing; that was caught by checking that the overlay frames
differed from the base screen's, not by the comparison, which was happily green.

### The setup QR is out of scope, deliberately

`set_setup_mode` is called without an AP SSID, which is the branch that skips QR
generation on both stacks. With an SSID the MicroPython path generates its QR
through `miqro`, which ships as a precompiled `.mpy` that CPython cannot import
— `tools/preview` has always rendered that screen QR-less. Comparing a screen
neither stack draws the way the device does would be worse than not comparing
it. The Rust encoder is separately pinned against the independent `qrcode`
package in `tests/qr.rs`, so what is unverified is narrow: the QR's *placement*
on the setup screen and `Regions.update_for_qr`'s line narrowing around it.

## Accepted-diff classes

Two items were pre-approved for review rather than chasing. **Neither costs the
corpus anything**, so the verdict above has no ACCEPTED-DIFF rows: the
brightening class is latent and does not occur, and the red-card fixture turned
out to be fixable rather than a diff to accept.

### Team-color brightening — latent, does not occur

`state._team_color_to_rgb565` computes the brightening scale in floating point
and truncates; `Rgb888::brightened` computes `channel * 128 / max` in integers.
They disagree by one where that product is exactly integral, because the float
form truncates 127.999….

The corpus *does* exercise brightening — 11 team-color instances have a
brightest channel below 128, including SF's pure black, which becomes mid gray.
None of them diverges. A sweep of the whole sub-128 space shows why: the two
forms differ on exactly **five** channel/maximum pairs, needing a brightest
channel of 49, 98, 103 or 107. No team in the corpus has one.

Where it does occur, the port is the correct arm: the float form lands on 127
and undershoots the `_TEAM_COLOR_MIN_CHANNEL = 128` floor the function exists to
enforce.

The classifier recognises the class **structurally** — every differing pixel
within one channel code on all three channels — never by fixture name, so a
genuine bug cannot ride along inside an accepted one. Because the class never
fires on the corpus, its recogniser would otherwise be code nobody runs; it has
its own unit test (`the_accepted_diff_class_is_recognised_by_shape_and_nothing_else_is`)
feeding it both a real brightening pair and a two-code difference that must be
rejected.

### `soccer/fifa.world/live_red_card` — was a fixture gap, now fixed

The fixture used to encode byte-identically to `overtime.bin`, and the standing
diagnosis was "the corpus contains no red card". That was wrong in an
instructive way. The fixture is a real ARG-SUI knockout match that *does* carry
a 72' red card for B. Embolo — but it ran on to 120'+4', and
`soccer::transform::last_event` surfaces only the latest goal-or-card. Three
later goals buried the card, so the wire carried a goal, both stacks rendered a
goal, and the red-card path was untested at every layer while the case looked
covered.

The fixture now stops just after the 72' card, which is the moment its name
always claimed and exactly what the backend's own unit test had been
constructing in memory. It renders `RED CARD 72'` over `B. Embolo` in the
carded side's colour, at 1-1 in the second half — a screen genuinely distinct
from `overtime`'s. `tools/extract_fixtures.py`'s selector was the root cause and
now mirrors the same max-by-clock rule, so a re-capture cannot reintroduce a
shadowed card.

## The bug this found

`geometry::football_top_x` truncated toward zero where `state._football_top_x`
rounds half away from zero. The scrimmage and first-down perspective lines, the
ball sprite and the possession arrow all hang off that projection, so all four
sat one pixel off — on **49 of the 100 yard positions**. Only
`football/nfl/end_of_period.bin` happened to land on one of them, which is a
good illustration of why a corpus beats a spot check: a single fixture, at all
four time points, was the whole signal.

The MicroPython side is the parity baseline, but this was not copied blindly —
rounding to nearest is what the field sprite's own perspective was drawn to, and
truncating is a straightforward port error. Fixed in `football_top_x`.

## Sensitivity

The harness has been shown to fail, not just to pass:

* The football bug above surfaced as 4 FAILs on the first run, with a diff
  artifact that showed the one-pixel offset directly.
* Temporarily changing `PLAY_SCROLL_PAUSE_MS` from 1000 to 900 produced 24
  FAILs — **none of them at t=0**. That is the argument for the non-zero time
  points: a first-frame-only harness would have passed that mutation.

## Regenerating

The goldens are committed. Re-run the generator after any change to the
MicroPython render path, the fixtures, the fonts or the sprites, and review what
moved before committing:

```
py tools/compile_layout.py && py tools/compile_fonts.py   # if art/fonts changed
py crates/scoreboard-render/tests/gen_parity.py
```

The generator is deterministic — two consecutive runs produce byte-identical
frames — so a diff in `git status` after regenerating means the MicroPython
output genuinely changed.

---

# HTTP surface parity — `api_routes.py` and `main.py`'s handlers

Phase 3, task #10. Every endpoint in INVENTORY's `api_routes.py` table, plus
`main.py`'s `GET /` and `/<path:path>` catch-all, served by picoserve. Validated
on hardware against the real device at `192.168.50.236`; the transcripts are in
the task report.

| Method | Path | Status | Notes |
|---|---|---|---|
| GET | `/api/config` | **Match** | The whole merged configuration, same key names and nesting. 977 B on the wire with the default config. |
| PUT | `/api/config` | **Match**, 2 deviations | Merge, cadence check against the *merged* pair, live-apply, echo the config back. `400 {"error":"invalid_cadence"}` verbatim. Deviations below. |
| GET | `/api/status` | **Match**, memory fields redefined | Three shapes (`ap`/`station`/`unknown`), every field present in all three, `configured_ssid` only for the two failure reasons. The four memory keys mean something different — see below. |
| GET | `/api/logs?since=` | **Match** | NDJSON, one `[seq, ts, level, msg]` per line, tail-follow by last seq. Chunked rather than a generator; same bytes. |
| GET | `/api/logs/previous` | **Seam** | Always `404`. There is no previous-boot record until task #12 writes the panic breadcrumb; MicroPython's file-missing branch produced the same status and body shape, so the SPA already handles it. |
| POST | `/api/check-update` | **Match** | `501 {"status":"unsupported"}` — precisely the answer `api_routes.py` gave when the OTA attribute seam was absent. Task #15 gives it a real one. |
| POST | `/api/reboot` | **Match** | Responds, then resets after 1 s. Measured recovery: **10 s** from request to serving again. |
| POST | `/api/reset-network` | **Match** | Clears the credentials, leaves the live link up — deliberately, as `api_routes.py` did, so the response reaches the browser. |
| GET | `/` | **Match** | The gzipped SPA with `ETag`, `Cache-Control` and `Content-Encoding: gzip`; `304` on a matching `If-None-Match`. |
| ANY | unknown path | **Match**, bug fixed | Ours → `404`, foreign → `302` in setup mode. See below. |

## Deviations, all deliberate

**1. A wrongly-typed value is rejected instead of stored.** `update_many` wrote
whatever it was given and left the defensive accessors to cope, so
`{"display": {"brightness": "bright"}}` was stored and read back as garbage.
Here it is `400 {"error":"invalid_json"}`. Rejecting at the boundary is the
better answer and the SPA never sends one.

**2. Unmodelled `display.variants` keys do not round-trip.** MicroPython's
variants dict was free-form, so an unknown key was stored and echoed back
(while still selecting nothing, because `screen_geometry.set_variants` ignored
it). Only the four keys `_DEFAULTS` carries are modelled here, and an unknown
one is dropped rather than echoed. Nothing reads them, and both firmwares
render identically.

**3. The ETag is quoted, and `If-None-Match` accepts more forms.** `main.py`
sent `ETag: 1a2b3c4d5e6f7788` — a bare hex string, which RFC 9110 §8.8.3 does
not permit — and compared the header with `==`. This sends `"1a2b…"` and
accepts quoted, unquoted, weak (`W/"…"`), comma-separated lists and `*`. The
unquoted form is accepted precisely so a client that cached under the
MicroPython firmware still validates. Host-tested in
`scoreboard-portal::conditional`.

**4. `/api/status`'s four memory fields answer a different question.** There is
no allocator and no filesystem, so `gc.mem_alloc()` and `os.statvfs` have no
counterpart. `memory_used`/`memory_free` are now statically-allocated RAM and
the remainder the stacks grow into; `flash_used`/`flash_free` are the image and
its partition's remainder. All four are *constants* per image rather than the
sawtooth MicroPython's were, which is the point — the sawtooth measured
garbage, not need. Six new fields report what can actually exhaust here:
`core0_stack_used`/`_total`, `core1_stack_used`/`_total` (high-water marks, not
instantaneous depths), and `log_entries`/`log_latest_seq`. Reasoning in
`http::status`'s module docs.

**5. Log timestamps are boot-relative until a clock exists.** No RTC and no
time sync yet (task #11), so `ts` is seconds since boot. The SPA already renders
anything below 1e8 as `+Ns`, because MicroPython's `time.time()` before an RTC
sync was equally fictional — so this needs no client change, and none was made.

**6. The station-mode redirect bug is fixed** (inherited from task #9's
`MyHosts`). MicroPython built its host set from the AP interface, which does not
exist in station mode, so a request for an unknown path on a joined network was
answered with `302` to `192.168.4.1` — an address that is not up, on a network
the client is not on. Bench-validated: `GET /nope` with a forged
`Host: captive.apple.com` returns `404` on a station-mode device, and `GET /`
returns the SPA.

## Not yet bench-validated

**The AP-mode `302`.** `MyHosts::captive()` is false in station mode by design,
so the redirect cannot fire on a device joined to the LAN — the `404` above *is*
the correct station-mode answer. Reaching the redirect needs the test host
associated to the device's own setup AP, which was not done here because it
would take the developer's machine off its network. The decision is host-tested
(`scoreboard-portal::hosts`, `captive_probes_are_foreign`) and the response
construction is nine lines in `http::routes::Redirect`; it should be exercised
once during Phase 3's soak, from a phone, which is the client it exists for.

## Not carried over

`Response.send_file_buffer_size = 2048` (`main.py:242`) has no counterpart:
picoserve writes the body straight out of `.rodata` through the TCP send buffer,
so there is no intermediate copy to size.

---

# Backend client parity — `api_client.py`, `poller.py`, `LogoPool`, time sync

Phase 3, task #11. The whole poll pipeline, from the socket to a committed
snapshot. Bench-validated against the **production backend** from the device at
`192.168.50.236` on 2026-08-08, on real August MLB data; transcripts in the task
report. The pure half — the error mapping, the failure streak, the skip machine,
the buffer sizing — is host-tested in `scoreboard-model::poll`.

## `api_client.py`

| Behaviour | Status | Notes |
|---|---|---|
| Scheme downgrade `https://` → `http://`, once, on the leading scheme | **Match** | Bench-validated by restoring `api.url` as `https://…fly.dev/` mid-run and watching the next poll succeed — the migration case a stored MicroPython config presents. |
| No API key on score routes | **Match** | Unauthenticated backend-side; a cleartext key would leak. `config.api.key` stays in the configuration for OTA. |
| Single pre-allocated receive buffer, 4,096 B | **Match**, derivation corrected | The constant is right; `api_client.py:22-27`'s reasoning sized it against the logo. The binding case is a games list — see BUDGET.md. |
| Response is an aliasing view, valid only until the next request | **Match**, enforced rather than documented | The response borrows the caller's buffer, so using it after the next request does not compile. `api_client.py` stated the rule in a docstring. |
| `_request_in_flight` → `RuntimeError` on concurrent use | **Unreachable** | Every request takes `&mut self` and the client has one owner. The runtime guard has no counterpart because the condition cannot arise. |
| 15 s timeout on every request | **Match** | `embassy_time::with_timeout` around the whole request. |
| Session closed on timeout | **Match**, by construction | The connection lives inside the timed-out future; dropping the future drops it, and `TcpConnection::drop` closes the socket. |
| Persistent `ClientSession` across polls | **Deviation** | One connection per request. See below. |
| `Accept: application/x-scoreboard-struct` | **Match** | |
| ETag scanned case-insensitively | **Match** | The header's case is the server's choice; the deployed backend sends lowercase `etag`. |
| ETag stored and echoed **with its quotes** | **Match** | The backend compares strings. Bench-validated: `"0b81674f50a5ebf9"` echoed as `If-None-Match` returned `304`. |
| `304` returns without reading a body | **Match** | And it has to, harder than in MicroPython — see SPEC §7.4's fourth bullet. |
| Detail `404` → `None`; other 4xx/5xx → error | **Match** | A `404` on a *games list* is still an error: a configured league that does not exist is a configuration to fix. |
| Error bodies are JSON regardless of `Accept`; the `error` field, defaulting to `unknown_error` | **Match** | Host-tested. |
| `_log_api` per request at DEBUG | **Match** | Path, status and elapsed time, to defmt *and* the ring — `logger.debug` reached the log file, so `/api/logs` is where it belongs. The URL is trimmed to its path, because the ring's 128-byte message would otherwise spend 96 of them repeating the host and truncate away the status. |

## `poller.py`

| Behaviour | Status | Notes |
|---|---|---|
| One poller owns every league, slates merged into one rotation | **Match** | |
| `sources_from_config` order: MLB, NBA, football, soccer | **Match** | |
| Live-first rotation; finals before pregames; empty merged slate → `no_games` | **Match** | `Slate`, host-tested. |
| League filter, its empty-slate fallback, and the index restore | **Match** | `Slate`, host-tested. |
| Per-source ETag; `304` keeps the cached slate | **Match** | Bench-validated. |
| One source failing keeps its cached slate; only an all-source failure fails the tick | **Match** | Bench-validated: `baseball/mlb list refresh failed, keeping cached slate`. |
| `MAX_FAILURES = 5` → `set_error("API ERROR", …)` | **Match** | Bench-validated: five timeouts, then `Mode::Error` on the panel. |
| `_friendly_error`'s four arms and the two 25-character detail lines | **Match** | Host-tested arm for arm. The `OSError` arm says what failed in words rather than an errno — see below. |
| `failing for {n}m`, recomputed on every failure past the fifth | **Match** | |
| Recovery logged at ERROR with the streak length | **Match** | Bench-validated: `recovered after 7 failed polls`. |
| **No exponential backoff**; the sleep is always `poll_interval_seconds` | **Match** | |
| Sleep interrupted by a wake | **Match** | An `embassy_sync` channel rather than an `Event`; see `poller`'s module docs. |
| Skip machine: armed/rejected, one in flight, sticky spinner, `finally` teardown on every path | **Match** | Host-tested. The *sender* is task #12's — the button loop does not exist yet. |
| `skip_league` stays within the league filter | **Match** | `Slate::advance_league`, host-tested. |
| `_poll_current` re-fetches every tick, including static screens | **Match** | Bench-validated: three detail fetches per rotation at `poll_interval=5`, `game_rotation=15`. |
| Detail `404` skips the slot; the next rotation refreshes the list | **Match** | |
| `_flash_play`: one slot for every sport, previous id carried in the snapshot | **Match** | `Store::commit_detail`, host-tested. |
| Soccer stale-clock guard | **Match** | `Store`, host-tested. |
| `no_games` committed every tick | **Deviation**, trivially | Committed on the transition only: it is a static mode, so an unconditional commit wakes core 1 out of its skip once a poll interval to draw the identical frame. |
| A partial list decode leaves the source short rather than keeping its cached slate | **Accepted** | `Slate::update_source`'s docs record it: the transport failures that dominate never get that far, and the next refresh is unconditional because the ETag is dropped. |

## `display.py`'s `LogoPool`

| Behaviour | Status | Notes |
|---|---|---|
| 8 slots, 24×24 RGB565, LRU | **Match** | Bench-validated, including eviction once all eight filled. |
| `Accept: image/x-rgb565`, `width=24&height=24&background_color=000000` | **Match** | |
| Keys league-namespaced and lower-cased | **Match** | `baseball/mlb/nym`. |
| One sequential caller | **Match**, enforced | `&mut self`. |
| Cache re-check after the fetch | **Unreachable** | It guarded against a second caller filling the same key mid-`await`; there is one caller and it holds `&mut self`. |
| Non-200 → `None`, nothing cached, not a poll failure | **Match** | Improved: the slot is never written on a failure, so a failed fetch cannot leave a torn crest behind. |
| Core 0 fills buffers core 1 draws from | **Deviation** | The pixels moved to core 1. See below. |
| Evicting a slot the displayed state references | **Fixed** | Held slots are never chosen as victims. |

## Time sync (`main.py:453-488`)

| Behaviour | Status | Notes |
|---|---|---|
| `GET {api_url}/time` under a 15 s cap | **Match** | Bench-validated: `GET /time -> 200 in 662 ms`, `utc offset -21600 s`. |
| `utc_offset` absent or `null` reads as `0` | **Match** | |
| **`None` ≠ `Some(0)`** — an unsynced device omits start times entirely | **Match** | Two values, not one: `local_clock()` returns `utc_offset_s: None` until the first success, and the model's pregame builder then skips both the time and the date line. |
| `machine.RTC()` set from the reply, RTC stays UTC | **Replaced** | There is no RTC (SPEC §7.4). The epoch is anchored against `embassy_time::Instant`, the clock every other deadline already rides. `ringlog::set_wall_clock` makes the log's timestamps real from that moment — bench-validated, `/api/logs` entries carry unix seconds. |
| Synced once, in the boot sequence, before services start | **Deviation** | Daily, and as the poll loop's first phase. See below. |

## Deviations, all deliberate

**1. One connection per request, not a persistent session.** `aiohttp`'s
`ClientSession` held one open across polls. The default poll interval is 30 s,
which is past the idle timeout of every proxy between the device and fly.io — the
persistent session was reconnecting on most polls anyway, it just could not say
so. Opening per request makes "close the connection on timeout" the default
rather than an `except` arm, and frees the socket between polls. Measured cost:
~640 ms on a cold connection against ~130 ms on a warm one, against a tick that
makes at most four requests every 30 s.

**2. `api.url` is re-read every tick.** `ScoreboardApiClient.__init__` computed
the base URL once, so changing the backend on the settings page did nothing until
a reboot — with no indication that it had not worked. The URL is read from the
running configuration each tick now, which is also what makes the failure-path
bench possible without a reset.

**3. Time sync re-runs daily, and does not block the boot.** `main.py` synced
once and never again, so a device up for a month drifted by whatever its crystal
drifted by. It also awaited the sync before starting any service. Here it is the
poll loop's first phase — so it still runs before anything commits — but the HTTP
server and the render loop are already up, and a device whose backend is
unreachable reaches its setup page instead of sitting on the startup screen.

**4. The crest pool is split across the cores.** `LogoPool` let Core 0 write
buffers Core 1 was drawing from. That is a cosmetic race in Python and undefined
behaviour in Rust; the pixels moved to core 1 and only the key/LRU bookkeeping
stayed on core 0. Costs 2,337 B and one 1,152 B copy per crest fetched. BUDGET.md
prices the three alternatives.

**5. `_friendly_error`'s `OSError` arm says what failed, not which errno.**
`str(OSError)` gave `[Errno 113] EHOSTUNREACH`, of which four characters were
useful on a 25-character line. The transport failures are enumerated instead —
`cannot resolve backend`, `cannot reach backend`, `connection lost`, `bad http
response`, `response too large` — which is the same information in words the
owner of a scoreboard can act on.

**6. A command reaches the poller between ticks, not at the instant it is
sent.** `skip()` set the spinner toast at press time; here the press is a message
the poller applies when its in-flight request finishes. Bench-measured that is
60–300 ms, and the 15 s request timeout is its ceiling. The alternative is a
second writer to the display state, which is the thing the design buys. Task #12
owns the button loop and can decide whether the gap is worth closing.

**7. One publish per commit, not two.** `set_mlb_live` committed and then
`_flash_play` committed again, microseconds apart, so core 1 could latch the
intermediate state. `Store::commit_detail` does both mutations and the poller
publishes once. The snapshot channel still needs its three slots — a toast and a
commit inside one frame still do it.

**8. UI colours are nudged, not waited for.** `update_ui_colors` wrote into a
module the renderers read directly, so a colour change appeared on the next
frame. Here colours ride *in* the snapshot, so they only move when something
commits — up to a poll interval away, on a screen the render loop is skipping.
`PUT /api/config` therefore sends the poller a `ColorsChanged` command.
Bench-validated: applied within the `PUT`'s own round trip, against the 30 s it
would otherwise have taken.

## Not yet bench-validated

**Every sport but MLB.** August has MLB and preseason college football and
nothing else, so the bench ran a single-source slate. The multi-league paths —
the merged rotation's league ordering, `skip_league`, the filter — are host-tested
against the corpus in `scoreboard-model`, and the wire decoders are pixel-parity
verified for all four sports (above), so what is unexercised on hardware is the
*fan-out*: N list refreshes in one tick, and the crest pool under two leagues'
worth of teams. Worth a deliberate run in task #13's soak with football and
soccer enabled, once their seasons overlap.

**The skip machine on real buttons.** Host-tested exhaustively, and the burst
rule now has an integration test of its own —
`crates/scoreboard-input/tests/burst.rs` folds eight taps through the tracker,
the menu controller and the skip machine and asserts one advance. **Still
unexercised on real hardware**: the bench unit has no buttons attached. Task #13
needs a unit with buttons wired, or this path ships untested against a physical
switch's bounce.

---

# Storage, inputs, brightness and supervision (task #12)

## Deviations, all deliberate

**1. Two storage keys, not four.** SPEC §9 listed wifi credentials, device
config, an OTA channel/dev flag and sticky user prefs. Only two of those are
records. The credentials are the config document's `network` section, because
that is where `config.json` had them and where `GET /api/config` returns them
from; splitting them would give the same four fields two owners and make
`reset-network` clear both. There are no sticky prefs — the rotation lock and
the league filter are deliberately session state, which `menu.py` states
outright, and brightness is `display.brightness`. The OTA dev flag is Phase 4's
and SPEC §8 already says it becomes a config field, so reserving a flash key for
it now would be guessing at Phase 4's shape in a storage format.

**2. `dev.toml` seeds only when no document exists at all.** The bench seam
survives storage, with a precedence rule chosen against the tempting one.
Field-level fallback — "fill in anything empty" — would undo
`POST /api/reset-network` at the next boot, because that route's whole job is to
leave the SSID empty. Document-level is also what makes "delete `dev.toml`,
rebuild, reboot" a real test that storage alone brings the device up; both
halves are bench-validated below.

**3. The configuration is stored as JSON, not as a packed struct.** It is
larger — 942 B for the bench unit's config — and it is the reason a document
written by a firmware with one fewer key still reads correctly, because
`DeviceConfig`'s serde defaults are the same deep merge `config.py` performed by
hand. A packed struct would make every added config field a storage migration.

**4. `storage`'s API is blocking, and says so.** `sequential-storage`'s map is
async, and it would have been one line to write `.await` at every call site.
That would be a lie: the futures underneath are `BlockingAsync` and never return
`Pending`, and the operation parks core 1 for its whole duration. An `async fn`
tells the reader that other tasks run meanwhile. They do not.

**5. The panic handler writes RAM, not flash.** SPEC §9 said flash; it cannot be
done. See that section — a write from core 1 is refused and a write from core 0
while core 1 has panicked hangs forever, which is the crash most worth
recording. The record goes to a `.uninit` cell and the next boot promotes it,
before core 1 starts, where the write is free.

**6. `/api/logs/previous` says "last abnormal shutdown", not "previous boot".**
MicroPython rotated the whole ring at every boot, so the file always described
the immediately preceding session. One breadcrumb instead describes the most
recent abnormal shutdown and survives however many clean boots follow — which is
the more useful of the two, and means a device that has never crashed answers
`404` forever. The rendered text carries uptime and wall-clock time so a reader
can tell which boot it was. The endpoint's name is the SPA's and is unchanged.

**7. The sensor logs transitions only, including its retries.**
`LightSensor._try_init` logged an error on *every* attempt, and it retries every
15 ticks — a line every three seconds, twenty a minute, which evicts the
200-slot ring in ten minutes and takes the history worth reading with it. On the
bench unit, which has no sensor at all, that is the difference between one line
and 1,500. First failure logged, recovery logged, the thousands between silent.

**8. A failed read drops the sensor handle and re-runs `init`.** `main.py` kept
the object and retried only the read. Re-initialising is four register writes on
a path that already runs once every three seconds, and it is what recovers a
part that browned out — strictly more recovery for no measurable cost.

**9. Button init cannot fail, so the non-fatal guard is gone.** `init_buttons`
caught everything and returned `(None, None)`. Here `Pio::new` takes the block
by ownership rather than looking one up, and the program's fit in instruction
memory is a compile-time fact asserted by a test. What remains — and is
supported, and is what the bench unit is — is a device with **no buttons
physically attached**: the pins idle high, the state machines run, nothing is
pushed, and nothing logs, because nothing is wrong.

**10. Presses reach the poller between ticks.** The input task decodes and
forwards; the poller owns the `Store`, the `Slate` and the skip machine, so the
arm/reject decision stays where `poller.py` made it. The cost is that a press
lands between ticks rather than the instant it arrives. Almost always the poller
is asleep on the poll interval and the command wakes it immediately; when it is
inside a request the press waits for that request — bench-measured at
130–800 ms per request, bounded by the 15 s request timeout. MicroPython's
button loop ran concurrently with the poller and had no such wait. **The menu is
the case that would have been visibly worse**, because its 10 s inactivity
timeout would only have been checked when the poller woke, up to a poll interval
— 30 s by default — later. So the controller publishes a deadline and the
poller's sleep is capped by it.

**11. The watchdog's health gate is mode-aware.** In setup mode there is no
poller by design, so its health reads as "never answered" forever; gating on it
would reset the device every eight seconds while somebody was typing their
Wi-Fi password into the settings page. Only the frame counter applies there.
*(The network half of this gate was revised by BACKLOG 70 — see the section at
the end of this file. The mode-awareness described here is unchanged.)*

**12. `SCB::sys_reset()` is not used anywhere.** It does not reset an RP2350 —
see SPEC §12. Every reset goes through the watchdog's `TRIGGER` bit.

**13. The rotary encoder is not ported, and that is not an omission.** It is
wired to GPIO 2/3/4 on the board, `lib/rotary_encoder.py` is a genuinely nice
PIO quadrature decoder, and **`main.py` never imports it** — INVENTORY confirms
its only consumer is `hardware_diagnostic.py`, where it drives a 0–100
brightness preference for bring-up. Parity is against the shipping firmware, so
porting it would mean adding a feature the product does not have under cover of
a parity release. SPEC §12 already routes the diagnostic tool to a
`--features diag` build; the encoder belongs there, with it. Nothing in
`scoreboard-input` or `inputs.rs` reserves PIO or pins for it, so adding it
later costs a state machine and nothing else.

**14. `ThreadHealth.healthy` has no counterpart, because it has nothing to
describe.** MicroPython's feeder watched two core-1 signals: `frame_seq` for a
hung thread, and a `healthy` boolean that the render thread's `except` handler
cleared for a crashed one. That second state existed because the render loop
caught its own exceptions and could die while the rest of the firmware kept
running. Core 1 has no such handler here — a panic reaches the panic handler,
which stashes a breadcrumb and resets the chip — so there is no window in which
core 1 is dead and something is left alive to set a flag. The replacement is
strictly more informative: the crash is *reported* at `/api/logs/previous`
rather than inferred from a counter that stopped.

## Bench-validated

Bench unit at 192.168.50.236, 2026-08-08, release image. **No light sensor and
no buttons are physically attached**, which is itself two of the tests.

**Absent hardware.** `veml7700 init failed (no acknowledge); assuming a bright
room, retrying every 3000 ms` — logged **once** across a 75 s capture, never
repeated. The panel sat at the preference-derived level rather than the floor.
Buttons initialised, and across every capture no spurious event was ever decoded
from either idle pin.

**Config persistence.** `PUT /api/config` → `storage: configuration saved,
942 B` → hardware reset → `config: loaded from storage`, brightness 42 and
rotation 45 s intact, and the device rejoined Wi-Fi from the stored credentials.
Then `dev.toml` deleted, image rebuilt (every `DEV_*` empty), reflashed: still
`config: loaded from storage`, still joined. Storage alone brings the device up.

**Reset to setup and back.** `POST /api/reset-network` → `storage: configuration
saved, 919 B` → reboot → `wifi: no ssid configured, going straight to setup
mode`, AP up on 192.168.4.1 with the captive DNS and DHCP servers, and the rest
of the document intact (brightness still 42) — a targeted clear, not a wipe. The
input task was **not** started, matching `main.py`'s station-only task table.
*The re-provisioning `PUT` was not sent over the AP*: this host has one Wi-Fi
adapter and it is its only network link, so joining the device's AP would have
taken the machine off the network. Recovery was `probe-rs erase` plus a reflash
with `dev.toml` restored, which exercises the same seam from the other side.
What is therefore untested on hardware is the AP-side transport, which tasks #9
and #10 already validated and which this task did not touch; what *is* tested is
the half this task added, that the write persists.

**The watchdog drill (BACKLOG 69).** Watchdog enabled, `api.url` pointed at a
closed port. `watchdog: armed, timeout 8000 ms, feeding every 2000 ms` → polls
time out, streak climbs → at 91 s of uptime, over three 30 s poll intervals,
`watchdog: starving on purpose (backend unreachable); hardware reset within
8000 ms` → the chip reset → the next boot served:

```
last abnormal shutdown: watchdog starved on core 0
uptime: 91 s
stack high-water: core 0 18036 of 266536 B, core 1 3348 of 8192 B

watchdog starved: no successful poll in 91 s, over three poll intervals
```

Note the absent `unix time` line: the backend was unreachable, so the clock
never synced, and the record says nothing rather than claiming the epoch. It
also proves the `.uninit` cell survives a **watchdog** reset, not just a
software one. The config was restored inside the reboot cycle's window and the
poll loop recovered to `poll: lists refreshed, sources 1, rotation 15`.

**The panic drill.** `POST /api/induce-panic` (feature-gated) → the device reset
itself and was answering again before the next 2 s poll → `/api/logs/previous`:

```
last abnormal shutdown: panic on core 0
uptime: 7 s
unix time: 1786187303
stack high-water: core 0 0 of 0 B, core 1 0 of 0 B

panicked at src\http\routes.rs:61: induced by POST /api/induce-panic
```

The zeroed watermarks are the design working: the device died at 7 s, before the
10 s scan that publishes them, so it reports nothing rather than a stale guess.

**The frame hitch.** One 942 B save takes its frame to 14,544 µs against a 50 ms
budget and drops nothing. Full table and derivation in BUDGET.md — including
what the same 14.5 ms means against the 16.67 ms budget the loop now paces at,
which is the one place 60 FPS narrowed a margin that mattered.

**Core 1 held 20.0 FPS throughout** — every `supervise::liveness` line in every
capture above reads `200 ticks in 10 s (20 FPS)`, including the reports either
side of the flash write and through the watchdog drill. These captures predate
task #17; the same line reads 600 ticks per 10 s on the current firmware, and
re-taking them is on the drill-day list rather than done.

## Not yet bench-validated

**Physical buttons and the league menu.** Nothing is attached to GPIO 10 or 22
on the bench unit, so the debounce program has never seen a real switch, the
menu has never been opened by hand, and the burst rule has only been proved
against synthetic events. The PIO program itself is verified against
`tools/pio_sim.py`'s scenarios by a cycle-accurate interpreter in
`crates/scoreboard-input`, which is a stronger check of the *timing* than a
finger would be — but it cannot tell you the pull-ups are right or that the pins
are the ones on the board. **This is the largest untested surface task #12
leaves**, and task #13 should not sign off without a unit that has buttons.

**A real VEML7700.** Same shape: the driver's register writes and the lux scale
are transcribed and the curve is host-tested against the Python's values, but no
part has ever acknowledged on this bench. The absent path is thoroughly tested;
the present path is not.

**The one-week soak.** Task #13. BACKLOG 69's blocker is cleared: a device that
falls off the network now resets itself and leaves a record saying so.

---

# The health gate keys on answers, not successes (BACKLOG 70)

Approved and implemented after the silent association-loss recurred live on the
bench. Deviation 11 above described the gate as first shipped; this replaces it.

## What changed

The gate had two starving conditions — "no successful poll in three intervals"
and "`MAX_FAILURES` consecutive failures". Both are now one: **"nothing has
answered over the network in three poll intervals."** The failure streak still
counts, still raises the error screen at `MAX_FAILURES`, and is no longer an
input to the watchdog at all.

The reason is the case the first version got wrong. A backend that is up and
returning 500s, or a path that 404s, is a **working network** — and the old gate
rebooted the device over it roughly every hundred seconds for as long as the
outage lasted, where MicroPython showed the error screen and sat there. Only
silence should starve, because only silence is evidence the radio is gone.

The gate moved to `scoreboard_model::poll::gate` on the way, so it is host-tested
rather than reachable only from a device: it is a decision over two numbers and
a boolean, which is exactly what SPEC §2's crate-boundary rule is about.

## "Answer" means the HTTP layer — the one real design question

A connection *refused* is, in principle, equally good evidence of a live link: a
RST came back, so something out there is alive. It is not usable evidence here,
and both reasons are measurements rather than opinions.

**`Transport::Connect` already cannot tell "refused" from "no socket".**
`api_client`'s own comment, written in task #11 and unrelated to this change,
records it: embassy-net answers `ConnectionReset` for a refused connect and for
an exhausted socket pool alike, and separating them needs socket state the
client does not have.

**And on this stack a closed port does not even refuse.** Task #12's watchdog
drill pointed `api.url` at a closed port and got `Timeout: backend not
responding` — not a connect error. A gate keyed on "refused" would therefore
have starved in precisely the case it was written to exempt. The discrimination
is not there to key on.

Against the deployed backend the distinction is nearly vacuous anyway: a dead
app behind Fly's edge answers 502, which is an HTTP answer. So "answer" is
defined as **a response reached the client** — any status, plus a body that
arrived and then failed to decode, all three of which prove DNS resolved, TCP
connected and bytes crossed in both directions.

The residual is stated rather than hidden: an `api.url` typed to a
reachable-but-refusing address reads as link death. That is a misconfiguration,
it is visible on the error screen, and the watchdog is opt-in.

The clock is stamped in `ApiClient::get`, the single function every request
funnels through — not at the six call sites — so a seventh endpoint cannot
forget it, and so the failures count too.

## Bench-validated, in both directions, with the streak on the wrong side

Deliberately arranged so that the *old* gate and the *new* one disagree in each
drill.

**A backend that answers and fails must not starve.** Watchdog armed at 8,000 ms,
poll interval dropped to 10 s so the silence limit is 30 s, `api.url` pointed at
the real backend with a bogus path so every request 404s:

```
[INFO ] watchdog: armed, timeout 8000 ms, feeding every 2000 ms
[INFO ] api: GET /backlog70/baseball/mlb/games -> 404 in 145 ms
[ERROR] poll: poll failed (1/5): HTTP 404: unknown_error
...
[ERROR] poll: poll failed (19/5): HTTP 404: unknown_error
[WARN ] poll: no successful poll since boot, failure streak 19
```

Failure streak **19**, nearly four times `MAX_FAILURES`, across several minutes —
**no starvation, no reset**, the streak climbing monotonically the whole time
(which is itself the proof it never rebooted). The old gate would have starved
this device twice over: once on the streak at 5, once on `since_success` at 30 s.

**True silence must still starve.** `api.url` pointed at an unroutable address so
nothing answers at any layer, poll interval back to 30 s:

```
[ERROR] time: sync failed, Timeout: backend not responding
[ERROR] poll: baseball/mlb list refresh failed, keeping cached slate: Timeout: backend not responding
[ERROR] poll: poll failed (2/5): Timeout: backend not responding
[ERROR] watchdog: starving on purpose (link silent); hardware reset within 8000 ms
```

Starved at a failure streak of **2** — *below* `MAX_FAILURES`, so the streak
demonstrably did not cause it — and the reset produced:

```
last abnormal shutdown: watchdog starved on core 0
uptime: 91 s
stack high-water: core 0 25816 of 266504 B, core 1 3348 of 8192 B

watchdog starved: nothing answered in 91 s, over 3 poll intervals (failure streak 2)
```

The streak rides along in the message even though it starved nothing: "silent
for 91 s after 2 failed polls" and "silent for 91 s having never polled" are
different faults, and the breadcrumb is the only place that difference is
recorded. Config restored inside the reboot cycle's window; the poll loop
recovered to `poll: lists refreshed, sources 1, rotation 15`.

**Link death itself** — the radio associated but off the network — is covered by
the live recurrence that prompted this change plus the silence drill above,
which produces the identical signal the poller sees: no HTTP answer, from the
first request to the last. What differs between them is the cause, not the
evidence, and the evidence is all the gate can read.

## Cost

32 B of RAM (one atomic, plus the gate's move into `scoreboard-model`) and 552 B
of flash. Nine host tests in `scoreboard-model`, covering both halves of the
distinction, the boundary, the setup-mode exemption and the streak's
irrelevance.


## Phase 4: OTA, and one gap MicroPython never had to think about

### `<device_name>.local` — a regression found, not a feature added

MicroPython resolved it for free: `network.hostname()` sets the lwIP hostname
and its port compiles lwIP with `LWIP_MDNS_RESPONDER`, so the network stack
answered before any Python ran. embassy-net only sends the hostname in DHCP
option 12, which teaches the **router** the name and does nothing for a client
that goes straight to multicast — which is most of them. The Phase 3 app
inherited that silently; `app/Cargo.toml` even wrote down that mDNS was not
enabled "because DHCP option 12 tells the router", which turned out to be the
bug rather than the justification.

`scoreboard_portal::mdns` + `net::mdns` close it, in both station and setup
mode. Four things differ from `dns.py`'s responder and each has its own
argument in the module docs: no question section in a multicast response, the
legacy-unicast shape for queries from a port other than 5353, the QU bit, and
the cache-flush bit. **AAAA is deliberately not answered** — the captive portal
lies to every query on purpose, and doing that here would hand a resolver a
malformed answer.

18 host tests, including every truncation of a query and a decompression bomb.

### OTA, feature for feature against `ota.py` + `main.py`'s `ota_check_task`

| MicroPython | Here | Status |
|---|---|---|
| daily check, hourly after failure, 120 s settle | same intervals, same settle | **ported** |
| `POST /api/check-update` → status string | same six statuses, same SPA contract | **ported**, asynchronously — the handler signals the poll loop and waits, because the client and the display state are poller task locals |
| progress screen, commit per percent change | `scoreboard_ota::Progress`, host-tested | **ported** |
| 5→1 restart countdown | same | **ported** |
| `/ota_dev` marker file | split: the rollback guard is a property of the image (`dev` version prefix), the staging pin is `ota.channel` | **improved** — a marker can be missed, a compiled-in constant cannot |
| `apply_staged()` at early boot | embassy-boot swaps before the app runs at all | **replaced** |
| `recover()` — re-download when the app will not import | *no counterpart, and none needed* | **retired** — the bootloader reverts to the last confirmed image, which is the case `recover()` existed for. A corrupt active partition is not reachable: the swap is atomic and resumable from the state partition's progress array. |
| boot-fail counter in `main.py` | the OTA attempt record (SPEC §9's third key) | **relocated** — A/B handles the *boot* failure; the record handles the *re-download* loop, which A/B does not |
| sha256 identity | build-stamped version + sha256 as an integrity check | **changed** — a running image cannot hash itself, so identity is stamped at build time and the hash goes back to being a checksum |
| TLS + API key | plain HTTP, ed25519 on the artifact, separate API key | **changed**, SPEC §8 |

**Not yet exercised on hardware.** Everything above is host-tested or
build-verified; the swap, the revert, the trial-boot confirm and the timings
are task #16's drill day.

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

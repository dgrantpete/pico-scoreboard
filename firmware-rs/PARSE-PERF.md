# Parse performance — measured reality, the batched drive, and the lever still on the table

Written 2026-08-17, the morning after the owner's benchmark-everything
night. This is the onboarding document for anyone (human or agent) picking
up on-device parsing performance: what was measured, what was already
fixed, how to validate any change to this stack without fear, and exactly
where the remaining speedup lives. Companion documents:
`PHASE-S-CHECKLIST.md` (the S1 validation addendum carries the corpus
evidence), `crates/scoreboard-espn/DESIGN.md` (the 16 rulings),
`repos/picojson-feasibility` and `repos/s1-differential` (sibling harness
repos).

## Why this exists

S3 (the direct-to-ESPN feed) makes the device parse ESPN's JSON itself.
The owner's bar, stated 2026-08-16: parsing must not compromise core-0
responsiveness — a sustained 1+ second CPU burn was named as grounds to
retreat to the hosted backend entirely. So the numbers below are not
curiosities; they are the go/no-go input for the standalone architecture.
Verdict as of this writing: **go, with margin** — see "Decision status."

## The measured picture (RP2350 @ 150 MHz, real bodies, 3 reps, ±0.1%)

Bench: `firmware-rs/bench` (standalone workspace, hub75-diag pattern).
Bodies baked into flash: the largest stored MLB slate (489,771 B), a live
99-event college-football Saturday slate (1,238,737 B — larger than the
device's entire RAM; the case streaming exists for), the largest stored
MLS slate (207,433 B). Chunks are memcpy'd into a 4 KB RAM buffer inside
the timed region, socket-read-shaped.

| Operation | Stock picojson 0.2.3 (+#98) | Batched drive (`ea29142`) | SWAR tokenizer (`ca799df`)¹ | + `ram-exec` (`8fb8a76`) |
|---|---|---|---|---|
| MLB list, 489 KB | 948 ms (516 KB/s) | 874 ms (560 KB/s) | 542–609 ms | **348 ms** (1405 KB/s) |
| CFB list, 1.21 MB | 2.62 s (473 KB/s) | 2.03 s (609 KB/s) | 1.44–1.70 s | **1.17 s** (1054 KB/s) |
| MLS list, 207 KB | 320 ms (648 KB/s) | 247 ms (840 KB/s) | 197–212 ms | **152 ms** (1368 KB/s) |
| MLB detail | 886 ms | 610 ms | 396–612 ms | **280 ms** |
| CFB detail, early target | 2.12 s | 1.18 s | 962 ms–1.33 s | **899 ms** |
| CFB detail, last target (worst) | 2.36 s | 1.62 s | 1.43–1.64 s | **1.20 s** |
| MLS detail | — | 349 ms | 179–248 ms | **151 ms** |

¹ Ranges, not noise: two builds of identical SWAR parse code (the second
merely added a bench section) differed ±30% per lane — see "the layout
lottery" below. Within one build, reps repeat to ±0.1% as before. The
batched-drive column was re-measured 2026-08-17 afternoon and reproduced
the prior night's numbers exactly (2.0336 s CFB list, 610.0 ms MLB
detail), so the baseline is solid; it is the flash-resident-code world
that is layout-sensitive.

Findings that shape design, all measured the same night:

- **Chunk size is a non-issue.** 1379 B (TLS-record payload) vs 4096 B
  chunks differ by 1–2% end to end. Feed whatever the transport hands you.
- **Validate-until-found is nearly free.** Pre-optimization the worst-case
  detail (target last of 99 events) cost only +11% over the best case —
  proof the per-byte tokenizer path dominated and the matcher/sink layer
  was noise. (Post-batching the spread widens to ~37% because the noise
  floor dropped; the policy is still cheap in absolute terms.)
- **PNG is a solved problem.** `crates/png-stream` decodes a 100 px CDN
  combiner logo to a 24×24 RGB565 sprite in **~8.3 ms** (500 px originals:
  156–209 ms). S3 should fetch the 100 px combiner variants; at 8 ms per
  crest the entire 32-slot pool refills in a quarter second of CPU.
- **The original estimate was off 30×.** PHASE-S.md's "~20 ms per 300 KB"
  paper number vs ~0.5 MB/s measured. Same species as drill day's 8×
  hash miss. The lesson keeps being the lesson: measure before believing.

### The responsiveness reframe (read this before panicking at "2 seconds")

Parse *seconds* are not core-0 *freezes*. The poll loop yields between
chunks, so during a 2-second parse, every other core-0 task (HTTP server,
button drain, watchdog feeder, brightness) gets scheduled every **~2–3 ms**
at TLS-sized chunks. What a long parse costs is *duty cycle*, not latency:
post-batching, a college-football Saturday ≈ 11% of core 0 at a 30 s poll
cadence; a normal MLB day ≈ 5%. Core 1's rendering shares nothing with any
of this. The owner's "1+ second parse causes core-0 problems" concern
resolves to scheduling granularity, and 2–3 ms granularity is comfortably
below anything user-perceptible on this device.

### Host controls (for scale, and for honesty about ratios)

- scoreboard-espn end-to-end list pass, desktop: ~61 MB/s → device ratio
  ~128×, the expected envelope for ~5 GHz out-of-order vs 150 MHz in-order.
  Nothing pathological on-device; the M33 is just small.
- picojson parse-only (`picojson/examples/throughput.rs` on the fork),
  same 1.21 MB body: 113 → 161 MB/s (+43%) from the batched drive. Host
  gains *under-predict* device gains for this class of change: the
  eliminated per-byte scaffolding is exactly what a wide OoO core hides in
  parallel and an in-order core pays serially — details on device improved
  45–80% against the host's 43%.
- Beware one harness trap that burned a control run: the sweep example
  treats any body with parse failures as anomalous and detail-extracts
  EVERY event in it. A "throughput" measurement over such a body measures
  ~100 full re-parses, not one. Use clean bodies (MLB max) for list-rate
  controls.

## What the batched drive changed (and where it lives)

**Diagnosis** (2026-08-17, from source + arithmetic + the +11% evidence):
`PushParser`'s drive loop fed the ujson tokenizer ONE BYTE per
`parse_chunk` call — per input byte: a call with a one-byte slice, an
event-accumulator clear/scan, closure construction, and a per-byte
content-accumulation callback. ~310 cycles/byte total against an estimated
30–60 of real tokenizer work.

**Fix**: `write()` hands the whole chunk to `parse_chunk` once; events
(which carry chunk-relative positions — always available, never used by
the old drive) dispatch straight from the tokenizer callback; content is
captured as position-derived **span copies** at boundaries only (token
begin/end, escape begin/end, chunk end). Clean content bytes are never
visited by parser code. Escape regions keep a slow path. The public API is
byte-identical to the PR #98 surface, so it drops into every consumer
unchanged. Two subtleties that will bite a re-implementer:

- The borrowed-vs-scratch event identity is part of observable behavior:
  a chunk boundary forces scratch mode **only when a nonzero prefix was
  copied** — a token starting at a chunk's last byte stays `Borrowed`
  from the next chunk. The stress suite catches violations.
- The tokenizer emits `Begin(UnicodeEscape)` at the *first hex digit*
  (after consuming `\u`), and `End` at the fourth — span math for split
  `\uXXXX` sequences keys off those positions.

**Where**: fork `dgrantpete/picojson-rs`, branch `perf/batched-push-drive`
(commit `ea29142`), sitting on top of the `push-parser-per-call-input`
branch (upstream PR #98). The tokenizer work below stacks further:
branch `perf/tokenizer-swar-scan` = `ca799df` (SWAR scan) + `8fb8a76`
(`ram-exec` feature). **All local — deliberately not pushed** as of this
writing; the owner wanted them PR-shaped but held. To ship: push the
branches, open the PRs as a stacked follow-up chain to #98, and once any
release carries them, replace the `[patch.crates-io]` git-rev pins (root
`Cargo.toml` AND `firmware-rs/bench/Cargo.toml` — standalone workspaces
repeat the patch; any future device workspace using picojson needs it
too) with the release version.

## The tokenizer-core lever, pulled (2026-08-17 afternoon)

The SWAR surgery predicted above landed as `ca799df` on fork branch
`perf/tokenizer-swar-scan` (stacked on `perf/batched-push-drive`; both
local, unpushed). Three changes, all inside `parse_chunk_inner`:

- **String interiors** (measured 72–75% of every real body's bytes)
  advance through a 32-bit SWAR scan — four bytes per iteration against
  the three in-string byte classes (`"`, `\`, control); the scan advances
  only past definitively-clean bytes and every flagged byte re-enters the
  untouched byte machine, so events, positions, errors, and chunk
  resumability are identical *by construction*. The less-than-0x20
  detection admits borrow false-positives only above a genuine stop; the
  scan takes the lowest flagged byte, which is therefore always real (an
  exhaustive boundary-palette unit test pins this).
- **Digit runs** in the three self-looping number states skip the same
  way (byte-wise loop; numbers are short).
- **Position/line/column bookkeeping hoisted** out of the per-byte loop;
  the absolute `Position` is built on demand at error sites and comma
  records only.

The full validation ladder above was rerun in order and is green: fork
suite (now 235 + stress + buffer-reuse), thumbv8m check, patched-check
corpus at chunks 1/7/1379/4096, `scoreboard-espn` 112, `backend` 63 (incl.
the 33 byte-exact goldens), then silicon. Host tokenizer throughput:
160 → 496 MB/s (3.1×) on the CFB body; full football extraction on host:
8.7 → 3.6 ms (2.4×, `crates/scoreboard-espn/examples/detail_bench.rs`,
written for exactly this A/B).

### What silicon then taught: the layout lottery, and `ram-exec`

The device did not follow the host. Lists improved 1.2–1.6×, but CFB
detail *regressed* in the first SWAR build — and a second build of
identical parse code (only a bench section added) swung every lane by
±30%, some up, some down. The re-measured baseline reproduced exactly, so
this is real: **once the per-byte ALU work shrinks, XIP instruction fetch
is the wall.** The hot path (tokenizer + drive + engine + per-sport sink,
~30–40 KB across `write`/`handle_event`/`fire_values` monomorphizations)
does not fit the RP2350's 16 KB XIP cache, and link-layout luck decides
which lanes thrash.

Two attribution experiments, both in the bench now:

- **Data-source control** (`CTRL` / `mls-207K-RAMSRC` sections): feeding
  the MLS extraction from a RAM copy of the body instead of flash saves
  ~10% (196.8 → 175.6 ms; the memcpy+sum control is 15.9 ms of it). So
  flash-resident *bodies* pollute the XIP cache only mildly — the bench
  stays representative, and the real firmware (socket data, already in
  RAM) gets that ~10% for free.
- **Code-source experiment**: `ram-exec`, a new default-off feature on the
  fork (`8fb8a76`), places `parse_chunk_inner` in `.data` on embedded ARM
  so startup copies it to SRAM. That one function executing from RAM took
  every lane to its best measured value (table above): 1.3–2.5× vs the
  flash-code best case, ~4.6 KB of RAM per tokenizer monomorphization
  (the bench pays 27.5 KB for its six sink types; the real app's fetch
  path uses one or two).

To reproduce the `ram-exec` numbers: path-flip bench's `Cargo.toml` pin as
usual, then add a direct `picojson = { version = "=0.2.3", features =
["ram-exec"] }` dependency there — feature unification turns it on for the
shared build. The committed bench does not enable it (the committed pin
predates the feature).

### Levers that remain, sized

1. **`ram-exec` in the real firmware** — the big one, but it is a RAM
   budget line (~5–10 KB) against BUDGET.md's 40%-plus-reservation
   discipline, so it is the owner's call at S3 integration time, measured
   against real headroom. The same trick extended to the engine/sink layer
   (`fire_values`, `handle_event`) would chase the rest of the layout
   lottery, at proportionally more RAM.
2. **UTF-8 revalidation second pass**: every string/key token pays
   `core::str::from_utf8` over its content at extraction — a second walk
   over ~75% of body bytes, low single-digit % of total. Removing it needs
   an ASCII-purity side-channel from the tokenizer's scan (which already
   sees every byte) — an upstream API conversation, not worth it yet.
3. **Number parsing is NOT a lever**: `JsonNumber::from_slice` eagerly
   parses (soft-float f64 for decimals) and scoreboard-espn only reads
   `.as_str()`, but real bodies carry only ~1–2.5k number tokens (~400
   floats max) — a few ms per parse. Measured, dismissed.

## Hardware bench operations (learned the annoying way)

- `firmware-rs/bench` is a standalone workspace; `cargo build --release`
  then flash. **Always check the ELF entry before probe-flashing
  anything**: `0x10000xxx` = standalone (bench, safe), `0x1000Axxx` =
  boot-integrated (the real app). TOOLCHAIN.md's warning is not decorative.
- **Never leave a `probe-rs run` attached when you need the probe for
  anything else.** It holds the USB device exclusively; a dying session
  holds a half-dead handle. What looked like a hardware wedge (DAP
  no-acknowledge, five straight failures) was competing sessions — stop
  the old task first and everything works. `--connect-under-reset` is
  unavailable on this rig (no reset line wired).
- Restore procedure after benching: build `firmware-rs/boot` (release) and
  `firmware-rs/app` with `--no-default-features --features
  link-boot-integrated`; `probe-rs download` both ELFs; `probe-rs reset`.
  The config/storage flash region sits above anything the bench image
  touches, so the device rejoins WiFi on its own — verify at
  `http://192.168.50.57/api/status`. A source-built restore is
  version-stamped `dev` and **refuses OTA self-update by design**; the
  next `publish-fw --channel stable` restores the published OTA identity
  (and pairs naturally with the still-held backend deploy).

## Decision status (owner, 2026-08-17)

The hosted-backend retreat was on the table if parsing threatened
responsiveness. With the batched drive landed on the fork and the duty
cycle/granularity numbers above, the retreat is **not forced** — S3 can
proceed on the streaming design. `main` keeps the full backend world until
S4 regardless, so the retreat remains a one-decision fallback, not a
rebuild. Reopen this question only if S3 integration shows real-world
symptoms the bench didn't predict (poll-loop CPU delaying input handling
or WiFi servicing), and reach for the tokenizer-core lever before reaching
for the retreat.

*Addendum (2026-08-17 afternoon, tokenizer lever pulled):* the margin
widened again. With SWAR alone the college worst case is 1.4–1.7 s
(≈5–6% of core 0 at a 30 s cadence); with `ram-exec` it is 1.2 s, and a
normal MLB day's list is 348 ms (≈1.2%). The remaining decision in this
document's orbit is whether the real firmware spends the ~5–10 KB of RAM
`ram-exec` wants — an S3-time BUDGET.md line, not a go/no-go input.

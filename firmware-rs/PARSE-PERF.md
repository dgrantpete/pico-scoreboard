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

| Operation | Stock picojson 0.2.3 (+#98) | Batched drive (`ea29142`) |
|---|---|---|
| MLB list, 489 KB | 948 ms (516 KB/s) | **874 ms** (560 KB/s) |
| CFB list, 1.21 MB | 2.62 s (473 KB/s) | **2.03 s** (609 KB/s) |
| MLS list, 207 KB | 320 ms (648 KB/s) | **247 ms** (840 KB/s) |
| MLB detail | 886 ms | **610 ms** |
| CFB detail, early target | 2.12 s | **1.18 s** |
| CFB detail, last target (worst) | 2.36 s | **1.62 s** |

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
branch (upstream PR #98). **Local only — deliberately not pushed** as of
this writing; the owner wanted it PR-shaped but held. To ship it: push the
branch, open the PR as a follow-up to #98, and once any release carries
both, replace the `[patch.crates-io]` git-rev pins (root `Cargo.toml` AND
`firmware-rs/bench/Cargo.toml` — standalone workspaces repeat the patch;
any future device workspace using picojson needs it too) with the release
version.

## The lever still on the table: tokenizer-core surgery (est. 2–3×)

Post-batching, the wall moved *inside* ujson: the tokenizer still runs a
per-byte state machine (a match over (state, byte-class) plus a position
counter store, per byte). Measured residue: 268 cycles/byte on the MLB
body, 246 on college, 179 on MLS — string-heavy bodies cost more, which
points straight at the opportunity:

**Word-at-a-time string scanning.** String interiors dominate JSON bytes,
and inside a string the tokenizer only cares about three byte classes:
`"` (close), `\` (escape), and control bytes (< 0x20, error). The classic
fix is scanning words, not bytes: on the Cortex-M33 (no SIMD) that is
32-bit SWAR — the `(x ^ splat(b))` then `(y - 0x01010101) & !y & 0x80808080`
zero-byte trick per target class, four bytes per iteration, falling back
to the byte loop at boundaries and matches. Same idea applies to
whitespace runs between tokens. Expected: 2–3× on the remaining cost
(strings are most of the bytes; the scan replaces ~4 matched-dispatch
iterations with ~3 ALU ops), putting the college worst case plausibly
under 700 ms and MLB under 300 ms.

What a taker needs to know:

1. **Where**: `picojson/src/ujson/` (the tokenizer core) on the fork —
   this is the conformance-tested heart of the crate, shared by all three
   parsers. Touching it is upstream-scale surgery; do it on a branch off
   `perf/batched-push-drive` so the two PRs stack.
2. **Contract to preserve**: event *positions* exactly as today (the
   batched drive's span math depends on them — especially the
   `Begin(UnicodeEscape)`-at-first-hex quirk); resumability at any byte
   boundary (a scan window must stop cleanly at chunk end and resume);
   the callback event order for multi-event bytes (e.g. a digit that ends
   a number AND closes an array).
3. **The validation ladder makes this safe** — run it in full, in order:
   - fork: `cargo test` (234 unit + stress + buffer-reuse) and
     `cargo check --target thumbv8m.main-none-eabihf`;
   - `repos/picojson-feasibility/patched-check`: full ESPN corpus through
     a reused 4 KB buffer at chunk sizes 1/7/1379/4096, sequences
     byte-compared (do NOT run the top-level feasibility crate — its
     compile_fail doctest pins crates-io 0.2.3 and will misfire);
   - pico-scoreboard: temporarily flip root `Cargo.toml`'s
     `[patch.crates-io]` picojson to `path = "../picojson-rs/picojson"`,
     run `cargo test -p scoreboard-espn` (112 incl. engine chunk-split
     invariance) and `cargo test -p backend` (33 byte-exact goldens
     through the real serving path), then restore the pin and
     `git restore Cargo.lock`;
   - hardware: `firmware-rs/bench` with the same temporary path patch in
     ITS `Cargo.toml` — see operations below. The host throughput example
     is a screener; the M33 number is the verdict.
4. **Ambition cap**: if SWAR inside ujson balloons, a cheaper cousin is
   hoisting only the position-counter update out of the per-byte match.
   Profile first; the fork's `examples/throughput.rs` gives 5-second
   feedback loops on the host.

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

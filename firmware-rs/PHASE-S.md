# Phase S — direct-to-ESPN standalone mode: feasibility notes

Written 2026-08-08, the night the Rust firmware became the daily driver, after
the owner asked whether the moonshot was real. Verdict: **feasible — roughly
two Phase-2-sized efforts, and every seam it needs already exists.** These
notes capture the analysis so the project can start cold when its time comes.
SPEC.md §1 lists this as a non-goal for the parity release; that stays true.
Nothing here leaks requirements into the parity firmware beyond the seams
already cut.

## What "direct" means

The device talks to `site.api.espn.com` itself: TLS, scoreboard + summary
JSON, logo PNGs. The backend proxy becomes optional — either a degraded
fallback (the spec's original framing) or, for a standalone single-user
device, absent entirely. The wire format is not "collapsed" in direct mode —
it is **deleted**: it exists only to cross the proxy hop. A second
`GameFeed` implementation (`scoreboard-model/src/feed.rs` — the seam
preserved through every phase for exactly this) produces the model types
straight from ESPN's JSON.

The deeper collapse is architectural: port the backend's per-sport
`transform.rs` logic into a shared `no_std` crate over a streaming-JSON
abstraction, and one transform implementation serves both deployments — the
same trick `scoreboard-wire` pulled for the wire format in Phase 0.

## The budget math (all figures measured, 2026-08-08)

| Concern | Number | Basis |
|---|---|---|
| ESPN scoreboard JSON | ~284 KB (MLB, in season) | NUC collector logs |
| ESPN summary JSON | 390–456 KB per game | NUC collector logs |
| RAM free on device today | ~250 KB | /api/status, BUDGET.md |
| Streaming parse state | ~2–4 KB | tokenizer + path machine, no tree |
| TLS record buffers | ~40 KB (conservative) | embedded-tls, no fragment negotiation assumed |
| `png-stream` inflate window | 32 KB, transient | SPEC §14; union with OTA scratch |
| Net RAM delta | **~70–80 KB** | fits inside the ≥40 % headroom rule |
| JSON parse CPU | ~20 ms per 300 KB | compiled parse at 150 MHz, streamed |
| TLS handshake | ~0.5–1 s | software P-256 on M33 (RP2350 has SHA-256 accel, no ECC accel); amortize with keep-alive |

The payloads never fit in RAM and never need to: a pull tokenizer walks 4 KB
chunks; a path-matcher extracts the ~30 fields per game the transform reads
and skip-subtrees the rest.

## The streaming parser

The hard 20 % is **resumability mid-token**: a chunk can end inside
`"Ohtan`, inside a `\u30` escape, or mid-number, and the tokenizer must
return "need more bytes" and resume. That is a ~500-line state machine, not a
monster — general-purpose parsers balloon from API surface, not from this.

Ecosystem survey (2026-08-08 — re-audit at start of work, this corner moves):

- [`picojson-rs`](https://github.com/kaidokert/picojson-rs) — push-based,
  incremental, no_std, escape handling, conformance tests. **Audit this
  first** (SPEC §10 criteria: no-alloc, caller-owned buffers, resumable
  across arbitrary chunk splits, license, fuzzing).
- [`json-streaming`](https://crates.io/crates/json-streaming) — no_std via
  feature flags, BYO read traits.
- [`json-stream-parser`](https://crates.io/crates/json-stream-parser),
  [`json_event_parser`](https://docs.rs/json-event-parser) — streaming,
  std-flavored.
- API role models (std-only, won't board the firmware): `struson` (Gson-style
  pull reader), `actson` (push/non-blocking — the right *shape* for network
  chunks).

Whichever tokenizer wins, the layer above is ours and is where the
cleanliness lives: a **declarative const path table**
(`$.events[*].competitions[0].competitors[*].score → field`) with
skip-subtree as the default. The ESPN-specific knowledge stays in one
readable table per sport, mirroring the backend transform it replaces.

A satisfying convergence: streaming forbids borrowing from the receive buffer
(the bytes leave with the chunk), so every extracted value copies into
bounded storage immediately — which is exactly `scoreboard-model`'s `Text<N>`
types with corpus-derived bounds. The no-alloc discipline is the shape
streaming needs.

## Honest risks

1. **ESPN drift.** Undocumented API; the backend absorbs shape changes with
   lenient parsing and a five-minute redeploy. On-device, a parse break is an
   OTA cycle — which is why OTA (task #15) had to exist first. Mitigations:
   skip-unknown-by-default leniency; keep proxy mode primary with direct as
   fallback.
2. **TLS housekeeping.** Root-CA pinning makes CA rotations OTA events. Pin
   multiple roots; document the rotation runbook.
3. **User-Agent filtering.** ESPN's edge 403s unrecognized UA prefixes
   (found 2026-08-07, see `backend/config/default.toml`). The device spoofs
   the same allowlisted prefix; residential IPs are friendlier than
   datacenter ranges, but the filter can change under us.
4. **Fleet economics.** One proxy polling ESPN once and fanning out ~300-byte
   wire packets beats N devices each pulling 300 KB (ESPN sends no ETags we
   can rely on). Direct mode is the standalone dream, not necessarily the
   fleet architecture. The backend also still hosts OTA.

## The validation rig already exists

`tools/espn`'s mock server replays captured real streams (NUC collector
Postgres, `.espnbundle` exports) over plain HTTP. Point the direct feed at
the mock, replay a full pregame→final capture, and assert against the same
parity harness that proved the render stack. The July staging infrastructure
was unknowingly built for this.

## Prerequisites and sequencing

1. Task #15 (OTA) shipped and drilled — parse fixes must be shippable.
2. The shared-transform refactor (backend `transform.rs` → no_std crate) —
   standalone value even before the device uses it, same as Phase 0's wire
   extraction.
3. Streaming tokenizer audit → adopt or write.
4. `png-stream` (SPEC §14) for logos.
5. TLS bring-up (embedded-tls via reqwless) against the mock first, then ESPN.

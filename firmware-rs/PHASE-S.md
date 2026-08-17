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

## State of play, 2026-08-16 — re-read after drill day

The owner asked again the evening the OTA drill finished, which is the right
moment: **prerequisite 1 is done.** All eight drill steps passed on the seated
unit — rollback, attempt records, three power cuts, the no-backend confirm
deadline — so "a parse break is an OTA cycle" is now a proven recovery path
rather than a plan. Prerequisites 2–5 remain untouched.

Three things the drill taught that this document should carry:

- **Distrust the crypto estimates the way the hash estimate earned.** The
  verify hash was estimated at 1–3 s and measured at **10,635 ms** (1.09 MB,
  ~103 KB/s read-and-hash, opt-level 3) — past the 8 s watchdog and fatal
  until the loop learned to feed. The TLS handshake figure above (0.5–1 s
  software P-256) is the same species of paper number. Budget seconds, plan
  the keep-alive amortization as load-bearing rather than nice, and measure
  before believing (BACKLOG 87 is the profiling instrument).
- **The wall clock is really two questions, and ESPN answers neither.** The
  device's one `GET /time` returns the epoch *and the UTC offset*, the offset
  from the backend's GeoIP lookup — that is what the MaxMind database is for.
  Direct mode replaces the epoch trivially (SNTP over UDP costs one transient
  socket; `SOCKETS = 10` has room). Nothing upstream answers "what is this
  living room's offset."

  **DECIDED (owner, 2026-08-16): the browser seeds it.** The settings SPA
  runs on a device that lives in the scoreboard's own household, so on load
  the client posts an offset *schedule* in the background — current UTC
  offset, the instant of the next DST transition, and the offset after it,
  all computable client-side by probing `Date` over the coming months. The
  Pico stores three numbers and flips at the timestamp: no timezone database
  aboard, every casual settings visit refreshes the horizon, storage is
  written only when the values change, and last-writer-wins is accepted for
  a household device. This is BACKLOG 95, it retires the MaxMind dependency,
  and it is one more reason the shared transform treats the offset as
  configuration, not discovery. A manual override field can ride along for
  the pathological case, but the seed is the design.
- **OTA lives on the backend too.** `/fw/manifest` + `/fw/image` are backend
  routes. Backend-optional therefore means OTA-optional unless the artifacts
  move to any dumb HTTPS host — the ed25519 signature carries the trust, not
  the transport — at which point TLS-to-that-host joins the scope.

Which sharpens the scope sentence: "remove the Rust backend" decomposes into
(a) **the device not needing it** — games, logos, time — which is Phase S
proper and achievable, and (b) **deleting the deployment**, which stays
blocked on the gift fleet (wire v2 + `/app/*`) and on rehoming `/fw`. (a) is
the goal; (b) is a separate decision for a later era, exactly as the fleet
economics section already argued.

Budget check against today's measured device: ~230 KB free after drill day's
four-connection pool, against the ~70–80 KB net delta estimated above — fits
twice over. Sequencing stands as written, with one note: the `tools/espn`
mock speaks plain HTTP, so the TLS bring-up (step 5) needs a TLS terminator
in front of the mock — or a disposable HTTPS echo target — before pointing at
ESPN itself.

## Owner decisions, 2026-08-16 evening — branch-separated full removal

Recorded the evening Phase S kicked off, superseding three framings above;
the working tracker is `PHASE-S-CHECKLIST.md`.

- **Separation by branch/release, not by flag.** The "direct as a mode /
  degraded fallback / config switch" framing is retired. `main` keeps the
  backend world; the `phase-s` branch deletes it outright once the direct
  feed is proven (S4), and the two worlds ship as different releases. The
  fleet-economics argument stands — which is exactly why the deletion lives
  on a branch while the gift fleet lives on `main`.
- **Consequence accepted: no in-image proxy fallback.** Risk 1's mitigation
  ("keep proxy mode primary") becomes an OTA runbook instead: ESPN drift on
  a standalone device is recovered by OTA, including OTA back to a
  proxy-world image kept published on a reachable channel.
- **OTA artifacts move to GitHub Releases** (repo verified public
  2026-08-16), which resolves the "rehoming `/fw`" question this document
  left open: the signature carries the trust, the host is dumb, and
  TLS-to-GitHub joins the S2 scope alongside TLS-to-ESPN.
- **Smarthome (BACKLOG 91) gets a reservation, not a design**: the
  BACKLOG-94 re-earn target now includes ~1 socket + 10–15 KB left
  unspoken-for after Phase S lands.

# Phase S checklist — branch-separated full removal

Written 2026-08-16, the evening the owner turned Phase S from "direct as a
mode" into **branch-separated full removal**. This file is the progress
tracker; the strategy and budget math stay in `PHASE-S.md`, the working rules
in `PHASE-S-KICKOFF.md`. Where this file and the older two disagree, the
dated decisions below win — both older docs carry pointers here.

## The decisions this file exists to carry (owner, 2026-08-16 evening)

1. **Separation by branch/release, not by runtime flag.** There is no
   dual-mode firmware and no `mode: direct` config switch. `main` remains the
   backend world (gift fleet, `/app/*`, `/fw/*`, proxy polling). `phase-s`
   becomes the standalone world: the firmware talks to ESPN itself and the
   backend is **fully deleted** on that branch. "For the time being" —
   `main` stays the deployed reality until the owner cuts over.
2. **OTA artifacts move to GitHub Releases** (owner: "if possible" —
   feasibility gates below; the repo is public, verified 2026-08-16, so
   unauthenticated asset fetches work). The ed25519 signature carries the
   trust; the host is dumb.
3. **The BACKLOG-94 re-earn target absorbs a smarthome reservation**: after
   Phase S lands, ~1 socket + 10–15 KB stay unspoken-for, for BACKLOG 91's
   future client. No smarthome design work inside Phase S.

**Branch mechanics that follow from decision 1.** S0–S3 produce only
additive, host-tested work — they land on `phase-s` and merge to `main` at
phase boundaries exactly as the kickoff prescribes, because nothing in them
deletes anything. S4 is the point where `phase-s` stops merging back: the
deletion sweep happens there and stays there. `main` merges *into* `phase-s`
routinely throughout, so the branch never drifts. One consequence to honor
early: **anything both worlds need forever lands on `main` side of the S4
line** — the `testdata/` relocation is the known case.

---

## S0 — streaming tokenizer: audit, adopt or write (host-only)

Gate to start: none. Gate to exit: chunk-split property tests green over the
real corpus.

- [x] Re-audit the ecosystem (2026-08-16): `picojson` 0.2.3 published
      2026-07-10, active, Apache-2.0, and now ships a **PushParser** —
      SAX-style push, the actson shape PHASE-S.md wanted — plus a `defmt`
      feature. Nothing among the other candidates beats it.
- [x] Scored against SPEC §10 (2026-08-16): `no_std` PASS (checked on
      `thumbv8m.main-none-eabihf` from a `#![no_std]` consumer), no-alloc
      PASS (source-audited; the only `Vec` is test code), caller-owned
      buffers PASS, split-resumability PASS (below), scratch overflow is a
      clean `Err`. Fuzzing evidence THIN — mitigations in the harness README.
- [x] Feasibility harness: `repos/picojson-feasibility` (2026-08-16, green).
      Whole-parse vs 1-byte-at-a-time (every split point in one pass) and
      seeded random chunkings over the full `backend/testdata/` JSON corpus;
      every individual two-chunk split of escape/surrogate/UTF-8/number
      nasties. Identical event streams throughout.
      **One blocker found and pinned by a `compile_fail` doctest:** `'input`
      is a struct lifetime on `PushParser`, so feeding from a *reused
      receive buffer* — the firmware's socket loop — does not borrow-check.
      Unnecessary by the crate's own contract (partials are copied to
      scratch and the slice reset before `write` returns); unchanged on
      upstream master; no upstream issue exists.
- [x] Adopt-or-write decision recorded as a SPEC Appendix A audit line
      (2026-08-16, "Phase S0 addition"): **adopt `picojson` =0.2.3 + the
      lifetime fix**. Owner approved outward contact; issue filed
      ([picojson-rs#97](https://github.com/kaidokert/picojson-rs/issues/97))
      and PR opened
      ([#98](https://github.com/kaidokert/picojson-rs/pull/98), fork
      `dgrantpete/picojson-rs` branch `push-parser-per-call-input`, local
      checkout `repos/picojson-rs`). Upstream's full suite green on the
      patch; `picojson-feasibility/patched-check` proves the firmware's
      reused-receive-buffer shape against the full corpus. Until a release
      carries the fix, S1+ pins the fork by rev via `[patch.crates-io]`.
- [x] Exit (2026-08-16): tokenizer chosen, harness results in
      `repos/picojson-feasibility/README.md`, audit line in SPEC Appendix A;
      merged to `main` at this boundary.

## S1 — shared transform crate (host-only)

Gate to start: S0 tokenizer chosen. Gate to exit: corpus parity with the
backend's transforms.

- [x] New `no_std` crate `scoreboard-espn` (2026-08-16/17): the path-matcher
      engine (bitset alive-sets over picojson push events, skip-by-masking,
      chunk-split invariance proven over the corpus) plus all four sport
      tables. **33/33 committed wire goldens byte-identical** through
      stream-extraction; 107 tests; rulings 14–16 grew out of the lanes
      (validate-until-found detail policy, one helper-consolidation pass,
      Exact refuse-to-truncate compare keys). `crates/scoreboard-espn/
      DESIGN.md` carries the architecture and all sixteen rulings.
- [x] **Resolve the feed-seam shape** (design item, this phase):
      `GameFeed::detail` takes a complete `&[u8]` payload — a whole-payload
      contract a 300–450 KB streamed body can never satisfy. Recommended
      shape: stream-extract into an owned bounded per-sport extract struct,
      then hand `Store` borrowed views over that extract, keeping `Store`
      untouched. Decide against the path-table design; golden-test either way.
- [ ] Soccer commentary decision (flagged for owner visibility, resolve in
      design): live soccer commentary comes from ESPN's *summary* endpoint
      (390–456 KB per live game). Recommendation: fetch it — streaming makes
      the size a non-issue (~30 ms CPU) — rather than degrade the render.
- [x] Backend migrated to the shared crate (2026-08-17): handlers feed raw
      bytes to the extractors, thin adapters map to unchanged DTOs, the
      serde DUs and per-sport transforms are deleted (−3,634/+1,215), and
      `wire_corpus` runs every fixture through the real serving path —
      33/33 goldens without re-blessing, 46/46 workspace test binaries.
      The adapter's friction report drives one API-unification pass
      (shared Counts/TransformError/Report shapes) as S1 polish.
- [x] Exit (2026-08-17, code-complete): zero transform diffs across the
      full corpus; merged to `main` at this boundary. **Deploy of the
      migrated backend is the owner's call** — it carries the seated
      unit's data plane; everything is green and ready when they are.

**S1 validation addendum (2026-08-17, owner-requested, NUC store).** The
whole collector corpus was ruled on, two independent ways:

- *Mass sweep* (`crates/scoreboard-espn/examples/sweep.rs`): ~149k bodies /
  1.37M events through the new extractors — 0 panics, 0 malformed-shell
  errors; 604 event rejections total (5 MLB, 599 MLS), all with coherent
  verdicts; 39,649 real soccer summaries through the summary extractor
  clean (closing the no-summary-fixtures gap).
- *Byte differential* (old serde pipeline vendored from git vs new, harness
  preserved at `repos/s1-differential`): 113,904 real bodies, 1,374,186
  events — **1,363,791 wire payloads byte-identical (117 MB), all 604
  rejections matched by event id, 1.29M JSON-only field comparisons clean,
  ZERO diffs**. Negative-control self-corruption proves the comparator's
  loudness. Only findings: soccer's `LIST_MAX = 64` list bound (synthetic
  80-game body only; dissolves in the API-unification round's sink list)
  and football's wire-derived JSON score (same round).
- *First silicon numbers* (`firmware-rs/bench`, standalone, on the seated
  unit's RP2350 @150 MHz): streaming list extraction ≈ **0.5 MB/s**
  (489 KB MLB slate 1.01 s; 1.21 MB live college slate 2.65 s) — the
  PHASE-S.md "~20 ms per 300 KB" paper number was off ~30×. Host control
  61 MB/s (128× — the expected in-order-M33 envelope, nothing
  pathological). S3 sizing: fine under a 30 s cadence with per-chunk
  yields; optimization lever: picojson's one-byte-per-call tokenizer feed.
- Corpus gaps that remain are seasonal, not tooling: NBA + college bodies
  are off-season empties until October / week 1 (BACKLOG 41/58).

## S2 — TLS bring-up (first on-device phase)

Gate to start: **BACKLOG 94 settled** — headroom re-earned to ≥40 % *plus*
the smarthome reservation, measured, not estimated. Gate to exit: handshake
and steady-state numbers in BUDGET.md.

- [x] BACKLOG-94 levers, measured then chosen (2026-08-17): the
      `-Zprint-type-sizes` walk found the arena was mostly duplication, and
      the two structural levers alone re-earned the budget — flat route
      dispatch (−7,360 B) and picoserve's select-duplication fix on fork
      `dgrantpete/picoserve` `perf/select-by-ref` (−25,408 B). Statics
      337,712 → 304,944 B = **42.7 % headroom, measured**; HTTP behavior
      byte-identical under a 16-probe matrix on the seated unit. Store fold
      and crest halving priced, deliberately untaken. Residual (BACKLOG 94):
      beyond-40 % slack is 14,544 B — covers the smarthome reservation at
      10 KB, 816 B short at 15 KB, and `ram-exec` at S3 wants 5–10 KB more;
      the owner names the reservation and the S3-time lever if one is
      needed. Gate is open on the 10 KB reading.
- [x] embedded-tls via reqwless, against a TLS terminator fronting the
      `tools/espn` mock first, then real hosts (2026-08-17 night,
      `firmware-rs/tls-spike` on the seated unit): full 481–498 KB mock
      bodies and real 215 KB `site.api.espn.com` scoreboards fetched over
      TLS 1.3 under the ruled posture, plus github.com. Two ecosystem
      walls found and fixed on fork `dgrantpete/embedded-tls`
      `port/der-0.8-stable`: (1) published embedded-tls cannot build
      against today's crates.io (der rc-pin rot; upstream PR #196 fixes
      main, unreleased), and (2) the default config only advertises RSA
      signature schemes with `alloc` on, so ESPN's RSA-only edge aborts a
      no-verify client at the handshake — advertising unconditionally
      fixed it, measured before/after on silicon. Numbers and the
      TCP-window-limited finding: BUDGET.md "Phase S2".
- [x] **Protocol check, early** (2026-08-17, from the dev machine): all four
      hosts the standalone build will touch — `site.api.espn.com`,
      `a.espncdn.com` (logo hrefs are absolute), `github.com`,
      `objects.githubusercontent.com` (release-asset redirect target) —
      negotiate TLS 1.3 with `TLS_AES_128_GCM_SHA256` specifically
      (embedded-tls's primary suite; verified via openssl s_client forced to
      that suite). No stop-and-redesign finding. The on-device handshake
      against real hosts remains S2's own exit evidence.
- [x] **Verification posture decided (owner, 2026-08-17 evening)** — the
      root-CA-pinning item dissolved into a per-host-family policy once the
      chain recon landed. ESPN and `a.espncdn.com` serve RSA-only chains
      (no ECDSA dual certs — verified by forcing ECDSA-only offers), and
      embedded-tls's RSA verification hard-requires `alloc`; the owner
      ruled the no-alloc contract wins: **ESPN runs `TlsVerify::None`** —
      transport encryption, no server auth. Damage ceiling accepted as
      wrong-game-data; strictly stronger than today's plain-HTTP data
      plane. The honest corollary, stated once: the parser and png-stream
      are now the security boundary for hostile payloads — evidence is the
      no-alloc bounded design, 149k-body sweep at 0 panics, and clean-Err
      overflow paths; S0's thin-fuzzing note inherits a little more weight.
      OTA plane: ed25519 carries the trust end-to-end as designed;
      `github.com` is all-ECDSA so full `rustpki` verification is free —
      take it there; `objects.githubusercontent.com` (RSA) rides the
      signature. No CA pinning, so no CA-rotation runbook — the only
      TLS-shaped OTA event left is embedded-tls capability drift.
- [x] Measure the handshake on silicon before believing any figure
      (2026-08-17 night): connect+handshake 132–198 ms LAN / 311 ms ESPN /
      ~400 ms github; kept-alive requests 50–310 ms vs cold fetches of
      0.6–6 s — keep-alive amortization confirmed load-bearing. The 8×-class
      surprise this time was elsewhere: bulk transfer is TCP-window-limited
      (1.5 KB rx buffer), not crypto-limited — the S3 rx-buffer sizing
      lever in BUDGET.md "Phase S2".
- [x] User-Agent: the spike sent the allowlisted prefix from
      `backend/config/default.toml` on every request — five real ESPN
      fetches answered 200, no 403s (2026-08-17).
- [x] Exit (2026-08-19): five real ESPN fetches, RAM delta and handshake
      times in BUDGET.md "Phase S2"; **28.5 h sustained soak of the
      fronted mock clean** (2,890 polls, 33 s median cadence, every TLS
      reconnect succeeded; only failure class = the known Wi-Fi
      association loss, one event self-recovered) and the PowerSave-off
      control answered (soak ran `PowerManagementMode::None`, throughput
      floors unchanged — window/RTT physics). Numbers folded into
      BUDGET.md "Phase S2". Merge to `main` at this boundary.

## S3 — the direct feed (device, still merge-clean)

Gate to start: S2 exit. Gate to exit: replay parity.

- [x] Direct feed over the shared transform crate (2026-08-19,
      code-complete): `crates/scoreboard-direct` (DetailStream +
      ListStream + the 404-vs-502 verdict fold, 33/33 golden parity +
      a list-parity oracle gate) under `poller/direct.rs` — same fetch-
      phase names and signatures as the wire twins, streaming transport
      in `net::espn` (one held TLS connection, TlsVerify::None per the
      S2 ruling, redial-once on an untouched sink). Soccer summary
      fetched for live games, best-effort (S1 decision taken). The
      warmer's probe class is DELETED: list rows carry team identity
      (abbreviations + crest paths), so the warm index fills from the
      list pass and `Step::Probe` marks missed without a fetch
      (S3-DESIGN decisions 8–13, all wave-2 design in that file).
- [x] `png-stream` crate (SPEC §14): landed at S1 validation; wired to
      the pool 2026-08-19 via `get_direct`/`prefetch_direct` — CDN
      combiner 100 px variant (~8.3 ms decode), black background blend
      matching backend `logo.rs`, decode-before-claim preserving the
      no-torn-slot rule, RGB565 layout verified at all three ends and
      const-asserted. (The 32 KB-window union with OTA scratch was NOT
      taken: `png_stream::Scratch` is its own 61.7 KB static — priced
      in BUDGET "Phase S3"; union is a shrink lever if RAM tightens.)
- [x] SNTP over UDP for the epoch (2026-08-18, sntp lane): portal codec
      host-tested, app transport `direct`-gated. `SOCKETS` is 12 under
      `direct`, not 10 — the held ESPN connection took the tenth slot
      and the margin was re-established (+704 B, table in `net`'s docs).
- [x] Browser-seeded timezone (2026-08-18, tz lane): endpoint + own
      sequential-storage key + manual override + SPA card; pure schedule
      logic in `scoreboard_config::timezone` (20 host tests); offset
      chain manual > schedule > GeoIP > None with `OFFSET_UNKNOWN`
      sentinel. SPA bundle re-baked 2026-08-19.
- [x] Polling economics stated in the module docs (`poller/direct.rs`):
      no upstream ETags — every poll pays a full streamed parse; the
      real number is S2's window/RTT law (sub-second at 16 KB rx), and
      the paper ~20 ms/300 KB was corrected 30× by the S1 silicon bench.
      Cadence respects `poll_interval_seconds` unchanged.
- [ ] Exit: full pregame→final captures for every sport replayed through the
      TLS-fronted mock, asserted against the parity harness — same goldens
      as proxy mode. Merge to `main` (additive: the direct feed exists,
      nothing uses it on `main`).
      Status 2026-08-19 (bring-up night): crate-level parity green
      (33/33 + list oracle). **The device replay RUNS**: bench unit on
      the TLS-fronted mock, all four sports committed and rendered
      through the live rotation (`mlb_live`, `final`, `football_live`,
      `nba_live`), 60 FPS, zero overruns, crests from the real CDN
      decoded on-device. The road there was three silicon-only stack
      failures, fixed with SP-probe evidence — BUDGET "Phase S3"'s
      settled-stack section is the record. Remaining for exit: a
      sustained replay window (running), golden-assertion pass against
      the captures, then merge to `main`. Field trial (real ESPN + real
      display + live soak) follows per the owner's ask.
      Update 2026-08-20: replay window done (~24 h through the mock,
      all four sports; one Wi-Fi-class stall reset by hand — the bench
      config had the watchdog off). **FIELD TRIAL RUNNING**: real-ESPN
      image flashed (no base override), live MLB committed and rendered
      from `site.api.espn.com` within a minute of boot, real CDN
      crests, a rotation of 6 real games across 3 sources; MLS then
      added and the watchdog ENABLED via `PUT /api/config` + reboot, so
      the association-loss class self-heals for the rest of the soak.
      The NFL-crest gap the owner saw in the replay was the
      trimmed-fixture artifact the list lane pinned (no `team.logo` in
      `testdata/football/nfl`), not firmware — real NFL bodies carry
      logos. First-press-after-reboot input bug fixed the same day
      (seed settles one debounce window; tracker heals a wrong seed).

## S4 — the standalone build and the deletion sweep (phase-s diverges here)

Gate to start: S3 exit. From this phase on, `phase-s` does not merge to
`main`.

Preparation that lands on `main` first (both worlds need it):

- [ ] Relocate `backend/testdata/` → repo-root `testdata/` and repoint the
      three outside consumers (verified 2026-08-16:
      `crates/scoreboard-model/src/tests.rs`,
      `crates/scoreboard-render/tests/*`, `tools/espn/mock*`), plus
      `firmware-rs` docs references.

On `phase-s`:

- [ ] Rewire the app: direct feed replaces `WireFeed`; the backend
      api-client path goes.
- [ ] Delete: `backend/` (whole tree, fly configs included),
      `crates/scoreboard-wire` (both consumers gone), `WireFeed` and the
      wire goldens (`testdata/wire/`), the backend-deploy /
      `publish-app` / staging paths in `tools/build.py`.
- [ ] Untouched on the branch: `firmware/` (MicroPython — `main`'s concern,
      deleting it here only buys merge pain), `tools/espn` + `infra/`
      (collector and mock are the validation rig, not the backend),
      `frontend/` (served from the device, both worlds).
- [ ] `publish-fw` → GitHub Releases: upload `image.bin` + `manifest.json`
      via `gh release`; channel mapping — `stable` = latest release
      (`releases/latest/download/…`), `dev` = fixed-tag pre-release
      (`releases/download/fw-dev/…`). Manifest fetch handles the **302
      redirect** to `objects.githubusercontent.com` (reqwless does not
      follow redirects; second TLS host, S2 already verified it).
- [ ] OTA client fetches over TLS (today's `/fw/*` is plain HTTP; the
      signature still carries the trust — TLS is just what the host speaks).
- [ ] Recovery runbook written: ESPN drift with no in-image fallback means
      OTA is the *only* recovery — keep the last-known-good proxy-world
      image published on a reachable channel so a standalone device can
      always be walked back to the backend world.
- [ ] Field trial: bench unit first; the seated unit only after its current
      soak completes and the owner calls it. Owner drives cutover, as ever.

---

## Cross-cutting rules (inherited, restated once)

- Budget: net delta estimate 70–80 KB (PHASE-S.md table); any static ≥ 1 KB
  updates BUDGET.md in the same PR; headroom target = 40 % **plus** the
  smarthome reservation.
- Crypto numbers are hostile until measured on silicon (BACKLOG 87 is the
  instrument if verify time starts mattering).
- Delegation, git, and device discipline: `PHASE-S-KICKOFF.md`, unchanged.
- House rules: no TODO comments (this file and BACKLOG.md are the list), no
  compat shims, benchmark before replacing designs that look wrong.

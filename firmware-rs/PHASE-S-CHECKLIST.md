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
- [ ] Adopt-or-write decision recorded as a SPEC Appendix A audit line.
      Verdict from the harness: **adopt, conditional on the lifetime fix** —
      preferred path is an upstream issue + PR with a `[patch.crates-io]`
      fork pin until merged (draft at
      `picojson-feasibility/ISSUE-DRAFT.md`, not filed — owner approves
      outward contact first).
- [ ] Exit: tokenizer choice + harness results written up; merge to `main`.

## S1 — shared transform crate (host-only)

Gate to start: S0 tokenizer chosen. Gate to exit: corpus parity with the
backend's transforms.

- [ ] New `no_std` crate (working name `scoreboard-espn`): declarative const
      path tables per sport
      (`$.events[*].competitions[0].competitors[*].score → field`),
      skip-unknown-by-default, every extracted value copied into bounded
      `Text<N>` storage (bounds from corpus + assert-on-corpus test).
- [ ] **Resolve the feed-seam shape** (design item, this phase):
      `GameFeed::detail` takes a complete `&[u8]` payload — a whole-payload
      contract a 300–450 KB streamed body can never satisfy. Recommended
      shape: stream-extract into an owned bounded per-sport extract struct,
      then hand `Store` borrowed views over that extract, keeping `Store`
      untouched. Decide against the path-table design; golden-test either way.
- [ ] Soccer commentary decision (flagged for owner visibility, resolve in
      design): live soccer commentary comes from ESPN's *summary* endpoint
      (390–456 KB per live game). Recommendation: fetch it — streaming makes
      the size a non-issue (~30 ms CPU) — rather than degrade the render.
- [ ] Backend migrates to the shared crate immediately (on the pre-S4 merge
      path, so `main`'s backend runs it against live traffic — the strongest
      parity instrument available, and standalone value if Phase S stalls,
      same as Phase 0's wire extraction).
- [ ] Exit: zero (or reviewed) transform diffs across the full corpus;
      backend deployed on the shared crate; merge to `main`.

## S2 — TLS bring-up (first on-device phase)

Gate to start: **BACKLOG 94 settled** — headroom re-earned to ≥40 % *plus*
the smarthome reservation, measured, not estimated. Gate to exit: handshake
and steady-state numbers in BUDGET.md.

- [ ] BACKLOG-94 levers, measured then chosen: Store-into-back-buffer fold
      (2,848 B), crest pool 32→16 (18,432 B), picoserve arena layout
      (`-Zprint-type-sizes`), flatten the route tree. Boxing is not a lever
      (no-alloc).
- [ ] embedded-tls via reqwless, against a TLS terminator fronting the
      `tools/espn` mock first (the mock speaks plain HTTP), then real hosts.
- [ ] **Protocol check, early**: embedded-tls is TLS 1.3-only. Verify 1.3 on
      all four hosts the standalone build will touch:
      `site.api.espn.com`, `a.espncdn.com` (logo hrefs are absolute),
      `github.com`, `objects.githubusercontent.com` (release-asset
      redirect target). Any 1.2-only host is a stop-and-redesign finding.
- [ ] Root CA strategy: pin multiple roots per host family; write the
      rotation runbook (a CA rotation is an OTA event).
- [ ] Measure the handshake on silicon before believing any figure — the
      house rule PHASE-S.md earned at 8× on the hash estimate. Keep-alive
      amortization is load-bearing, not nice-to-have.
- [ ] User-Agent: device sends the allowlisted prefix from
      `backend/config/default.toml` (ESPN's edge 403s unknown UAs).
- [ ] Exit: sustained TLS polling of the fronted mock + one real ESPN fetch
      from the bench unit; RAM delta and handshake time in BUDGET.md;
      merge to `main`.

## S3 — the direct feed (device, still merge-clean)

Gate to start: S2 exit. Gate to exit: replay parity.

- [ ] Direct feed over the shared transform crate: scoreboard JSON per
      configured league; summary fetch for live soccer (per S1 decision).
- [ ] `png-stream` crate (SPEC §14): streaming inflate (32 KB window,
      unioned with OTA scratch) → downsample → 24×24 RGB565 sprite; alpha
      handling matching backend `logo.rs` (premultiplied resize, background
      blend). Prefer the CDN combiner's size parameters over decoding
      500×500 sources — verify what the corpus hrefs support.
- [ ] SNTP over UDP for the epoch (one transient socket; `SOCKETS = 10` has
      room).
- [ ] Browser-seeded timezone (BACKLOG 95, decided): SPA posts the offset
      schedule (current offset, next DST transition instant, offset after);
      firmware endpoint + storage; manual override rides along. Storage
      shape recommendation: **own sequential-storage key**, by SPEC §9's own
      reasoning — different writer, different cadence, and a config `PUT`
      must not reset it. Write only on change.
- [ ] Polling economics stated in the module docs: no usable upstream ETags;
      every poll pays a full streamed parse (~20 ms/300 KB — fine); cadence
      respects today's `poll_interval_seconds`.
- [ ] Exit: full pregame→final captures for every sport replayed through the
      TLS-fronted mock, asserted against the parity harness — same goldens
      as proxy mode. Merge to `main` (additive: the direct feed exists,
      nothing uses it on `main`).

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

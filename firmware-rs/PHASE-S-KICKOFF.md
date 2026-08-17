# Phase S kickoff — orientation for the team taking this on

Written 2026-08-16, the day the OTA drill closed Phase 4 and the owner asked
for this handoff. You are building **direct-to-ESPN standalone mode**: the
device talks to `site.api.espn.com` itself — TLS, streaming JSON, logo PNGs —
and the backend proxy becomes a mode, not a dependency. The strategy,
verdict, budget math and risk register live in `PHASE-S.md`; this file is how
to start without stepping on anything.

## Read first, in this order

1. `PHASE-S.md` — the whole strategy, including the 2026-08-16 state of play
   and the decisions already made. Do not relitigate those here; argue with
   them in writing if you must, to the owner.
2. `SPEC.md` §1–2 (the crate-boundary rule: pure logic in `crates/*`, never
   imports embassy), §10 (dependency-audit criteria — the tokenizer audit is
   scored against these), §14 (Phase S's original framing).
3. `BUDGET.md` — top table and the "Drill day" section. **RAM headroom is
   36.6 %, already under the 40 % rule (BACKLOG 94), and Phase S wants
   70–80 KB.** The budget must be re-earned before your on-device phases
   land; your host-side phases are deliberately first.
4. `BACKLOG.md` items 87 (verify throughput — your crypto-perf instrument),
   90 (mDNS — unrelated, don't chase it), 94 (the headroom levers), 95 (the
   timezone decision).
5. `tools/espn/README.md` — the mock/replay rig. It is your validation
   backend and it already exists; the July staging infrastructure was
   unknowingly built for you.

## Decisions already made — build on these, don't reopen them

- **Direct is a mode, not a backend deletion.** The gift fleet and `/fw`
  hosting keep the backend alive regardless. Scope (a) of PHASE-S.md.
- **Wire format v2 is frozen** and in direct mode it is *deleted*, not
  ported — the second `GameFeed` impl produces model types straight from
  ESPN JSON. The seam is `crates/scoreboard-model/src/feed.rs`.
- **Timezone: browser-seeded offset schedule** (BACKLOG 95, recorded in
  PHASE-S.md). Not GeoIP, not a bare offset, not a tz database.
- **Clock: SNTP over UDP**, one transient socket.
- **Skip-unknown-by-default leniency** in the parser; ESPN drift is an OTA
  event and OTA is drilled, but leniency is the first line.
- **Crypto estimates are treated as hostile** until measured on silicon: the
  verify hash's paper estimate missed by 8×. Measure the TLS handshake
  before believing any number you read about it.

> **Correction, 2026-08-16 evening (owner):** two of the decisions below
> changed the day this document was written — "direct is a mode" became
> **branch-separated full removal**, and S4's dual-mode trial became the
> deletion sweep + standalone build. `PHASE-S-CHECKLIST.md` is now the
> normative sequencing and tracker; PHASE-S.md's closing section records the
> decisions. Everything else here — read order, delegation discipline,
> branch/commit rules — stands, with one refinement: S0–S3 merge to `main`
> at phase boundaries as written, and S4 is where `phase-s` stops merging
> back.

## Sequencing (each phase gates the next)

- **S0 — tokenizer audit.** `picojson-rs` first, against SPEC §10: no-alloc,
  caller-owned buffers, resumable across arbitrary chunk splits, license,
  fuzzing. Adopt or write the ~500-line state machine. Host-only.
- **S1 — shared transform crate.** Backend `transform.rs` per-sport logic →
  a `no_std` crate over the streaming abstraction, one declarative const
  path table per sport. Backend migrates to it immediately — standalone
  value even if Phase S stalls, same as Phase 0's wire extraction. Host-only.
- **S2 — TLS bring-up.** embedded-tls via reqwless, against a TLS terminator
  in front of the mock first (the mock speaks plain HTTP), then ESPN.
  First on-device phase; budget gate (BACKLOG 94) must be settled by here.
- **S3 — the direct feed.** Second `GameFeed` impl + `png-stream` logos +
  SNTP + the timezone seed. Validated by replaying full captures through
  the parity harness.
- **S4 — dual-mode field trial**, proxy primary, direct behind a config
  switch. Owner drives the cutover decision.

## Delegation discipline (the owner's explicit model — it is not optional)

- **The orchestrator validates everything**: read the actual diff, run the
  tests, check the idiom. An agent's report is a claim, not a fact — the
  doc-sweep standard is the bar: every number measured-or-flagged, with the
  artifact it was measured from named.
- Opus-class agents take concrete, well-scoped tasks; the two genuinely hard
  subsystems — mid-token resumability in the tokenizer, the TLS bring-up —
  get the strongest model available. Brief richly, keep agent contexts
  short-lived, give disjoint file lanes.
- **Agents do not run git.** The orchestrator commits after validation, with
  explicit pathspecs (`git commit -- <paths>`) — a plain `git commit` from a
  shared checkout once swept another agent's staged files into an unrelated
  commit. For heavier parallelism, use worktrees instead of sharing.
- **Agents never touch the live device, the probe, fly, or any publish
  pipeline.** The production scoreboard is in its soak. S0–S1 need none of
  it; when S2 needs hardware, the orchestrator runs the bench, and anything
  involving the seated unit goes through the owner.

## Branch and commit rules

- Branch **`phase-s` off current `main`** (`main` is the branch of record;
  the old `rust-firmware` branch is history). All Phase S work lands there.
- **Globally-applicable fixes go straight to `main`**, not the feature
  branch — the owner's standing rule. Find a bug in shipping code while
  you're in there: fix it on `main`, then merge `main` into `phase-s`.
  Keep the branch mergeable; don't let it drift for weeks.
- Merges back to `main` happen at phase boundaries, with the parity/golden
  gates green and the numbers in the commit message.
- CI runs clippy with `-D warnings` on **both link profiles**. Code reachable
  from only one profile needs the `cfg_attr(allow(dead_code, reason))`
  house pattern (`ota/mod.rs` has examples) or the standalone lane fails.
- House rules that are already law: no TODO comments (BACKLOG.md is the
  list), no legacy/compat shims (pre-release — delete and refactor),
  benchmark before replacing any design that looks wrong, and if the
  "wrong" design wins the benchmark, consider making it the standard.

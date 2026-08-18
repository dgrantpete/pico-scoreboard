# S3 — the direct feed: design and lane plan

Written 2026-08-18, the night S2's numbers landed and the owner called for
code completion to pixels. This is the working design for S3
(PHASE-S-CHECKLIST.md is the tracker; this file is the how). House rules
apply unchanged: no TODOs, no compat shims, measure before believing,
agents never run git or touch hardware.

## The architecture in one paragraph

The display half of the firmware does not change at all. `GameDetail<'a>`
(scoreboard-model/src/feed.rs) wraps *borrowed* scoreboard-wire structs,
and `Store` copies what it keeps — so direct mode stream-extracts each
ESPN body into an **owned, bounded, per-sport Extract struct** and hands
`Store` the same borrowed `GameDetail` views it gets today, built over the
extract instead of over a decoded wire payload. `Store`, the snapshot
channel, core 1, and every renderer are untouched by construction; the S3
exit gate (same goldens as proxy mode) is the proof.

## Decisions (locked here, argue in writing)

1. **Compile-time selection, no runtime flag** (the owner's standing
   branch-separation decision): a cargo feature `direct` on
   `firmware-rs/app` selects the direct fetch path in the poller. Default
   builds keep `WireFeed`; `main` merges S3 additively with nothing using
   it. `GameFeed` stays as-is for the wire world; the direct path does not
   impersonate it (its whole-payload `detail()` contract is the thing
   streaming exists to avoid).
2. **URL construction**: `LeagueId.key` ("baseball/mlb",
   "soccer/usa.1") is exactly ESPN's path segment. Scoreboard:
   `https://site.api.espn.com/apis/site/v2/sports/{key}/scoreboard`.
   No new registry.
3. **One fetch, two extractors**: ESPN's scoreboard body carries the whole
   slate *and* every game's detail. Per poll, the body streams once and
   each chunk feeds both the list extractor and the detail extractor
   (validate-until-found on the displayed game). Parse CPU roughly doubles
   (S1/S2 numbers say that is fine); a second 6-second-class fetch is
   avoided. Revisit only if silicon disagrees.
4. **Soccer commentary**: fetched, per the standing S1 recommendation —
   the summary endpoint streams like everything else and the extractor
   already exists (39,649 real summaries clean in the S1 sweep). The
   S1 checklist's open owner item resolves this way unless vetoed.
5. **Crests**: fetch the 100 px CDN combiner variant, decode with
   `png-stream` (~8.3 ms), blend per backend `logo.rs` semantics, into the
   existing pool under the existing `{league key}/{abbreviation}` keying.
   How the URL is obtained is the crest lane's survey-first question
   (extractor exposure vs constructible pattern) — smallest correct
   change wins.
6. **TLS posture** (owner-ruled, checklist S2): `TlsVerify::None` to ESPN
   hosts, fork-pinned embedded-tls. The poller's receive window grows to
   16 KB (the S2 window/RTT finding) — a BUDGET line in the same commit
   that lands it.
7. **Clock**: SNTP over UDP, one transient socket. **Timezone**: BACKLOG
   95's browser-seeded offset schedule; own sequential-storage key (SPEC
   §9 reasoning — different writer, different cadence; a config PUT must
   not reset it); write only on change.

## Lanes

Wave 1 (host-side, parallel, disjoint):

- **extracts** — `crates/scoreboard-direct`: per-sport Extract structs +
  sink impls over `scoreboard-espn` extractors + `GameDetail` view
  construction. Parity gate: over `backend/testdata`, extract-then-view
  must equal WireFeed-decoding of the committed wire goldens, field for
  field. The backend's `backend/src/*/adapter.rs` files are the
  authoritative mapping references.
- **crest-hrefs** — make the crest URL reachable in direct mode
  (survey-first: what does `backend/src/logo.rs` + `team.rs` actually use;
  extend `scoreboard-espn` tables only if construction is impossible).
  Corpus-validated like every table change.
- **sntp** — `net::sntp` in the app plus the timesync phase swap, behind
  `direct`.
- **tz** — the BACKLOG 95 endpoint + storage key + manual override in the
  app, and the SPA posting the offset schedule. Frontend and firmware in
  one lane so the contract can't drift mid-flight.

Wave 2 (after extracts): the poller's direct fetch path (orchestrator's
lane — it touches the single-owner poller contract and the crest pool's
cross-core rules), the crest PNG pipeline integration, replay parity
through the TLS-fronted mock, then the bench-unit field trial the owner
asked for: real ESPN, real display, soak on live data.

## Validation ladder

Host: crate suites + the extract-vs-wire-golden parity test + clippy both
profiles. Replay: full pregame→final captures for every sport through the
TLS-fronted mock, asserted against the same goldens as proxy mode. Then
silicon, then the display soak. Numbers land in BUDGET.md as they are
measured, never estimated.

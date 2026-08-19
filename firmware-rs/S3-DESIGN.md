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
3. **One fetch, two extractors — now per-sport** (amended 2026-08-18 on
   the extracts lane's measurements): concurrent list+detail extractor
   state is fine for MLB (14.8 KB), NBA, and football (21.6 KB), but
   soccer's bounds put it at **62.0 KB before scratch** — roughly twice
   the whole BACKLOG-94 re-earn. (Sizes are device-measured and
   const-asserted in scoreboard-direct; post-crest-exposure the peak
   under sequential fetching is ~31.0 KB. Device and host sizes differ —
   the crest option costs 144 B/extract on thumbv8m, not the host's 160 —
   BUDGET.md takes device numbers only.) And the trade that motivated
   concurrency has shifted: with S2's 16 KB receive window a fetch is
   ~1 s, not ~6 s. So: concurrent extraction for MLB/NBA/football,
   **sequential two-fetch for soccer** (list pass, then detail pass) as
   the wave-2 default, with the alternative lever — shrinking soccer's
   SCORING_MAX/LIST_MAX bounds toward corpus reality (7 and ~15 observed
   vs 32 and 64 allocated) — priced during integration, measured not
   estimated, and taken only if the RAM is needed elsewhere.
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

- **extracts** — `crates/scoreboard-direct` (LANDED 2026-08-18, 33/33
  golden parity, workspace 734 green). Thinner than briefed, correctly:
  S1's extract structs + `as_game()` views already were the owned seam,
  so the crate supplies only the genuinely missing layer — `DirectExtract`
  sport dispatch into `GameDetail`, `DetailStream` unifying four divergent
  extractor surfaces, the backend's 404-vs-502 verdict fold (the device's
  "game ended" signal), the soccer two-body commentary seam, and the
  parity gate with a loud-comparator negative control. Structural
  finding for the S1 API-unification pass: a uniform *list* stream is
  blocked until the list extractors converge on owning their sinks
  (football's shape); until then the poller dispatches the four list
  extractors directly. Measured extract sizes live as const-asserts in
  the crate; the soccer number amended decision 3 above.
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

## Wave 2 — the poller's direct path (decided 2026-08-19, orchestrator's lane)

8. **The probe dies with the list pass.** The wire backend's games list
   carries `(id, state)` only, which is why the warmer's probe exists: a
   whole detail fetch to learn two abbreviations. ESPN's scoreboard body —
   the body the list extractors already stream — carries every
   competitor's abbreviation and logo href. So the list extractors grow
   best-effort per-event team identity (abbreviations + crest CDN paths;
   never a validity gate — an event with missing markers still lists with
   the extras empty), the poller's list pass feeds `WarmIndex::learned`
   and a crest-path index for free, and `Step::Probe` in a direct build
   answers `warm.missed()` without a fetch: a game whose list row lacked
   identity retires after the standing give-up count and gets its crests
   the normal way when the rotation reaches it. A ~300–450 KB summary
   fetch per unshown game becomes zero fetches.
9. **The crest-path index is the app's, not the model's.** `WarmIndex`
   stays exactly as it is — wire builds have no paths to remember, and
   the model's contract does not grow a direct-only field. The app keeps
   a bounded `{league key}/{abbreviation}` → `CrestPath` map
   (direct-gated, pool-sized), refilled by every list pass, so a stale or
   evicted entry self-heals within one refresh. A warmer crest step whose
   path is missing marks the game missed — same vocabulary as every
   other unfetchable thing.
10. **One transport seam.** `net::espn::EspnClient::fetch(url, sink)`
    streams the body chunk-by-chunk into a `&mut dyn FnMut(&[u8]) -> bool`
    (one monomorphization — the picoserve future-duplication lesson,
    BUDGET.md addendum). 404 is a value (`Fetched::NotFound`), matching
    the wire client's "game left today's scoreboard" arm. Keep-alive to
    the last host, reconnect-once-on-failure — the 28.5 h soak's proven
    recipe.
11. **Extract and scratch live in StaticCells, never the task arena.**
    `DirectExtract` (soccer sequential peak ~31.0 KB device) and the
    16 KiB picojson scratch are taken once at poller start. The commit
    borrow shape is preserved from wire mode: crest paths are copied out
    of the extract *before* the crest fetches await, and the
    `detail()` borrow spans only the synchronous commit — no borrow
    across an await, same rule, same reason.
12. **Refresh ticks decide list strategy per decision 3**, with one open
    item to measure, not estimate: whether the list extractor's scratch
    can be small (captured list tokens are ≤ 255 B; whether *skipped*
    tokens transit scratch is a picojson property to verify on the
    corpus). If a second 16 KiB scratch is the price of concurrent
    list+detail, sequential two-fetch for every sport on refresh ticks
    is the fallback decision 3 already prices.
13. **The replay lever is the spike's.** The ESPN base URL is a
    compile-time override (env at build, exactly `SPIKE_TERMINATOR`'s
    pattern) so the same `direct` image points at the TLS-fronted mock
    for replay parity and at real ESPN for the field trial, with no
    runtime switch to ship by accident.

## Validation ladder

Host: crate suites + the extract-vs-wire-golden parity test + clippy both
profiles. Replay: full pregame→final captures for every sport through the
TLS-fronted mock, asserted against the same goldens as proxy mode. Then
silicon, then the display soak. Numbers land in BUDGET.md as they are
measured, never estimated.

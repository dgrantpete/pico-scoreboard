# scoreboard-espn — the shared ESPN transform (Phase S1)

Written 2026-08-16 by the S1 orchestrator, before any implementation. This
is the design the lanes build against; deviations go through the
orchestrator. Strategy context: `firmware-rs/PHASE-S.md`; tracker:
`firmware-rs/PHASE-S-CHECKLIST.md`.

## What this crate is

One `no_std` implementation of the ESPN-JSON → game-model transform, shared
by the backend (today) and the firmware's direct feed (S3) — the same move
`scoreboard-wire` made for the wire format in Phase 0. The ESPN-specific
knowledge lives in one declarative const path table per sport plus the
semantic mapping around it; everything else is a small streaming engine.

```
crates/scoreboard-espn/
├── DESIGN.md        this file
├── src/lib.rs       crate docs, re-exports
├── src/path.rs      the engine: path patterns + matcher over picojson events
├── src/{mlb,nba,football,soccer}.rs   per-sport: extract structs, PATHS
│                    table, sink with the semantic mapping
└── tests/           corpus extraction + chunk-split invariance
```

Dependencies: `picojson` (=0.2.3 + the PR #98 lifetime fix, pinned by rev
via the workspace `[patch.crates-io]` until upstream releases it),
`heapless`. Nothing else. `#![no_std]`, no alloc, SPEC §10 applies.

## The parity contract (what "correct" means)

**Byte-identical wire output.** For every fixture in `backend/testdata/`,
running the fixture through this crate's extraction and encoding the result
with `scoreboard-wire` must produce the same bytes the backend's existing
`types.rs → transform.rs → wire.rs` pipeline produces. That harness lives in
`backend/` as a dev-dependency test (the backend is the only place both
pipelines exist under `std`). Reviewed-and-accepted diffs are the only
escape, same as Phase 2's golden rule.

Corollaries the implementer inherits:

- Truncation must happen where and how the backend does it today, with the
  same limits (the inventory reports capture where that is, per field).
- Extract-struct string bounds are wire limits or measured corpus maxima
  plus margin, whichever the inventory justifies — cited in a comment at
  the bound, per SPEC §10.
- JSON-DTO-only fields (served by the backend's JSON representation but
  never wire-encoded) ARE in scope: the backend must be able to build its
  DTOs from extracts alone, or S1's exit (backend deployed on this crate)
  is unreachable.

## The extract structs (the seam, resolved)

Per sport, an owned bounded struct (`heapless`-backed `Text<N>` semantics,
plain ints/enums) holding the post-transform fields — domain-shaped, not
raw-ESPN-shaped. Two consumers:

- **Backend (S1)**: thin adapter Extract → existing domain structs;
  `wire.rs`, handlers, and DTOs unchanged; per-sport serde `types.rs` is
  deleted. Lenient parsing is preserved by the engine's skip-unknown
  default plus per-field Option/default rules from the inventory.
- **Firmware (S3)**: `impl Extract { pub fn as_game(&self) -> wire::Game<'_> }`
  — a borrowed wire-shaped view over the extract's own storage, so `Store`
  and `GameDetail` are untouched. `GameFeed::detail(&[u8])` is *not*
  implemented by the direct path (a 300–450 KB streamed body never exists
  as a slice); the extract struct + `as_game()` IS the seam. Recorded for
  S4: when the wire codec is deleted on `phase-s`, the `Game` struct
  *definitions* should be re-homed (wire = codec dies, vocabulary
  survives) rather than rewriting `Store` — decide there, not here.

Streaming forbids borrowing from the receive buffer, so every extracted
value is copied into the extract immediately — the discipline PHASE-S.md
already noted is exactly the `Text<N>` shape.

## The engine (src/path.rs) — requirements, not prescriptions

The implementer owns the mechanism; these are the contract:

- **Input**: picojson `PushParser` events (the S0-audited tokenizer;
  `DefaultConfig` = 32 nesting levels, ESPN needs ~15 — assert headroom in
  a test). The engine itself is sport-agnostic.
- **Patterns**: const tables of segments — `Key("competitions")`,
  `Index(0)`, `AnyIndex` — e.g.
  `$.events[*].competitions[0].competitors[*].score`. Matching is against
  the current structural path; nothing about unmatched subtrees is stored
  (skip-unknown is the default by construction). No key text is buffered:
  incoming keys are compared against the active states' next segments.
- **Sink callback**: on a matched leaf, the sink receives the pattern
  index, the concrete indices bound to each `AnyIndex` (u16 — a college
  Saturday slate can exceed a u8), and the value (`Str`/raw `Num` text/
  `Bool`/`Null`). Values are valid only for the duration of the call.
- **Element boundaries**: sinks must be able to finalize per-element state
  for arrays of objects (close out game N when `events[N]` ends). The
  engine provides boundary notification for patterns that designate
  containers; the mechanism is the implementer's choice.
- **Skip directive**: a sink may answer "skip the enclosing element" (used
  to fast-forward events whose id is not the requested game). The engine
  must skip without matching work until the depth unwinds.
- **Bounded everything**: state is O(patterns × depth) in small ints/bits;
  no allocation; caller-owned scratch only (picojson's own scratch buffer
  is provided by the caller).
- **Chunk-split invariance is inherited, then proven again**: S0 proved the
  tokenizer; the engine's tests must prove the *extract* level — whole-buffer
  vs 1-byte-at-a-time vs random chunkings produce identical extracts over
  the corpus (same methodology as `repos/picojson-feasibility`).

## Decisions recorded here so nobody relitigates them mid-lane

- **Soccer commentary**: direct mode fetches the per-event summary for live
  soccer, streamed like everything else (~450 KB is a non-issue streamed;
  the CPU is ~30 ms). The soccer lane's tables cover both the scoreboard
  event and the summary's commentary paths. (Owner may veto; recorded in
  the checklist as resolved-by-design.)
- **Numbers**: picojson default features (`int64` + `float`). ESPN encodes
  many numerics as JSON strings; per-field handling comes from the
  inventory, and `Value::Num` hands the sink raw text so nothing is lost.
- **One table serves both deployments.** No device-side feature-gating of
  DTO-only fields in S1; revisit at S3 only if a field measurably hurts.

## Rulings (2026-08-16, after the football + NBA inventories)

The inventory reports (session scratchpad, `s1-inventory/*.md`) converged on
a set of cross-sport questions. Answered here once so no lane relitigates:

1. **Rejection parity is part of the contract.** The backend's per-state
   required-field rules (a pregame event missing `displayClock` is dropped;
   the games *list* is the same parse) must be reproduced: extract finalize
   validates the same required set and skips the event exactly where the
   backend's DU conversion would. Byte parity on transformed games does not
   imply set parity on the list — we owe both. Likewise the two-tier error
   model survives: DU-tier failure = skip event; transform-tier failure
   (bad color hex, unparseable score/date) = extract error the backend
   adapter maps to today's 5xx.
2. **Extract strings are bound at `MAX_STRING_BYTES` (255), never tighter.**
   The backend truncates strings only at wire encode (char-boundary walk in
   `codec.rs`); a tighter extract bound would silently diverge. The copy
   into the extract truncates with the *same shared function* —
   `scoreboard-wire`'s `truncate_utf8` becomes `pub` and both call it (one
   implementation, zero drift). Structurally-bounded fields (colors,
   scores) keep their integer types. `scoreboard-model`'s tighter `Text<N>`
   bounds are downstream at `Store` copy-in and unchanged.
3. **`start_time` parsing is hand-rolled, not a date crate**: fixed-width
   parse of the two observed formats + days-from-civil epoch arithmetic
   (~40 lines of pure `core`), property-tested against `chrono` in a std
   dev-test. A date dependency for one field fails the edge-mdns
   proportion test.
4. **Field-order independence is a hard rule.** serde never depended on
   ESPN's key order; the tables may not either. Cross-field logic
   (possession → `Side`, home/away by marker never by index) resolves at
   element finalize from buffered ids. No sport table may assume emission
   order inside an object.
5. **Warn-only duplicate clamps are dropped** (the wire-side clip is
   authoritative and byte-identical); the *diagnostic* survives as ruling 6.
6. **Diagnostics**: the crate emits structured quirk events (unknown phase
   label, malformed record, clipped line score, …) through a small
   caller-wired callback — backend routes them to `tracing`, the device to
   its log ring. An enum with minimal payload; no formatting in the crate.
7. **Behavior changes are out of scope, all of them.** Line-score period
   gaps, NFL tie-record drops, OT phase labels, absent-vs-explicit-null
   leniency, device-side bad-color policy: S1 reproduces today's behavior
   bit for bit; each becomes a BACKLOG candidate for the owner. (Device
   error *policy* is S3's layer anyway.)
8. **`is_college` is a call parameter** of the football extractor, not a
   table row; conditional logic gates in the sink.
9. **The parity oracle is the existing golden harness.**
   `backend/src/wire_corpus.rs` already pins transform→wire bytes to
   committed goldens over every fixture; S1's acceptance test runs the
   shared crate over the same fixtures against the same goldens. Corpus
   gaps (no OT, no non-ASCII play text, synthetic-only football) are
   surfaced to the owner as a capture/synthesis request — the gate extends
   automatically as fixtures land.
10. **Unicode uppercasing** (the college rank line) uses `core`'s
    `char::to_uppercase`; a std dev-test property-checks byte-identity
    against `str::to_uppercase` over the corpus plus targeted non-ASCII
    names.

Added after the MLB + soccer inventories, same day:

11. **Hex colors parse with `core`'s `u32::from_str_radix`** after the
    identical `#`-strip and exactly-6 check — the backend's own function,
    available in `no_std`, so quirks like the accepted leading `+` carry
    over by construction instead of by re-implementation.
12. **The datetime parse replicates chrono's flexibility**: both observed
    formats AND 1-or-2-digit numeric fields (`"2026-7-8T1:40Z"` parses
    today); the chrono property test covers those forms explicitly.
13. **Extractors report ok/failed event counts**, because
    `find_event`'s 404-vs-502 rule (a glitched scoreboard must never look
    like "game ended") needs the failure count — on the backend today and
    on the device at S3. And every sport lane must pin the tie-break
    semantics with named tests: `max_by`/`max_by_key` keep the LAST of
    equal maxima, sorts are stable — the soccer goldens encode both, so a
    fold-based max or `sort_unstable` silently breaks bytes.

Added as the sport lanes landed:

14. **Detail mode validates until the target is found, then skips**
    (REVISED same day — the football lane's deviation is adopted
    crate-wide, superseding the skip-at-id policy and its backend
    workaround). Rationale: `SkipElement` never skipped *tokenization*,
    only matching work, so blind skipping bought almost nothing — while
    validate-until-found makes the failure counts exact precisely when
    the 404-vs-502 verdict consumes them: a missing target means nothing
    was ever skipped. Post-target events are skipped and uncounted, which
    is fine — the verdict is already Found. Pinned by football's
    `events_after_found_target_are_skipped` (both directions); MLB, NBA
    and soccer retrofit to the same policy and pin it the same way. The
    backend adapter no longer needs a separate full-validation pass.
15. **Local-helper promotion happens once, after all four lanes land** —
    the duplicated serde-shape integer parsers, `wire_phase`, and the
    stable insertion sort consolidate into `common.rs` in one
    orchestrator pass, so mid-flight lanes never chase a moving target.
16. **Compare keys refuse to truncate (`common::Exact`), crate-wide.**
    Two storages for >255-byte compare keys existed: truncate-at-bound
    (genuinely-equal keys stay equal, but distinct keys sharing a
    255-byte prefix silently *match* — wrong possession bit, wrong game
    served) and `Exact`'s refuse-to-store (equality checks fail visibly,
    the situation drops, the game resolves NotFound). Both diverge from
    the backend's unbounded compare only in absurd-land, but a false
    positive is wrong data and a false negative is safe degradation —
    `Exact` wins. Applies to detail-target ids, `team.id`, possession
    ids, and any future key whose equality gates a byte.
17. **Betting lines never fail an event** (2026-10-12). Odds are the first
    field with no serde-era predecessor and they only decorate the pregame,
    so ruling 1's rejection parity has nothing to match: a wrong type, an
    empty, overlong or non-numeric string, or a missing side leaves the
    lines out and the event whole (`common::OddsScratch`). Only
    `odds[0]`'s `close` numbers are read; a second sportsbook never stands
    in for the first. Football shows them only when neither team is ranked
    (ranks or lines, never one of each); the NBA's ride the wire's `ext=1`.

## The hockey lane (2026-10-14, NHL.md phase 6.2)

The fifth sport, built after the parity contract had done its work: hockey
has no serde predecessor, so rulings 1 and 9 have nothing to compare
against and the lane's correctness is `firmware-rs/NHL.md` §3, which is
normative, plus the corpus under `backend/testdata/hockey/nhl/` with the
evidence in its `PROVENANCE.md`. Everything else — field-order
independence, chunk-split invariance, bounded memory, quirks, the ok/failed
counts, `Exact` compare keys — applies unchanged. What is specific to the
lane, decided here so nobody relitigates it:

- **Two documents, one combiner.** The scoreboard event feeds
  `hockey::GameExtractor` / `ListExtractor` (soccer's split); the per-event
  summary feeds `hockey::SummaryExtractor`, for live *and* final games
  (NHL.md D3: strength, goals, flash and shootout live; the reliable shots
  on finals). Both produce owned extracts and
  `Extract::as_game(&self, Option<&Summary>)` borrows the wire view from
  them; without a summary the view is the scoreboard alone.
- **The summary extractor takes a `SummaryContext`** built by
  `Extract::summary_context()` from the scoreboard extract — the season
  kind, the state, and whether a live shootout is on. It must know the
  season kind *while streaming*: a period-5 scoring play is a goal in the
  playoffs and a shootout attempt in the regular season, and ruling 4
  forbids waiting for the summary header to say which. Football's
  `is_college` precedent (ruling 8); the backend extracts the scoreboard
  first anyway, and the context has no other constructor.
- **Required fields, written once** (the module docs carry the full list):
  `id`, `date`, `competitions`; with a competition `status.type.state`,
  exactly two competitors each with `homeAway`, `team.id`,
  `team.abbreviation`, `team.color`; live adds `status.period` and
  `status.displayClock`; live and final add each `score`. Missing or
  wrong-typed drops the event (counted failed). A found target is rejected
  (`TransformError`, the 5xx tier) on an unparseable color or score, a
  pregame date that is not a start time, or two home/two away markers.
  `alternateColor` is optional — ESPN sent none for the NHL through
  2026-10-14, then one for every team from 23:44 UTC that day — so absent
  (or not a string) reads as the primary and only a string that is not six
  hex digits rejects. A `linescores[*]` entry without a numeric `value`
  drops the event in every state — the NBA lane's rule, since skipping it
  would shift the periods after it. Everything else degrades; nothing in a
  summary fails a game.
- **The summary table** reads `plays[*]` (`id`, `type.{id,penaltyType,
  penaltyMinutes}`, `text`, `period.number`, `clock.displayValue`,
  `team.id`, `scoringPlay`, `participants[*].{type,athlete.id,
  athlete.shortName}`, `coordinate.{x,y}`, `strength.id`, `shotInfo.id`),
  `onIce[*].{teamId,entries[*].athleteid}`,
  `boxscore.players[*].{team.id,statistics[*].name,
  statistics[*].athletes[*].athlete.id}` (the goalie groups),
  `boxscore.teams[*].{team.id,statistics[*].{name,displayValue}}`
  (`shotsTotal`) and `header.competitions[0].competitors[*].{team.id,
  homeAway}` — the one team-id → side mapping every other summary
  reference resolves through, at finish.
- **Bounds** (each documented at its constant, overflow degrades): goals,
  the newest 16 (the wire's `MAX_GOALS`); shootout attempts, the first
  `ATTEMPT_MAX` = 40 in sequence (20 rounds, the longest NHL shootout),
  `BoundedOverflow` past it; `ATTEMPT_ATHLETES` = 3 participant ids per
  attempt for the goalie fallback, extras dropped silently; `ON_ICE_IDS` =
  8 athlete ids per `onIce` side (more entries are insane whether or not a
  goalie is among them); `GOALIES_PER_TEAM` = 4, a fifth is a quirk;
  `LISTED_PER_TEAM` = 32 listed players per team (a dressed roster is 20),
  past which the relief-goalie inference is suspended for that team with a
  quirk — an incomplete listing must never make a real skater read as the
  goalie and hide an empty net; a third `onIce` block, player group,
  boxscore team or header competitor is a quirk and ignored; line scores
  at the wire's `MAX_LINE_SCORE`; the games list at `LIST_MAX` = 32 (16
  games a night, 2×).
- **Known goalies** (§3.4, 2026-10-15): the boxscore goalies group plus
  the scoreboard's probable starting goalie, whose `athlete.id` rides the
  `SummaryContext`; a goalie is present when a known one is on the ice or
  when an on-ice id is not a listed player of its team (the relief goalie
  before the boxscore lists him). A goalie-less unit (2026-10-16) is not
  yet a pull: ESPN omits on-ice goalies and freezes a pulled unit after
  the goal that ended it, so it is believed pulled only when the count fits
  (six attackers, five when trailing in period ≥ 3, more than the
  opponent's skaters in the regular season's overtime) and the context
  fits (the team trails, or a penalty is pending against the other side —
  the opponent's newest penalty since the last face-off with no face-off,
  period boundary, goal or later-clock row after it); otherwise the goalie
  is presumed omitted and the unit capped at five. The pending penalties
  are tracked per team id as the plays stream and resolved to sides at
  finish, like everything else. An intermission's pulled reading is the
  frozen last unit and yields no strength.
- **Quirks** (`common::Quirk`): `OnIceInsane`, `FinalDetailMismatch`,
  `UnknownStatusName`, `TiedFinal`, `UnplaceableAttempt`, `UnplaceableGoal`,
  plus the shared `MalformedRecord` (W-L-OTL), `ClippedLineScore` and
  `BoundedOverflow`. NHL.md §3 names the cases that stay quiet: `onIce`
  `[]` at a period start, the shootout and an intermission's pulled reading
  (no strength at all), the 143 placeholder and minutes-less penalty-shot
  calls (expected shapes, not quirks); puck drop's empty goalie groups are
  covered by the probables and compute a real strength. Two empty nets at
  once is insane, not an empty net. A shootout
  attempt the play itself cannot place takes its side from the nearest
  placed attempt in either direction — strict alternation determines it —
  and is dropped only when there is none.

## Lanes and validation

- Engine lane: `src/path.rs` + its tests. Hard subsystem — strongest model,
  richly briefed, reviewed line-by-line by the orchestrator.
- Sport lanes (×4, disjoint files): `src/{sport}.rs` + corpus tests, built
  on the frozen engine API and the inventory reports.
- Backend migration lane: adapter + `types.rs` deletion + the byte-parity
  harness, after sport lanes land.
- The orchestrator reads every diff, runs every test, and commits with
  explicit pathspecs. Agents do not run git (kickoff rules).

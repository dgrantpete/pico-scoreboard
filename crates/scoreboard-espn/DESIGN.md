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
10. **Unicode uppercasing** (`rank_line`) uses `core`'s
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

## Lanes and validation

- Engine lane: `src/path.rs` + its tests. Hard subsystem — strongest model,
  richly briefed, reviewed line-by-line by the orchestrator.
- Sport lanes (×4, disjoint files): `src/{sport}.rs` + corpus tests, built
  on the frozen engine API and the inventory reports.
- Backend migration lane: adapter + `types.rs` deletion + the byte-parity
  harness, after sport lanes land.
- The orchestrator reads every diff, runs every test, and commits with
  explicit pathspecs. Agents do not run git (kickoff rules).

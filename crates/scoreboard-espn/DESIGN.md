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

## Lanes and validation

- Engine lane: `src/path.rs` + its tests. Hard subsystem — strongest model,
  richly briefed, reviewed line-by-line by the orchestrator.
- Sport lanes (×4, disjoint files): `src/{sport}.rs` + corpus tests, built
  on the frozen engine API and the inventory reports.
- Backend migration lane: adapter + `types.rs` deletion + the byte-parity
  harness, after sport lanes land.
- The orchestrator reads every diff, runs every test, and commits with
  explicit pathspecs. Agents do not run git (kickoff rules).

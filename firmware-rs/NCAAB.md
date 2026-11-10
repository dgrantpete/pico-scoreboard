# NCAA Basketball — end-to-end spec

Status: **decided** (owner, 2026-11-08/09: the decisions page "NCAA Basketball
Decisions", answers A1–A11, and the mockup gallery "NCAA Panel Mockups"
reviewed live on the bench panel). The engineering choices marked
**proposed** wait for the owner's veto. **Not started**: phases B0–B9 follow.
The owner authorised the release to production (backend, then the stable
firmware channel) once every phase is verified (2026-11-09: "make sure we
vouch stuff first").
Every mockup was drawn by a copy of the real `scoreboard-render` crate over
real ESPN plays (`data/ncaab-design/`, local), and the angled court was
reviewed pixel by pixel on the panel through the diag build.

Read order for an implementer: §1 decisions → §2 data facts → §3 semantics
(normative) → the phase you own in §6. §7 is cross-sport work this design
found; each item is its own change.

---

## 0. Scope

Add NCAA men's basketball (ESPN `basketball/mens-college-basketball`,
display name "NCAA BASKETBALL") as basketball's second league, the way NFL
and NCAA Football share football: one sport, one wire format, one set of
screens, the league in the endpoint, per-league knowledge resolved once at
the edge. Basketball becomes multi-league end to end (extractor → wire →
backend → model → render → config → menu → frontend), and both leagues move
to a new **angled court** (D5) that replaces NBA.md's top-down B5 court.

Out of scope: NCAA women's basketball (its scoreboards are collected since
2026-11-09; with this design it is a registry row and a rules row), WNBA
(BACKLOG 51), a college foul/timeout ledger derived from plays (D3; a
BACKLOG candidate), hockey's dots (BACKLOG 123), the indicator vocabulary
(BACKLOG 122), football play coordinates (BACKLOG 121).

## 1. Decisions

| # | Decision | Decided | Not chosen |
|---|----------|---------|------------|
| D1 | One sport | **`Sport::Basketball` with two leagues**, the football shape. NBA → Basketball is renamed first, byte-identically (owner, 2026-11-08) | `Sport::Nba` serving college (every name lies); a second sport |
| D2 | Per-league rules | **A product type of independent properties, resolved from the league once, at the edge** (owner, 2026-11-09). Device: `BasketballRules { periods, court, ledger }` — `periods` a private names table (`QUARTERS` = Q1–Q4, `HALVES` = 1ST/2ND; regulation = its length), `court` the rule-book lines in feet, `ledger` whether the league's screen has foul/timeout tracks. Extractor: a properties struct built from `BasketballLeague` (rank line, flash table, ledger lane, frame shift, regulation) — the `is_college` pattern. NCAA women's (quarters on the NCAA court) is why the axes stay independent | A `League` sum type matched inside the code; device-side per-league `match`es; carrying the format on the wire (spends an extension level to say what the endpoint already says) |
| D3 | College fouls/timeouts | **None**: ESPN publishes zeros (§2). The league has `ledger: false`, so its band has **no** foul or timeout tracks and its scores are **re-centred** on the crests (V2; owner, 2026-11-09). Layout keys on the league property, never on per-poll presence: the NBA keeps its tracks through a missing poll | Derive from plays (encodes NCAA bonus/timeout rules; BACKLOG); V1 rows left dark; V3 unlit placeholders (reads as a false zero); V4 AP rank in the freed rows (needs a live rank field) |
| D4 | Shot frame | **The extractor converts each league's ESPN coordinates to the wire's physical frame** (feet from the backboard plane, NBA.md §4); college y + 1 (ESPN's college rim at y ≈ 0.25 vs the NBA's 1.0, a 0.75 ft translation, scale 1.0 — §2). The renderer's `ESPN_ORIGIN` becomes the physical origin and stops knowing ESPN (owner, 2026-11-09) | A per-league origin on the device; rescaling college shots "to fill the range" (breaks marks agreeing with lines) |
| D5 | The court, both leagues | **The angled "arena" court** (owner, 2026-11-09, panel review): mild perspective, true per-league lines, pixel-art hoops (board, rim, net, stanchion) at 1.5× width, the centre circle back as an oval, each key's free-throw circle, plain wood, shapes drawn as regions — §3.11 is normative. Supersedes NBA.md D1's top-down court and D5's "no centre circle" | P1 oblique (the hoop floats); P3 2× hoop; plank texture and far-side darkening (invisible on the panel); NBA lines on the college court |
| D6 | Periods | Halves: chip **1ST / 2ND**, then **OT, 2OT …** counted from regulation + 1; halftime = the period that ended + **HALF** ("1ST" over "HALF"); end of regulation = **2ND + END**; final **FINAL / F/OT / F/{n}OT** | "H1/H2"; Q-labels for halves |
| D7 | Clock warning | Final regulation period or overtime, **under a minute in either clock shape** (college "0:39", NBA "39.0"), and never an all-zero clock ("0:00", "0.0") | NBA's colonless-only rule (never fires in college) |
| D8 | College flash | **Team timeouts (578, 579), technical fouls (521), challenge outcomes (215, 216)**; not TV timeouts (580: scheduled, team-less, the most common last play); placeholders (91) never. The NBA's table is unchanged — its feed has no TV-timeout type (owner, A3) | TV timeouts too |
| D9 | Pregame | **Venue, then ESPN's note** ("LAKE TAHOE CLASSIC"); **no league name on any sport's pregame** (owner, A2 — C1 below); per-team line by football's rule (a ranked side's "#10 HOUSTON", the unranked side named — 915f594) | League name first (football/soccer's convention until now) |
| D10 | Slate | **ESPN's default scoreboard** (games with a ranked team), as college football | All of Division I (150+ on winter Saturdays) |
| D11 | Config | **`sports.basketball.leagues`**, no migration (the stored `sports.nba` is dropped; owner's no-legacy rule). **Defaults: every league on except soccer** — MLB, NBA, NCAA BASKETBALL, NHL, NFL, NCAA FOOTBALL (owner, A4) | A one-shot `nba` → `basketball` migration; NBA-only default |
| D12 | Menu | Label **"NCAA BASKETBALL"** (15 glyphs in a 14-glyph window: clipped until highlighted, then it scrolls, as long labels already do). Registry and rotation order: MLB, NBA, NCAA BASKETBALL, NHL, NFL, NCAA FOOTBALL, MLS, PREMIER LEAGUE, LIGA MX, WORLD CUP (owner, A6) | "NCAA HOOPS" |
| D13 | Team colours | **Optional in the extractor**: a team without `color` or `alternateColor` keeps its game (absent = black → the model's dark-gray fallback). The NBA extractor's required-colour rule would silently drop 19 of 74 ranked-slate games (§2) | Dropping the event (today's NBA and football rule) |
| D14 | Release | Backend first, then **straight to stable**, as the NBA (owner, A11) | A dev-channel bench night |
| E1 | Court raster | **Proposed:** the court's static pixels (lines, keys, oval, hoops, knockout, per-league) are computed from the §3.11 code by a host test and committed as tables, regenerated with `SCOREBOARD_BLESS_COURT=1` and failing when stale — `art_compile.rs`'s pattern. Per frame the device fills the floor spans, plots the tables in the frame's team colours and projects the marks. Constant data, not memoisation; one source of truth stays the code | Region tests per frame on core 1 (~4,000 inside tests); a hand-drawn sprite |

## 2. Data facts (evidence-backed)

Sources (2026-11-08/09): the home collector's `ncaab` scoreboards (ESPN's
default slate, polled since 2026-07-20; this season from 2026-11-02; the
7-day window below is 15,900 distinct bodies), live ESPN scoreboards and
summaries fetched by hand (5 live games, 4 finals including 2 in overtime),
the core API's `situation`, and the collector's first college summaries
(from 2026-11-10 00:04 UTC). Raw captures: `data/ncaab-design/`.

### Scoreboard (`/apis/site/v2/sports/basketball/mens-college-basketball/scoreboard`)

- **Default slate = games with a ranked team**: 2–26 events a game day so far
  (7 days); all of Division I (`groups=50`) was 36 (Sunday) and 61 (Saturday).
- `status.type.name`: the NBA's five — `STATUS_SCHEDULED` (1),
  `STATUS_IN_PROGRESS` (2), **`STATUS_HALFTIME` (23) at period 1**,
  **`STATUS_END_PERIOD` (22) only at period 2** ("End of 2nd Half", before a
  final or overtime), `STATUS_FINAL` (3). Sequences (7 days, 73 games): 44
  SCHEDULED → IN(1) → HALFTIME(1) → IN(2) → END_PERIOD(2) → FINAL(2); 22 the
  same without END_PERIOD; 4 with an END_PERIOD(2) glitch before IN(2).
- **Two halves; overtime is period 3** (`detail` "Final/OT", `altDetail`
  "OT"; 7 OT finals in two days of Division I).
- `status.displayClock` is **always "M:SS"** (21,347 "d:dd" and 18,930
  "dd:dd" IN_PROGRESS samples; sub-minute "0:39", never "39.0"); breaks read
  "0:00" (HALFTIME 6,781, END_PERIOD 430). 380 IN_PROGRESS rows read "0:00"
  (end of a half before the status flips, or a frozen clock — not yet
  separated; D7 never paints it).
- `competitors[].curatedRank.current`: 1–25 ranked, **99 unranked** (98,278
  ranked vs 190,112 unranked competitor rows) — football's field and rule.
- `competitors[].team`: **10 % of competitor rows carry no `color` and no
  `alternateColor`, another 5 % no `alternateColor`** (288,390 rows): on the
  latest body per game day over 7 days, **19 of 74 games** have such a team
  (Quinnipiac, Monmouth, Bellarmine …). College football has the same gap
  (1 of 56 games, 2026-11-07/08, Merrimack). Abbreviations are 2–4 characters.
  Logos `…/i/teamlogos/ncaa/500/<team id>.png`.
- `leaders[]`: post rows `points / rebounds / assists / rating` (102,994),
  live the same (87,562; partial sets in the first minutes), pregame
  `pointsPerGame / reboundsPerGame / assistsPerGame` or none.
- `records[]`: `total`, `home`, `road`, `vsconf` as "W-L". **No `series`.**
- `notes[0].headline`: event names ("Hall of Fame Series - Las Vegas",
  "Lake Tahoe Classic", "Field of 68 Opening Day Marathon"). Postseason
  (`season.type` 3, the NCAA tournament's round and region) is unobserved
  live; historical March 2026 captures cover it (B0).
- `situation.lastPlay` (live): type ids differ from the NBA's except 215/216
  and 91 — most common: **580 OfficialTVTimeOut (7,372)**, 584 Substitution,
  519 PersonalFoul, 587 Defensive Rebound, 558 JumpShot, 540 MadeFreeThrow,
  572 LayUpShot, 598 Lost Ball Turnover, **91 Shot (placeholder, 1,552)**,
  586, **579 ShortTimeOut (1,255)**, 607, 574 DunkShot, 449, **578
  RegularTimeOut (308)**, 437 TipShot, 615 Jumpball, 618 Block Shot, **521
  Technical Foul (63)**, 402, **215/216 Coach's Challenge (Overturned /
  Stands)**, 412.

### Summary (`…/summary?event=<id>`: 121–154 KB early, 237–329 KB mid-game, 438 KB final)

- **The ledger is all zeros.** `header.competitions[0].competitors[]` carry
  `fouls {teamFouls, teamFoulsCurrent, foulsToGive, bonusState}` and
  `timeoutsRemaining/Used`, but every live game read 0 / "NONE" — including
  PSU–PITT with 11 personal fouls a side in the first half — and the core
  API's `situation` reads the same zeros. So did the collector's first four
  summaries. On finals the keys are absent, as in the NBA.
- `possession` false everywhere: no possession source (as the NBA).
- `plays[]`: the NBA's fields (`awayScore/homeScore` running, `scoringPlay`,
  `scoreValue`, `pointsAttempted`, `coordinate`, `team.id`, `participants`,
  `sequenceNumber`, `period.displayValue` "1st Half" / "2nd Half" / "OT").
  Team fouls (519, 521) and team timeouts (578, 579) are team-attributed;
  TV timeouts (580) are team-less.
- **Coordinates on every field goal** (35/35, 14/14, 95/95, 79/79, 128/128 in
  five summaries, small schools included); free throws carry the sentinel.
  Same half-court frame in feet (x 0–50, y toward half court), but a fit of
  417 "N-foot" shot texts puts the rim at **(25, 0.25)** against the NBA
  corpus's **(25, 1.0)** by the same method (700 texts), and a free scale
  fits **1.0** in both: a 0.75 ft translation, nothing else. Re-check on the
  B0 corpus before pinning.
- `boxscore.players[].statistics[0]`: 13 labels (no "+/-"; PTS still found by
  name); **14–18 athletes a team, one non-Division-I side listed 37** — the
  extractor's `ATHLETES_PER_TEAM = 18` overflows.
- `leaders[]` per team (points / assists / rebounds), `header.gameNote`
  (the event name).

## 3. Semantics (normative)

Everything NBA.md §3 says holds for both leagues unless this section says
otherwise. "Rules" are §1 D2's properties, resolved from the league.

### 3.1 States and periods

- Dispatch and phases exactly as NBA.md §3.1. A college `STATUS_HALFTIME`
  comes at period 1, `STATUS_END_PERIOD` at period 2; both are already
  phases, not periods.
- Labels from `Periods`: a regulation period is named by the table (Q1–Q4,
  or 1ST/2ND); period `regulation + 1` is **OT**, `regulation + n` is
  **{n}OT**. Final banner: **FINAL** through regulation, **F/OT**, **F/{n}OT**
  (the shared `set_final_text`; football passes `QUARTERS`).
- Line-score headers: the period numbers through regulation, then **OT**,
  **2OT** … (closes BACKLOG 104's NBA half; football follows).
- The extractor's `altDetail` cross-check uses the league's regulation.

### 3.2 Clock

- Drawn verbatim (NBA.md §3.2). Breaks: HALF / END in the accent colour,
  the chip keeping the period that ended.
- **Warning** when in progress, period ≥ the league's regulation, the clock
  under a minute — minutes field 0 in "M:SS", or the colonless "S.t" — and
  not all zeros.

### 3.3 Ledger

- `rules.ledger == false` (college): the extractor skips the header ledger
  lane entirely (no quirk: zeros are the league's normal, not a glitch), the
  wire's ledger group is absent, and the band has no foul or timeout tracks.
- `rules.ledger == true` (NBA): NBA.md §3.3 unchanged, including the hold.

### 3.4 Makes — the frame

- NBA.md §3.4 with the league's frame shift applied in the extractor before
  the wire: **college y + 1** (rounded 0.75 ft). The wire's y is "feet from
  the backboard plane" for every league.

### 3.5 Runs, 3.6 newest score — unchanged

NBA.md §3.5–3.6. The scorer's PTS lookup holds every athlete the box score
lists (raise `ATHLETES_PER_TEAM`; the extractor runs only on the backend).

### 3.7 Flash

Per-league eligible tables in the extractor's properties (D8): college
{578, 579, 521, 215, 216}; NBA unchanged. Placeholders never flash.

### 3.8 Pregame

Info cycle: **venue, then the note** (`notes[0].headline` upper-cased; plus
the series in NBA playoffs) — for every sport, no league name (C1). Per-team
line: a ranked college side's "#N SHORT NAME", the unranked side named
(football's rule, promoted to `scoreboard-espn::common`); otherwise the
betting line.

### 3.9 Final

NBA.md §3.9 unchanged: leaders from the scoreboard, records "W-L", no
series for college; the line-score fallback with §3.1's headers.

### 3.10 Team colours

D13: optional; absent reads as black and the shared rule lifts or swaps it.
Navy primaries draw faint marks on the wood (C4).

### 3.11 The arena court (both leagues)

Geometry is the reviewed prototype, `data/ncaab-design/render-ncaa/src/arena.rs`
view 4 (the "chosen court"), whose numbers are normative:

- **Camera:** perspective, 200 ft back from centre court on the near side,
  39° above the floor, fitted to the B5 court rect (x 2, y 26, 124 × 37): near
  sideline 118 px, far 97; far half 16 rows, near half 20. `project(x, y, z)`
  (feet: along from the left baseline, across from the centre line toward the
  far side, up) and its inverse on the floor (`floor_x`, `floor_y`).
- **Lines per league** (`CourtLines`, feet): NBA arc 23.75, corner 22, lane
  16 wide; NCAA arc 22.146, corner 21.65, lane 12 wide. Shared: 94 × 50,
  rim 5.25, backboard 4, free-throw line 19 from the baseline, free-throw
  circle and centre circle radius 6.
- **Raster rules** (the owner's pixel-art rules, reviewed on the panel):
  - **Shapes are regions.** The keys (lane box ∪ the free-throw circle's
    outer half, one shape) and the centre oval are digitised by pixel centre
    (a pixel is in when its centre is) and drawn as the region's edge —
    inside pixels with an outside side-neighbour that is on the court. This
    puts flats at every extreme and meets tangent lines by construction.
    The free-throw line is the last pixel short of 19 ft on each lane row.
  - **Pixel-perfect:** an edge pixel that is the corner of an L (exactly two
    edge neighbours, one beside and one above or below) is dropped.
  - The three-point line: corner rows and the arc as row runs joining the
    previous row (today's staircase), whole through every row.
  - **The boundary draws last** — sidelines and baselines unbroken; lane
    colours end against them.
- **Colours:** floor `WOOD` plain (no planks, no fog); lines `LINES`; keys in
  the attacking team's colour at 2/3; marks as NBA.md D5.
- **Hoops:** stanchion (base 6 × 2.5 ft behind the baseline at half `PAD`,
  post, arm), board 6 ft × 3.5 ft at 4 ft in, rim 18 in at 10 ft, net —
  drawn at **1.5× width, 1.06× height**; then a **one-pixel knockout**: every
  `LINES` pixel beside a pixel the hoop drew returns to the floor (or black
  off the floor). Marks and the stanchion keep their pixels.
- **Draw order:** floor → keys, lines, oval, arcs → boundary → marks (the
  newest blinking) → caption floor patch (clipped **between the three-point
  arcs**; it still clears the half-court line under the text) and caption →
  hoops with their knockout. The flash replaces the whole court, hoops
  included.
- **Band:** B5's crests, period and clock unchanged; with `ledger: false` the
  scores sit 2 rows higher (centred on the crests) and no tracks are drawn.

## 4. Wire

No layout change and no extension level. `crates/scoreboard-wire/src/nba.rs`
becomes `basketball.rs` (B1) and its docs say what is per-league: `period`
counts the league's periods (NBA quarters, college halves; overtime past
regulation), the ledger group is absent for leagues without one, `y` is feet
from the backboard plane after the extractor's frame shift. The college
endpoint serves the same layout at the requested level (ext=2 on the Rust
fleet; nothing frozen — no deployed decoder has ever polled it). The NBA's
level-0 bytes and route stay frozen for the gift fleet.

## 5. Budgets

To measure in the phase that changes them; record in BUDGET.md.

- `LEAGUE_KEY` 32 → **40** ("basketball/mens-college-basketball" is 34 bytes;
  "basketball/womens-college-basketball" 36): the crest directory's 32 keys
  (+256 B), the slate's league descriptors, `LeagueSources`.
- `MAX_SOURCES` 9 → **10** (`every_registered_league_fits_the_slate`): the
  ninth cost 208 B of poller arena (BUDGET.md).
- Snapshot: the view gains the court lines and the ledger flag (≈ 16 B a slot).
- Flash: the court tables (E1; two courts, a few hundred pixels each), the
  camera and the hoop drawing; the region code stays on the host.
- Core 1: the court per frame is floor spans + table plots + mark
  projections; measure frame time against today's court (BUDGET.md row).
- Slate (160 games): a November Saturday ≈ CFB 70 + NCAAB ≤ 26 + NBA ≈ 10 +
  NHL ≈ 14 + soccer (off by default) — fits; March's tournament days fit.
- Backend: one summary per live college game a device is polling, 121–438 KB
  per 5 s cache window; `JSON_CACHE_CAPACITY` (48) holds; re-derive its note.

## 6. Phases (each ends green: `cargo test --workspace`, clippy, thumbv8m builds; commits by explicit pathspec)

### B0 Corpus
`backend/testdata/basketball/mens-college-basketball/` as pairs
(`<name>.json` + `<name>.summary.json`) from the collector's college
summaries (from 2026-11-09 21:13 UTC) and historical March 2026 captures:
`pregame_ranked`, `pregame_unranked_colourless`, `tip`, `in_progress`,
`run`, `halftime` (period 1), `end_of_regulation` (END_PERIOD 2),
`overtime` (period 3), `final`, `final_ot`, `final_tournament` (season 3),
`technical`, `team_timeout`, `tv_timeout`, `placeholder_play`. PROVENANCE.md
as the NBA's. Re-check §2's counts and the frame fit; revise §2/§3 where
the data disagrees.

### B1 Rename (byte-identical)
`Sport::Nba` → `Sport::Basketball`; `nba` modules → `basketball` in wire,
espn, backend, model, render; `Mode::NbaLive/NbaFinal`, the views,
`commit_nba_*`, `PregameInput::nba`, `NBA_LIVE/NBA_FINAL`, `NBA_NEWS_MS`,
probe mode names; fixtures → `backend/testdata/basketball/nba/` and
`backend/testdata/wire/basketball/nba/` (football's layout); the golden
manifest's rows. Unchanged: the `/basketball/nba/…` route, the v2 bytes,
config keys (B8). **Gate: every wire golden, golden frame and court bitmap
byte-identical.**

### B2 C1 — no league name on any pregame
`PregameInput::football/soccer` lose `league_name`; venue leads, the note
follows where a sport has one. Football and soccer pregame goldens
re-blessed (only those).

### B3 Rules seam
`BasketballRules` (D2) with NBA values everywhere; byte-identical. Then the
OT line-score headers (§3.1) in their own commit.

### B4 Extractor
The properties struct; the college lane per §3 (no ledger lane, frame shift,
flash table, regulation 2, rank line promoted to `common`, colours optional,
athlete cap); football's colour rule in its own commit (C3). Tests: every B0
pair, chunk-split invariance, one named test per §3 rule.

### B5 Backend
`BasketballLeague { Nba, MensCollegeBasketball }` through `AnyLeague` and the
generic router; the handler threads the league to both URLs and the
extractor; `wire_corpus.rs` arms for the college pairs; the `ncaab-demo`
mock slate; the JSON cache note.

### B6 Model
The registry row (D12), `LEAGUE_KEY`, `MAX_SOURCES`, `from_slug` appending
the basketball slug, rules by league, §3.1–3.2's labels and warning, the
pregame (§3.8), the view's court and ledger flag; bounds and budget pins.

### B7 Render — the arena court
§3.11 for both leagues: camera, E1's tables and their bless test, hoops and
knockout, the caption clip, the re-centred band; golden frames re-blessed
(every basketball live frame changes); the bench-panel check through the
diag build; frame time and flash measured.

### B8 Config, menu, frontend
`sports.basketball.leagues`; D11's defaults (football's default changes
too); menu tests for ten rows; SportsCard's Basketball group ("College
Basketball — NCAA men's, ESPN's Top 25 slate"); `index.html.gz` rebuilt.

### B9 Release and docs
Backend deploy, then `publish-fw --channel stable`; watch the first college
rotation in the Fly logs. SPEC, BUDGET, PARITY (Rust-only screens), INVENTORY,
BACKLOG; ARCHITECTURE.md outside the repo.

## 7. Cross-sport work (each its own change)

- **C1 The league name leaves every pregame** (owner, A2) — B2.
- **C2 Today's flat court lets lane colour overdraw the baselines** (two
  pixels an end) — superseded by D5's boundary-last rule.
- **C3 Football's extractor drops games with a colourless team** (1 of 56) —
  D13's rule, its own commit in B4.
- **C4 Navy primaries** (UVA, Cal) draw faint marks on the wood: the colour
  rule lifts only hue-less darks.
- **C5 The hockey rink should follow the arena camera** for cohesion (nets
  as pixel art), after this ships.
- **C6 The diag build's upload stall** took the bench off the network again
  (2026-11-09): core 0 idle in the executor's WFE, core 1 rendering — the
  radio path, not the CPU (the field "goes dark" signature).

## 8. Risks

- **Thin corpus:** college summaries start 2026-11-09; postseason, delays and
  neutral-site quirks are unobserved live (historical captures cover the
  postseason's shape only).
- **The frame shift** is fitted on five games; B0 re-fits it.
- **Region raster bias:** pixel-centre digitising sits up to half a pixel
  inside the true shape (the NCAA lane drew a row narrower each side);
  accepted on the panel.
- **Perspective compresses the far half:** far-sideline marks pack tighter
  and read slightly hotter.
- **LED bloom:** bright neighbours fuse; the knockout handles the hoops, and
  dimmer lines are the next lever if needed.
- **The 380 "0:00" in-progress rows** may hide a frozen-clock quirk; the
  warning never paints all zeros either way.

## 9. Ops

- **Collector:** `ncaab` follows summaries since 2026-11-09 21:13 UTC
  (session 21); `ncaaw` scoreboards since 21:34 (session 22).
- **Working set:** `data/ncaab-design/` (gitignored): captures, crests,
  `render-ncaa/` (the renderer copy with the arena prototype),
  `mockup-harness/`, scripts, README.
- **Panel review:** the diag build (`--features diag`, boot-integrated,
  flashed with `probe-rs run … --speed 1000`) and `tools/diag.py`-style
  pushes of the harness's raw frames; the published image restores from a
  verified backup of the active partition.
- **Delegation:** the orchestrator owns §3, reviews every diff, runs every
  gate and commits by pathspec. B1 is mechanical (one lane); B4 is the
  hardest (strongest model, line-by-line review); B7 is art and is checked on
  the panel before it is called done.

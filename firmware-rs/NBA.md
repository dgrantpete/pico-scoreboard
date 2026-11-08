# NBA — end-to-end spec

Status: **decided** (owner, 2026-11-08, on the mockup gallery "NBA Panel
Mockups" and its taller-court study) for everything visual; three rows of
§1 (D8 play flash, D10 pregame line, D12 default) and the engineering
choices (D9 wire, D13 clock rule) are **proposed here** and wait for the
owner's veto. **Not started**: phases 6.0–6.9 follow. Every mockup was
drawn by a scratch program linking the real `scoreboard-render` crate over
real ESPN plays (`data/nba-design/mockup-harness`, local), so the geometry
in §6.5 is pixel-proven, not sketched.

The NBA already ships (2026-07-12): the scoreboard-only data path, a
"quarter + clock ledger" live screen and the shared line-score final. This
spec replaces the two NBA screens and widens the data path; it adds no
league, so most of the hockey spec's plumbing (`LeagueId`, config, menu,
frontend, `MAX_SOURCES`) is untouched.

Read order for an implementer: §1 decisions → §2 data facts → §3 semantics
(normative) → the phase you own in §6. §7 lists cross-sport work that is
deliberately its own change.

---

## 0. Scope

Redesign the NBA (`basketball/nba` on ESPN) end to end on the shipping data
path: collector → corpus → `scoreboard-espn` extractor (scoreboard table
grown, a new summary table) → `scoreboard-wire` (an extension on the
existing endpoint) → backend routes (the summary fetched for live games) →
`scoreboard-model` → `scoreboard-render` (the court, the leaders final) →
mock/staging → docs. The Rust firmware is the only firmware that gets the
new screens (the MicroPython tree is frozen; it keeps decoding the frozen
NBA layout, D9), and the backend is the only data path (BACKLOG 100).

Out of scope: WNBA and college basketball (BACKLOG 51 keeps WNBA; the
league registry shape leaves room), the lead chart (layout C, not chosen),
a win-probability bar, and the injury line (D10).

## 1. Decisions

The owner chose B "Court" and no alternates on 2026-11-08 (the layout-variant
experiment ended the same day: defaults are final, no settings knob), then
B5 from the taller-court study ("small clock is totally acceptable, well
worth the price"), Game leaders as the final, the BONUS caption, and the
primitives rule. The last column keeps what was turned down.

| # | Decision | Decided | Not chosen |
|---|----------|---------|------------|
| D1 | Live layout | **B5 "Court"**: crests, scores and the period-over-clock pair in one 24-row band, each score column a team ledger (foul dots over the score over timeout blocks), and a 124 × 37 top-down court under it carrying every made field goal where it was made (owner, 2026-11-08) | A "Quarter ledger" (today's frame filled in); C "Game flow" (lead chart); B as first drawn (17-row court, 4.1× squashed); B2 (29-row court, always-on situation line); B3 (ledger painted on the floor: clutter); B4 (true-proportion 62 × 33 court, big clock, a quarter of the area) |
| D2 | Final screen | **"Game leaders"** (own screen, the three-stars silhouette): PTS / REB / AST leaders of the game in their team colours, both records under the rule, the series line in the playoffs (owner, 2026-11-08). The line-score rows stand in while ESPN has not posted leaders, as hockey's rows stand in for its stars | Shared line-score final (today's); both, switchable |
| D3 | Fetch the per-game summary | **Yes, for live games only**, on the backend like hockey's; the device still receives one payload. The summary carries the marks (shot coordinates), the ledger (team fouls, bonus, timeouts), the runs and the last scorer. Finals and pregames come from the scoreboard alone: leaders, records and the series are on it (§2) | The 615-byte core-API `situation` (a second fetch for a subset of the summary's facts: one source wins); the summary on finals (nothing on the final screen needs it) |
| D4 | Runs | **Shown from 6-0**, the caption "N-0 RUN" with an arrow toward the running side; **from 8-0 the running side's score breathes** white ↔ team colour (the goal lamp's mechanism aimed at momentum); cleared when the other side scores (owner, 2026-11-08) | Thresholds 8/10; no run display |
| D5 | Court marks | **2 × 2, translucent at 96/256** over what is lit (one make 37 % of the team colour, two stacked 61 %, five 90 %: hockey's D9 heat map at a lower alpha for forty makes a side), **makes only**, threes the same size as twos, lane lines and free-throw line in the attacking team's colour at 2/3, floor wood, **no centre circle** (it squashed to a dash at every ratio), **each team attacks the basket under its own crest**, the newest make blinks opaque white for the news window (§3.6) (owner, 2026-11-08) | Filled paint (marks lose contrast); 1 px marks at 141 (invisible on wood); misses at a third alpha (noise); threes larger; the rink's "defended end" convention (puts a team's makes under the other crest) |
| D6 | Fouls and timeouts | **Five count dots above the score** = team fouls this quarter, the row **red when the side has no fouls to give** (the opponent shoots on its next foul); **seven 2 × 2 blocks below the score** = timeouts remaining, burned ones dark; a **"BONUS" caption on the floor** with the arrow at the side that shoots, so the dots name themselves within a quarter (owner, 2026-11-08: B5 plus the caption) | Dots and blocks in a labelled "FOUL"/"TO" ledger between the scores (B); the ledger on the floor (B3); fouls-to-give pips |
| D7 | Floor caption | One 56 px slot painted on the wood at the bottom centre; **run > newest score > BONUS > nothing**: a run while it lasts, else "FAMILY-NAME pts" of the newest scorer for the news window, else BONUS while a side is in the bonus, else bare floor (B5's rule; the priority order is this spec's) | An always-on situation line (B2: eight court rows); the game's leading scorer in the slot |
| D8 | Play flash | **Proposed:** the flash is for **stoppages and news** — timeouts (with the calling team), coach's-challenge and review outcomes, flagrant and technical fouls, ejections — and **made baskets do not flash**: the court's blink and the caption already announce them, and a make every ~80 s of wall time against a 15–20 s flash window would hide the court a fifth of the time, more in hot stretches | Every `lastPlay` (today: rebounds, substitutions and all); every make with ESPN's full text; a short "JOHNSON 3PT" flash (duplicates the caption and hides the court exactly when the new mark blinks) |
| D9 | Wire | **Proposed:** the new fields ride a **second extension level, `ext=2`**, appended after the frozen v2 payload and after the `ext=1` block on the existing `/basketball/nba/games/{id}`; the gift fleet (no `ext`) and every Rust firmware before this change (`ext=1`) keep decoding what they decode today, and a backend that predates the level answers plain v2, which the new decoder accepts as "groups absent" | A breaking relayout of the frozen NBA payload (deployed decoders on both fleets); a new endpoint |
| D10 | Pregame | **Proposed:** the shared "Big time" screen as today (records, venue, the betting line) with the info cycle's second line filled from the scoreboard: `notes[0].headline` ("NBA CUP - GROUP PLAY", "NBA FINALS - GAME 4"), plus " - " and `series.summary` in the playoffs — hockey's shape, free | The injury report as the second line (mocked; needs the 122 KB pregame summary per game per cache window for a cosmetic line — a BACKLOG candidate) |
| D11 | Primitives | **Everything through the shared helpers**: `big_score`, `chip`, `font::draw` with the one `Scroll`, the bottom-strip flash mechanism, `count_dots`, `pulse`, the toast overlay, the scroll rails. What the mockups invented becomes a **shared widget the moment two sports want it**: football's three timeout bars generalise to N and both sports call them; the possession arrow leaves `football.rs` for `widgets`; hockey's `mix`/`scale` move to the crate root beside `rgb565` and `pulse` (colour arithmetic, not a drawn primitive) and the court composites with them. No private copies in `nba.rs` (owner, 2026-11-08; landed as 6.5a) | Per-sport copies |
| D12 | Default | **Open (owner):** `sports.nba` defaults **off** today. NHL D5's reasoning (friends' units should rotate the sport without opening the settings page) applies once these screens ship | — |
| D13 | Clock | **Small font** (`spleen_5x8`, 30 px for "11:34") under the period chip — the price of the 37-row court, accepted by the owner; drawn verbatim, never extrapolated; **proposed rule:** warning colour only in the 4th quarter and overtime, sub-minute, and never for "0.0" (the frozen-clock quirk, §2) | The big clock (B4's court) |
| D14 | Art | **Procedural Rust court**, numbers beside the projection math that reads them (the owner's one-source-of-truth direction; the first BACKLOG 116 step). No Aseprite file | An Aseprite court compiled to bytes, as the rink was |

## 2. Data facts (evidence-backed)

Design time (2026-11-07/08, `data/nba-design/ESPN-NBA-FINDINGS.md`, local):
the home-lab collector's scoreboard bodies 2026-10-20 → 11-08 (40,598
polls, 33,246 distinct bodies, 287,740 event rows, **139 games**, 132
finals, all regular season), a live evening polled at 20 s with summaries
at 45 s (5 live games, 175 live/post summaries, 97 core-API situations), 41
historical scoreboards (18 regular-season dates, 23 play-in / playoff /
Finals dates: 253 events) and 5 complete historical summaries (2,718 plays,
90 play types). Counts are from that set. The collector began keeping NBA
summaries on 2026-11-08 03:15 UTC (§9.1); phase 6.0 re-checks every claim
below against its first fortnight and builds the corpus from it.

### Scoreboard (`/apis/site/v2/sports/basketball/nba/scoreboard`, ~19 KB/game)

- `status.type.name`: exactly five occur — `STATUS_SCHEDULED` (id 1, pre),
  `STATUS_IN_PROGRESS` (2, in), **`STATUS_HALFTIME` (23, in: halftime has
  its own status**, always period 2), `STATUS_END_PERIOD` (22, in: periods
  1, 3, 4 and 5 — **never 2**), `STATUS_FINAL` (3, post). Not seen in 139 +
  253 games: DELAYED, POSTPONED, CANCELED, SUSPENDED (hockey saw DELAYED
  once in 99; assume it exists).
- Canonical sequence: SCHEDULED(0) → IN(1) → END_PERIOD(1) → IN(2) →
  HALFTIME(2) → IN(3) → END_PERIOD(3) → IN(4) → [END_PERIOD(4)] →
  FINAL(4). 106/132 games showed END_PERIOD at period 4 before FINAL
  (median one body, max 3.9 min); 13 went straight to FINAL. Overtime:
  END_PERIOD(4) → IN_PROGRESS period 5 ("5:00 - OT") → END_PERIOD(5) "End
  of OT" → FINAL period 5; 2OT is period 6. **No OT status of its own.**
  Breaks: Q1/Q3 median 2.4–2.5 min (max 5.1); halftime median 15.1 min
  (min 4.2, one 51-min outlier). Glitches: one HALFTIME before IN(2), one
  END_PERIOD(4) between END_PERIOD(3) and IN(4).
- **Finals never flap** (0/132) and no final had a tied score. `period`
  alone gives periods played (no shootout): `altDetail` absent ↔ 4, "OT"
  ↔ 5, "2OT" ↔ 6, with `detail` "Final" / "Final/OT" / "Final/2OT".
- `status.displayClock`: time **remaining**; "M:SS" at ≥ 60 s, **"S.t"
  with one decimal under a minute** ("53.0", "0.1"), "0.0" pregame and on
  most END_PERIOD and HALFTIME bodies — "0.1" on some (corpus,
  2026-11-08). Reads "12:00" ("5:00" in OT) while IN_PROGRESS
  between a break and the first play. **It moves between polls without a
  new play** (15,515 of 106,301 consecutive live pairs: a running clock
  sampled at poll time, unlike hockey's) but is exact only at poll time.
  **Frozen quirk:** 1,502 IN_PROGRESS rows read "0.0" while the game went
  on; 21/133 games had ≥ 3 score changes under a frozen "0.0" (one game
  ran a whole 4th quarter at "0.0"). **Post-final clock is garbage** ("0.0"
  or "12:00", 103/132 games showed both).
- `competitors[].score`: string. Went **down** between consecutive bodies
  106 times in 66/139 games; 13 restored on the next body (stale CDN
  copies), the rest stayed down (corrections). Hockey's flicker class.
- `competitors[].leaders[]`: four categories, one leader each
  (`leaders[0]`): `points` / `rebounds` / `assists` / `rating` —
  **live game totals** once the game is live (full set on 104,317 of
  106,434 live rows; partial or empty only in the first minutes) and final
  totals on **94,489 of 94,504 post rows**; season averages pregame. Each:
  `displayValue` ("30"), `value`, `athlete{id, shortName "L. Ball",
  jersey, position, team{id}}`. `rating.leaders[0]` is ESPN's player of the
  game (`displayValue` "28 PTS, 9 REB, 3 BLK"). `status.featuredAthletes`
  **never exists** (0 rows): no stars of the game.
- `competitors[].linescores[]`: live includes the current period (99.7 %);
  final 4 entries, 5 with OT, 6 with 2OT; absent pregame.
- `competitors[].statistics[]`: sixteen names (FG, 3P, FT, REB, AST, PTS
  and averages); `[]` on 9 % of pregame rows and the first 1–3 live
  minutes. Not here: turnovers, steals, blocks, fouls, timeouts.
- `competitors[].records[]`: three "W-L" entries (`total`/overall, `home`,
  `road`); the total's `abbreviation` reads "Game" on regular-season finals
  — accept any. Playoff events add a bare `competitors[].record` string.
- `competitors[].team`: `color` and **`alternateColor` on every team**;
  `logo` `…/nba/500/scoreboard/<abbr>.png` with the abbreviation
  lower-cased (`gs`, `ny`, `sa`, `no`, `utah`, `wsh`), `logoDark` under
  `500-dark` (26/30 byte-identical to the light file). Black primaries: BKN,
  SA (black alternates: CHI, HOU, MIA, POR, TOR) — the colour rule that
  landed 2026-11-08 (§3.10) covers them.
- `competitions[0].situation` (live only, 106,434 rows): **`lastPlay`
  only** — no possession, timeouts, fouls, bonus or shot clock, ever.
  `lastPlay`: `id`, `type{id,text}`, `text`, `scoreValue`, `team.id`
  (85.9 %), `athletesInvolved[]` (59 %), **`probability` (99.9 %: a live
  win probability)**; no clock, coordinate or period. **Placeholders:** ESPN
  posts a generic type first and re-types later ("No Shot (Default Shot)"
  type 91 on 4,878 rows, "No Turnover" 945, "No Violation" 211); the text
  is real immediately, the type id is not — except that a placeholder's
  text can lack the player's name ("makes 2-foot shot"). The scoreboard
  re-types a placeholder before the summary's `plays[]` does, and
  `plays[]` lags `lastPlay` by a play (corpus, 2026-11-08). A "Full Timeout" `lastPlay`
  (type 16, team-attributed) stays up 80–220 s (median 180 s); a free-throw
  `lastPlay` (ids 97–105) is visible for one or two 20 s polls.
- `notes[0].headline`: "NBA Cup - Group Play" (41,225 rows), "NBA Mexico
  City Game", "West Finals - Game 7", "NBA Finals - Game 4", "NBA Play-In -
  East - 8th Seed Game" — **the only game-number source**. `series`
  (playoffs only, 31/31 post-season events, 0 regular-season rows):
  `{type: "playoff", summary: "CLE leads series 1-0", totalCompetitions 7}`.
  `season.type`: 2 regular (preseason and the NBA Cup included), 3
  post-season, **5 play-in**.
- `odds[]` on pregame events (the betting line the pregame already shows).

### Summary (`…/summary?event=<id>`: 110–114 KB at tip, ~160 KB after Q1, 273 KB at halftime, 338–395 KB in the 4th, 380–513 KB final)

- `header.competitions[0].competitors[]` (live only; the keys **vanish on
  finals** and are absent pregame): `fouls{teamFouls, teamFoulsCurrent,
  foulsToGive, bonusState}`, `timeoutsRemaining`, `timeoutsUsed`,
  `possession`, `linescores[]`, `homeAway`, `team.id`. Semantics from 318
  header sides and 182 core-API sides:
  - **Timeouts:** `timeoutsRemaining` is the game count, **7 at tip-off**,
    minus one per "Full Timeout" play, updated within a poll of the play;
    `timeoutsUsed` is this quarter's and resets each quarter. One late +1
    (a challenge-retained timeout) seen.
  - **Fouls:** `teamFoulsCurrent` is this quarter's team fouls (offensive
    included), `teamFouls` the game's; both reset at the quarter change.
    `foulsToGive` is the penalty counter: 4 at each quarter start, counts
    down on defensive fouls, **0 = the opponent shoots on the next foul**.
    `bonusState` is "NONE" or "DOUBLE", and **`X.bonusState == "DOUBLE"` ⇔
    the opponent's `foulsToGive == 0` ⇔ X is in the bonus** — 494/494
    sides, no exceptions, no "BONUS" value ever appeared.
  - **Glitch:** exactly one body in 318 read `timeoutsRemaining` 0/0,
    `timeoutsUsed` 0/0 and `foulsToGive` 0/0 with `bonusState` "DOUBLE" on
    both sides **while `teamFouls` (16/11) and `teamFoulsCurrent` (1/1)
    kept their values** — the reset zeroes the penalty fields and leaves
    the foul counts — gone one poll later. Zeros alone are not the
    signature: both sides out of timeouts and both in the penalty is a
    real end-of-game state (ORL@GS 2026-11-08, end of the 4th,
    `timeoutsUsed` 3/4). (Corpus, 2026-11-08.)
  - **Overtime:** at an overtime's start `timeoutsRemaining` resets to 2 a
    side and `teamFoulsCurrent` to 0 (`foulsToGive` 4); the END_PERIOD(4)
    body keeps the 4th quarter's ledger for ~40 s before the reset.
  - `possession` is `false` on all 318 sides, `possessionArrowAvailable`
    `false`: **there is no possession source.** The header's `status.clock`
    is a stale sample (matched the last play's clock on 48/153 live
    summaries) and froze with the scoreboard's.
- `plays[]` (~545 B each, 56–66 % of a final body): `id`,
  `sequenceNumber`, `type{id,text}` (90 types; 52 shot types ids 92–282),
  `text` ("Giannis Antetokounmpo makes 3-foot driving dunk (Davion Mitchell
  assists)"), `period{number, displayValue}` ("4th Quarter", "OT"; 2OT is
  period 6), `clock.displayValue` (remaining, "M:SS" or "S.t"),
  `awayScore`/`homeScore` (**running score after the play, on every
  play**), `scoringPlay`, `scoreValue` 0/1/2/3, `shootingPlay`,
  `pointsAttempted` 0/2/3 (free throws 0), `coordinate{x,y}` (sentinel
  `x: -214748340, y: -214748365` = none), `team{id}` (98.4 %; missing on
  End Period / End Game / ref-initiated reviews), `participants[]`
  (`[{athlete{id}}]` only — **no role, no name**; 1 = shooter / fouler /
  rebounder, 2 = shooter + assister or turnover + stealer).
- **Coordinates — the shot chart.** One half court in feet, **shared by
  both teams and both halves** (no flip): `x` 0–50 sideline to sideline,
  `y` grows toward half court. A fit against 707 "N-foot" play texts gives
  **the basket at (25, 1)** (0.39 ft error); the NBA rim is 5.25 ft from
  the baseline and 1.25 ft in front of the backboard, so **y = 0 is the
  backboard plane**, 4.25 ft from the baseline; corner threes read y −2..3,
  above-break threes median y 24, half court y ≈ 43, heaves to 70. Values
  are integers. **Coverage is complete on field goals** (953/953 shooting
  plays in 5 finals: made threes 138/138, made twos 280/280, misses
  535/535); **free throws carry the sentinel** (94/94), as do rebounds,
  substitutions and timeouts.
- Non-shot types that matter: **16 Full Timeout** (13–18 a game,
  `team{id}`, text "Spurs Full timeout"; no other timeout type observed),
  584 Substitution, 412/402 End Period/End Game (team-less), 44/45/43/42
  shooting / personal / loose-ball / offensive foul, technicals 29
  (defensive three seconds), 30 (double), 35, 48, 32 flagrant type 1 (no
  type 2 seen), 97–105 and 157/165/166 free throws, 62–90 turnovers,
  155/156 rebounds. **Challenges post first as 213 "Challenge" or 277
  "Coach's Challenge (replaycenter)"** (team-less, "(07:38) [Magic] COACH'S
  CHALLENGE (CALL REPLAYCENTER)") **and are re-typed in place** to 214 /
  215 / 216 (coach's challenge supported / overturned / stands) or 278 /
  279 / 280 (ref-initiated review, likewise). No ejection type or text
  exists in the corpus. The pinned table is in
  `backend/testdata/nba/PROVENANCE.md`. **No `run`, `streak` or
  `timesTied` key exists anywhere** in a summary; runs are derived from the
  running scores.
- `boxscore.players[]`: one entry per team, `statistics[0].athletes[]`
  (14–15 each) with `athlete{id, displayName, shortName, jersey}`,
  `active` (the five on the floor), `stats[14]` (MIN, **PTS**, FG, 3PT, FT,
  REB, AST, TO, STL, BLK, OREB, DREB, PF, +/-) as strings. Absent pregame.
  `boxscore.teams[].statistics[]` (25 names incl. `largestLead`,
  `leadChanges`): no bonus or team-fouls-this-quarter.
- `leaders[]` (per team: points / assists / rebounds with `shortName` and
  "12/25 FG, 9/11 FT"), `winprobability[]` (one entry per play),
  `injuries[]` (per team, present pregame/live/final), `gameInfo`,
  `header.gameNote` ("NBA Finals - Game 4"). **Caution:** the header's
  `series[]` / `seasonseries[]` entry of type `season` is the
  regular-season head-to-head even when its description says "NBA
  Finals"; the playoff series is the **scoreboard's** `series.summary` or
  a header entry of type `playoff`.
- **Not anywhere:** possession, a shot clock, a run field, stars of the
  game, team fouls on the scoreboard.

### Core API (`sports.core.api.espn.com/v2/…/competitions/<id>/situation`, 615 B, `max-age=3`)

Carries the same timeouts and fouls block as the summary header
(`homeTimeouts.timeoutsRemainingCurrent`, `homeFouls.bonusState`, …) and
nothing the summary lacks. Noted and **not used** (D3): one source.

## 3. Semantics (normative)

Each rule that handles a quirk names it. "Side" is away or home; "the
attacking end" of a side is the basket under its own crest (D5).

### 3.1 States and periods

- Dispatch on `status.type.state`: pre → Pregame; post → Final; in → Live
  with `LivePhase::InProgress`, `Halftime` for `STATUS_HALFTIME`, or
  `EndOfPeriod` for `STATUS_END_PERIOD`. A live body at period 0 (a delayed
  tip; unobserved) → Pregame, hockey's rule. An unknown status name is a
  quirk and follows its `state` as `InProgress`.
- Playoffs ⇔ `season.type` 3; play-in (5) is not the playoffs (no
  `series`), but its `notes` headline shows like any other.
- Period label: 1–4 → `Q1`..`Q4`; 5 → `OT`; n ≥ 6 → `{n-4}OT`. Final
  label: `FINAL`, `FINAL/OT`, `F/{n}OT` (n ≥ 2), from `status.period`;
  `altDetail` is the cross-check (a disagreement is a quirk).
- ESPN's own glitch orders (§2) are served as ESPN states them.

### 3.2 Clock

- Drawn verbatim from `displayClock`, never extrapolated: it is a running
  clock sampled at poll time and the device cannot know when the next
  whistle came.
- `Halftime` → the clock slot reads `HALF`, `EndOfPeriod` → `END`, both in
  the accent colour; the period chip keeps the period that ended (hockey's
  rule; today's NBA screen blanks the chip at halftime and reads "HT" —
  the word changes because the slot has room and every mockup the owner
  saw read HALF).
- Warning colour when `InProgress`, period ≥ 4, and the clock is sub-minute
  (the colonless "S.t" shape) **and not "0.0"**: a real 0.0 lasts a poll
  before END_PERIOD, while the frozen quirk (§2) would otherwise paint a
  red 0.0 for ten minutes. The frozen clock itself is shown as ESPN states
  it; accepted.
- Play clocks on screen (none today) would be **remaining** time, the
  sport's convention.

### 3.3 The ledger: team fouls, bonus, timeouts (summary header)

- Source: `header.competitions[0].competitors[]` matched to sides by
  `homeAway`. Per side: `fouls = teamFoulsCurrent` (saturating u8),
  `in_bonus = (fouls.bonusState == "DOUBLE")` — equivalently the opponent
  has no fouls to give — and `timeouts = timeoutsRemaining` (saturating u8).
- **The group is absent** when any field is missing on either side (finals,
  pregame, a summary the backend could not fetch), **and on a glitch
  body**: one whose penalty state is impossible — `foulsToGive` 0 with
  `teamFoulsCurrent` under 2 on either side (the penalty takes four team
  fouls, or two inside a period's last two minutes), which is the shape
  §2's reset has: zeroed timeouts and fouls-to-give under intact foul
  counts. Zeros alone are not the signature; two teams out of timeouts and
  both in the penalty is a real end-of-game state, and a detector keyed on
  it would freeze the dots for a quarter. An absent group hides nothing abruptly: the
  model **keeps the previous commit's ledger when that commit was this
  game, live** — the glitch lasts one poll and the hold outlives it — and
  shows no ledger at a first look (a game rotating in without its summary).
- Display: the dots count `min(fouls, 5)`; the row is the warning red when
  the opponent is in the bonus (this side has none to give), else the team
  colour. The mockups lit red at five dots; the real signal is ESPN's
  state, which also covers the last-two-minutes and overtime penalty rules
  without the device knowing them. The blocks count `min(timeouts, 7)`.
- Quarter changes reset the dots because ESPN resets the field; during a
  break whatever ESPN says is drawn.

### 3.4 Makes — the court (summary plays)

- A **make** is a play with `scoringPlay` and `pointsAttempted ≥ 2`
  (`shootingPlay` is also true), a team, and a coordinate that is not the
  sentinel. Per make: side (the play's team), `three = scoreValue == 3`,
  `x = coordinate.x − 25` (feet from the centre line) and `y =
  coordinate.y` (feet from the backboard plane toward half court), in play
  order. A scoring field goal without a team or with the sentinel (none in
  953 observed) is a quirk and gets no mark; it still counts in the score.
- Normalisation: **home's `x` is negated** — a 180° rotation about the
  court's centre, hockey's §3.5 rule, which keeps handedness when the two
  ends are drawn facing each other. (ESPN's half-court frame is already
  folded for both teams, so no viewer can tell; the rule exists so the two
  sports project the same way.) `y` is clamped to −8..=47: a made heave
  from beyond half court sits on the half-court line.
- Keep the **newest 128** makes (`MAX_MAKES`): the longest games run ~110
  made field goals; past the cap the heat map loses its oldest marks and
  the newest is always kept. The set is rebuilt from the plays on every
  poll, so a play re-typed or corrected in place (§2's placeholders) needs
  no reconciliation.
- Plotting (§6.5 has the numbers): along the court from the attacked
  baseline = `y + 4.25` ft (the rim is 5.25 ft in and ESPN's y = 1 is the
  rim), across = `x`; each mark a 2 × 2 composited at 96/256 over what is
  lit, in play order, clamped inside the court's lines.

### 3.5 Runs (summary plays)

- Walk the plays in order with the previous running score. A play whose
  `(awayScore, homeScore)` differs from the previous one is a **scoring
  step** for the side whose score rose, worth the difference. The current
  run is the side and the sum of the trailing scoring steps by the same
  side; a step by the other side starts a new run with that step's points.
  A step on which **both** scores rose (a correction) ends any run and
  starts none. A run **survives period breaks** (unanswered points are
  unanswered through a timeout or a quarter change).
- `run = Some(side, points)` when `points ≥ 6` (`RUN_SHOW`), else `None`.
  The renderer breathes the running side's score from `points ≥ 8`
  (`RUN_GLOW`): `mix(white, team, pulse(now − animation_start_ms, 1200) ×
  200/256)` on the wall rail. A run is **state, not news**: a first look
  shows it, and it is shown during the flash too (the band stays).
- Points are capped at 255; the caption reads "N-0 RUN".

### 3.6 The newest score and the caption

- The **newest scoring play** (any `scoringPlay` with a team, free throws
  included) gives `last_score: side`, the scorer and their points:
  - **Scorer:** the play text up to " makes " is the player's name
    ("Kelly Oubre Jr. makes …"); drop a trailing generational suffix (Jr.,
    Sr., II, III, IV) and take the last token, upper-cased: `OUBRE`,
    `TOWNS`, `GILGEOUS-ALEXANDER`. A scoring play whose text has no
    " makes " (a goaltending award), or nothing before it (a placeholder's
    "makes 2-foot shot"), has no scorer: the caption is skipped, the score
    and any mark still land. The summary's newest scoring play can trail
    the scoreboard's `lastPlay` by a play, so the caption and blink can be
    a poll behind the score; accepted.
  - **Points:** the player's PTS from `boxscore.players[].statistics[0]
    .athletes[]` whose `athlete.id` is the play's first participant;
    absent when the boxscore has not listed him yet (the first minutes),
    and then the caption shows the name alone. The model composes the
    caption once (`caption: Text<PLAYER>`, "BRUNSON 36" or "BRUNSON"); the
    renderer draws that string and the prepared view measures it, so the two
    cannot disagree about its width.
  - `newest_marked`: the newest scoring play is also the newest make (a
    field goal with a mark), so the blink has a mark to blink.
- **News window.** The store stamps `score_ms = now` on a commit whose
  newest scoring play's `sequenceNumber` (`score_seq` on the wire) differs
  from the previous commit's for the same game (first look: no stamp — a
  basket already on the board when the game rotates in is old news; rotating
  away and back is a first look again).
  The caption shows the scorer and the newest mark blinks opaque white
  (400 ms on, 400 ms off) while `now − score_ms < window`, where `window =
  max(NEWS_MS, one scroll cycle of the caption text)` — `NEWS_MS` 8 s,
  computed in the prepared view exactly as the flash window is, so a long
  name is read once in full. Wall rail; no core-1 state.
- **Caption priority** (D7): the run (while `run` is `Some`) → the newest
  score (while its window is open) → `BONUS` with the arrow at the side
  that shoots, both arrows when both sides are in the bonus → nothing. Text
  is in the side's team colour; the arrow is football's 3 × 5 triangle at
  the slot's edge nearest that side's crest. The slot is painted wood first
  (it erases the half-court line and any mark under it), and nothing is
  painted when there is nothing to say.

### 3.7 The play flash (D8)

Source: the scoreboard's `situation.lastPlay` when it is itself eligible
(it is the newest play there is), else the summary's newest eligible play
(older, but the latest that qualifies), else none; the two name the same
play by the same id, so the change detection below cannot double-flash.
Eligible: **Full Timeout**
(type 16; text "Spurs Full timeout" as ESPN writes it), the **challenge
and review outcomes** (214 / 215 / 216 coach's challenge supported /
overturned / stands, 278 / 279 / 280 ref-initiated review likewise:
"COACH'S CHALLENGE (CALL OVERTURNED) …"), **technical fouls** (29, 30, 35,
48) and **flagrant fouls** (32). The table is pinned in
`backend/testdata/nba/PROVENANCE.md` (phase 6.0) and lives in one table in
the extractor; no ejection type exists to add. Not eligible: made baskets
and free throws (the court and the caption announce them, §3.6), rebounds,
substitutions, ordinary fouls, turnovers, period boundaries, the
**transient first posting of a challenge** (213 "Challenge", 277 "Coach's
Challenge (replaycenter)" — re-typed in place to its outcome, which then
flashes once under the same id: hockey's placeholder-penalty rule), and
the placeholder types (91 "No Shot", 61 "No Turnover" — the re-typed play
keeps its id and flashes then if eligible). The id is the play id; the shared `flash_play`
change-detection rule applies unchanged. While the flash is up the court
hides entirely and the line scrolls across the court's rows with its ink
centred (the hockey rule, §6.5); the band and its ledger stay.

The same filtered play fills the frozen v2 `last_play` field that older
decoders flash, so the gift fleet's NBA flash becomes timeouts-and-news too
— a behaviour change on frozen firmware, not a break.

### 3.8 Pregame (D10)

The shared "Big time" screen: W-L records (two rows), the start time, the
info cycle, the betting line per team (the `ext=1` lines, unchanged).
Info cycle: primary = venue; secondary = `notes[0].headline` upper-cased
when present, plus " - " and `series.summary` in the playoffs ("NBA FINALS
- GAME 4 - NY LEADS SERIES 2-1"), else empty (the shared cycle skips empty
lines). Team colours follow the shared rule (§3.10).

### 3.9 Final (D2)

Game-leaders screen: crests + scores (winner colour / loser gray), the
result label in the accent colour, three rows `PTS` / `REB` / `AST` each
with the game's leader in that category — `leaders[name=points|rebounds|
assists].leaders[0]` from **both** competitors, the higher `value` wins and
a tie keeps the away leader — as `shortName` ("J. Brunson") and
`displayValue`, in the player's team colour, scrolled with the stars'
`Scroll`; a rule; then the series line (`series.summary`, playoffs only)
and both records (`records[type=total].summary`, "W-L") in team colours.
**Leaders present means all three**, else none; absent, the shared
line-score rows take the three rows' place (the data is committed to the
shared `FinalView` regardless), so the screen is complete either way. The
leaders arrive on the final body itself (94,489 of 94,504 post rows), so
the fallback is for the handful of bodies without them.

### 3.10 Team colours

The shared rule, nothing NBA-specific: `Rgb888::from(TeamColors)` lifts
every colour to the luminance floor and lets a hue-less dark primary (SA,
BKN) yield to the alternate (landed 2026-11-08, ecd6b2e, from this design's
"black is the loser's colour" finding). Lane lines use the team colour at
2/3; marks the full colour under alpha.

## 4. Wire format — `scoreboard-wire/src/nba.rs`

Normative once phase 6.1 lands; this section is the design. The frozen v2
NBA layouts (the crate docs) **do not change**: the gift fleet decodes
them, and every Rust firmware before this change decodes v2 plus the
`ext=1` pregame lines.

**Extension levels (D9).** The `Accept` parameter becomes a level:
`ext=1` is today's (the pregame lines), `ext=2` adds the blocks below after
the `ext=1` bytes. The backend serves the highest level it knows, capped by
the request (so a future `ext=3` device gets 2 from this backend); a decoder
accepts a payload that ends after any complete block, so an older backend's
plain v2 or `ext=1` answer decodes with the new groups absent. The Rust
firmware asks for `ext=2` (`STRUCT_CONTENT_TYPE_EXT`); the backend's
`wants_ext` becomes `ext_level`, and its test that rejects `ext=2` flips.
It lands in three commits, each leaving the workspace green and coherent:
6.1 adds `encode_at(game, level, out)` (0 = v2, 1 = + the pregame lines,
2 = + the blocks below) and the decoder, keeping `encode`/`encode_ext`
compiling for the backend; 6.3 switches the backend to the parsed level
and deletes `encode_ext`; 6.6 flips the device's `Accept` to `ext=2`.

**Live (state 1)** — after the v2 strings (`clock`, and the last play iff
v2 bit 0), the `ext=2` block:

| off | type | field |
|-----|------|-------|
| +0 | u8 | flags: b0 ledger, b1 run, b2 newest score, b3 newest score is the newest mark; b4–7 zero |
| +1 | u8 | away team fouls this period (b0) |
| +2 | u8 | home team fouls |
| +3 | u8 | bonus (b0): b0 away is in the bonus (shoots on home's next foul), b1 home is in the bonus; b2–7 zero |
| +4 | u8 | away timeouts remaining (b0) |
| +5 | u8 | home timeouts remaining |
| +6 | u8 | run side (b1): 1 away, 2 home; 0 when b1 is clear |
| +7 | u8 | run points, ≥ 6 when b1 is set, 0 otherwise |
| +8 | u8 | newest score side (b2): 1 away, 2 home; 0 when clear |
| +9 | u8 | the scorer's game points (b2); 0 = unknown, caption shows the name alone |
| +10 | u32 | the newest scoring play's `sequenceNumber` (b2): the news-window key, §3.6; 0 when clear |
| +14 | u8 | make count N, 0..=128 |

Then N make records, oldest first, 3 bytes each (`<Bbb`):

| off | type | field |
|-----|------|-------|
| +0 | u8 | b0 side (0 away, 1 home), b1 three-pointer; b2–7 zero |
| +1 | i8 | x, feet from the centre line, −25..=25, §3.4's rotation applied |
| +2 | i8 | y, feet from the backboard plane, −8..=47 |

Then, iff b2, the scorer's family name (§3.6). The play id and text for the
flash stay in the v2 field (§3.7).

**Pregame (state 0)** — after the `ext=1` lines block, the `ext=2` block:
one string, `info_secondary` (§3.8; empty when there is none).

**Final (state 2)** — after the shared v2 final (`common::write_final`),
the `ext=2` block:

| off | type | field |
|-----|------|-------|
| +0 | u8 | flags: b0 leaders, b1 series, b2 records; b3–7 zero |
| +1 | u8 | leader sides (b0): 2 bits per row, points lowest — 1 away, 2 home (0 is rejected: a leader always has a side) |
| +2 | u8×3 | leader values (b0): points, rebounds, assists, saturating |
| +5 | u16×4 | records (b2): away W, away L, home W, home L |

Then, iff b0, the three leader names (`shortName`), iff b1 the series
summary.

**Encode** clamps rather than fails: the newest 128 makes, coordinates
pulled back to the lines, counts saturated. **Decode rejects:** an undefined
flag bit, a side code outside 1–2 where a flag says it is present, a run
flagged with points < 6, a make record with b2–7 set, N > 128, a coordinate
outside the ranges, a zero leader side, truncation inside a block; a group
whose flag is unset is a placeholder, decoded as absent and never validated
(the football situation rule). A payload that ends before a block begins
decodes with that block absent (the extension rule above).

## 5. Budgets

- Snapshot today 3,208 B a slot (`the_snapshot_stays_inside_its_budget_line`,
  BUDGET.md 2026-11-08); four copies resident (channel 9,632 B + store).
  `NbaLiveView` grows from ≈ 34 B to ≈ 500 B: the 128-make ring (3 B a mark
  + length ≈ 385 B), the ledger (≈ 8 B), the run and newest-score groups,
  the caption `Text<PLAYER>` (42 B), the news key and anchor (12 B) and the
  two team colours (8 B). `NbaFinalView` ≈ 180 B (three `Text<PLAYER>`
  leader rows with sides and values, the series `Text<SERIES>`, two
  records). **≈ +650 B per copy, ≈ +2.6 KB total.** Update the pinned numbers in the
  same commit; record in BUDGET.md. This makes the NBA live view the
  largest and BACKLOG 103 (views as a sum) worth its measurement.
- Flash: no sprites (the court is procedural, D14; the leaders screen
  reuses the stars' silhouette); the court drawing, the two screens, the
  model and the wire decode ≈ 15–25 KB, hockey's non-extractor cost.
- Wire (§4): a typical 4th-quarter live game (80 makes, a 20-byte scorer,
  a 60-byte timeout text) ≈ 23 + 100 + 15 + 240 + 20 ≈ 400 B; the worst
  case (128 makes, a 255-byte play text) ≈ 750 B, inside the receive
  buffer's 2,048-B detail half (`poll::DETAIL_BYTES`). Pregame and final
  run 90–220 B.
- Backend: one summary per live NBA game per 5-s cache window
  (`JSON_CACHE_TTL`), 110–513 KB each; the 48-entry cache sized for hockey
  holds an NHL-plus-NBA night (16 + 15 summaries and the scoreboards);
  resident worst case ≈ 31 × 0.5 MB ≈ 16 MB on top of hockey's measured
  5–35 MB, against the Fly machine's 256 MB. Re-measure after 6.3.

## 6. Phases (each ends green: `cargo test --workspace`, clippy, thumbv8m builds)

### 6.0 Corpus and fixtures — done 2026-11-08
Landed: 21 events, 20 of them paired with a time-consistent summary (every
live and final one; pregame has none), from the design captures and the
collector's first night (ORL@GS's end of the 4th, overtime tip and OT
final), plus `bonus_both` (both sides in the penalty). Deferred to the
collector's fortnight: `double_overtime` (no live period-6 body exists
yet) and `final_no_leaders` (0 of 372 post rows lack the leaders). §2 and
§3 were revised against the corpus the same day (the glitch's shape, the
challenge types, break clocks of "0.1", the overtime ledger reset, the
nameless placeholder text).
- `backend/testdata/nba/` is **replaced**: today's seven April-2026 playoff
  events (BACKLOG 41: regenerate) give way to **pairs**
  `<name>.json` + `<name>.summary.json` (hockey's convention, PROVENANCE.md
  included; the harness skips `.summary` stems) from the collector's first
  fortnight of NBA summaries (from 2026-11-08) and the design captures:
  `pregame` (the betting line, a Cup or playoff note if one exists), `tip`
  (0–1 plays, `statistics []`, leaders empty), `in_progress`, `run` (a run
  ≥ 8), `bonus` (one side shooting; both if seen), `timeout` (lastPlay 16
  live), `end_of_period`, `halftime` (STATUS_HALFTIME), `end_of_fourth`,
  `overtime` ("5:00 - OT" at period 5), `double_overtime` if the fortnight
  has one (else the 2026-05-18 design capture), `frozen_clock` ("0.0" with
  a changing score), `ledger_glitch` (the 0/0 body, if caught again),
  `placeholder_play` (type 91), `final`, `final_ot`, `final_playoff` (a
  2026-06 design capture with `series`), `final_no_leaders` if one exists.
  Every live state exists in real data; nothing synthetic.
- The `.summary.json` skip leaves the hockey arm for the shared fixture
  enumerators (`wire_corpus.rs::fixture_names`, the espn parity gates): a
  summary is never an event fixture, for any sport. Tests pinned to the old
  fixtures' values move to the new ones; the NBA wire goldens and golden
  frames are re-blessed for today's screens (the renderer is unchanged
  until 6.5), so this phase is green on its own.
- Pin §3.7's eligible type-id table from every play type in the fortnight.
- Re-run the NBA schema and spec validation (`tools.espn validate --league
  nba`, BACKLOG 41's instruction) over the new fields; §2's counts are
  re-checked and §2/§3 revised where the fortnight disagrees, as NHL.md's
  were on 2026-10-14.

### 6.1 `scoreboard-wire`
`nba.rs` per §4: the level mechanism (`EXT_LEVEL`, `STRUCT_CONTENT_TYPE_EXT`
at `ext=2`, `encode_ext(level)`), the three blocks, round-trip + truncation
+ malformed-input tests, hand-checked hex goldens in `tests.rs`, the
"ends after any block" acceptance tests (v2 only, `ext=1` only, `ext=2`),
and the corpus goldens under `backend/testdata/wire/nba/` as `*.bin`,
`*.ext1.bin` and `*.ext2.bin` from 6.3's `wire_corpus.rs` arms.

### 6.2 `scoreboard-espn` (hardest lane — strongest model, line-by-line review)
`src/nba.rs`: the scoreboard table grows (leaders with `athlete.shortName`,
`displayValue`, `value` and `team.id` per category; `series.summary`;
`notes[0].headline`; `records` on finals; `lastPlay.type.id` for §3.7's
filter; `season.type`) and a **new summary table** (header competitors'
ledger by `homeAway`; plays: type id, `scoringPlay`, `scoreValue`,
`pointsAttempted`, `coordinate`, `team.id`, running scores, text,
`participants[0].athlete.id`, play id; `boxscore.players` athlete id → PTS)
with a `SummaryContext` from the scoreboard pass (state, period). All
sixteen DESIGN.md rulings apply; NBA has a serde predecessor only for the
scoreboard, so the summary lane writes its required-field rules down once.
Quirks (§3 says which stay quiet): the ledger glitch, a scoring field goal
without a team or coordinate, a scorer text without " makes ", an unknown
status name, `altDetail` disagreeing with the period. Tests: corpus
extraction over every pair, chunk-split invariance, field-order
independence, and one named test per §3 rule — the run walk (incl. the
both-rose correction and the break survival), the family-name rule (suffix,
hyphen, particle), the bonus equivalence, the glitch, the make filter
(free throws, sentinel, placeholder re-type), the 128 cap, the rotation,
the eligible-play filter, the leaders' both-teams pick and tie rule, the
secondary pregame line.

### 6.3 Backend
`src/nba/{handler,adapter,types,wire}.rs`: the detail handler fetches the
summary for **live** games only (`wants_summary` = live; hockey's
degrade-to-None on any failure), the DTO grows (ledger, run, newest score,
makes, leaders, series, records, secondary line — JSON clients see them
too), `game_response_ext` takes the level, `wire_corpus.rs` arms for the
three levels, OpenAPI schemas. The logo route needs nothing. Deploy with
6.8.

### 6.4 `scoreboard-model`
`Mode::NbaFinal`; `NbaLiveView` grows (ledger `Option`, run, `last_score`,
scorer text + points, `score_ms`, `newest_marked`, the make ring, team
colours; period/clock texts as today with the HALF word); `NbaFinalView`;
`commit_nba_live` with the first-look rule, the ledger hold (§3.3) and the
news stamp (§3.6) — the `LampWatch` shape, without the settle window (the
anchor is a play id, not a score); `commit_nba_final` committing both the
shared `FinalView` and the leaders; the news window computed beside the
flash window in the prepared view; bounds tests; the size pin and BUDGET.md
(§5).

### 6.5 `scoreboard-render`
`game/nba.rs` rewritten (live B5, final leaders), `geometry.rs` tables
`NBA_LIVE` (B5) and `NBA_FINAL`, `bottom_strip(Mode::NbaLive)` = the
court's flash slot. New shared widgets (D11), each landing in its own
commit with football's and hockey's goldens byte-identical (**landed as
6.5a, 2026-11-08**): `widgets::timeout_blocks(canvas, rect, total,
remaining, colour, dark)` replacing football's three-bar `timeout_bars` —
`total` equal blocks a pixel apart, as tall as the row and as wide as the
row leaves per block, so the geometry table decides the shape (football's
20 × 1 row gives three 6 × 1 bars, the NBA's 20 × 2 row seven 2 × 2
blocks), emptying left to right on both sides; `widgets::arrow`
(football's, moved); `scoreboard_render::{mix, scale}` (hockey's, moved to
the crate root); and **`court.rs`** (a crate module beside `qr.rs` and
`toast.rs`), the procedural court (D14): `Court {x, y, width, height}` with
`fx(feet)`, `dy(feet)`, `mid()`, `at(end, along)`, `draw(canvas, away,
home)`, `plot_over(canvas, end, along, across, colour, alpha)` and
`plot(canvas, end, along, across, colour)`, `End: From<Side>` (Away → Left,
Home → Right, so no screen invents the mapping), `MARK = 2` (no size
argument: one design), `ESPN_ORIGIN = RIM − 1 = 4.25` ft, `libm` for
`sqrtf`/`roundf` on the device — the mockup's `court.rs` is the reference
implementation, pinned byte for byte by `tests/court.rs`,
constants `LENGTH 94`, `WIDTH 50`, `RIM 5.25`, `ARC 23.75`, `CORNER 22`,
`PAINT_HALF 8`, `PAINT_DEPTH 19`, backboard at 4 ft (a 3-row vertical, the
rim taking the next column when the two round together), the three-point
line a staircase of row runs that never steps past its own row's x, wood
`rgb565(30, 20, 10)`, lines `rgb565(150, 150, 150)`, rim `rgb565(255, 110,
0)`, no centre circle, half-court line, lane lines + free-throw line at
2/3 of the attacking team's colour, away attacking the left basket.

**B5 geometry**, the 128 × 64 grid under the 1 px edge rule (rows 1–62,
columns 1–126; checked over every mockup frame):

| Element | Slice / rows | Note |
|---|---|---|
| Crests | away `rect(1, 1, 24, 24)`, home `rect(103, 1, 24, 24)` | |
| Foul dots | away `rect(26, 2, 24, 4)`, home `rect(78, 2, 24, 4)` | `count_dots`, five at a 5 px pitch, a pixel in from the crests |
| Scores | away `rect(25, 7, 24, 16)`, home `rect(79, 7, 24, 16)` | `big_score`; white, or the run glow (§3.5) |
| Timeout blocks | away `rect(27, 21, 20, 2)`, home `rect(81, 21, 20, 2)` | `timeout_blocks` with 7: 2 × 2 at a 3 px pitch, burned ones `rgb565(40, 40, 40)`, emptying left to right on both sides |
| Period | `rect(49, 5, 30, 8)` | `chip` (`unscii_8`), white |
| Clock | `rect(49, 14, 30, 8)` | `spleen_5x8` centred; normal / accent / warning (§3.2) |
| Court | `Court {x: 2, y: 26, w: 124, h: 37}` | rows 26–62; 1.31 px/ft along, 0.72 across (1.8× anisotropic) |
| Caption | `rect(36, 54, 56, 8)`, wood box `rect(36, 53, 56, 9)` | §3.6; small font; arrow 3 × 5 at the slot's edge |
| Flash | `rect(1, 38, 126, 16)` | replaces the court; ink centred in the court's rows; the band stays |

The band's arithmetic, for whoever touches it: 2 × 24 px of crest and 2 ×
24 px of three-digit score leave 30 px in the middle, and a five-character
clock is 30 px only in the small font (40 in the other two).

**Leaders final geometry** (the three-stars silhouette): crests `(1, 2)`
and `(1, 38)`; scores `rect(25, 6, 25, 16)` / `rect(25, 42, 25, 16)` in
`winner_colors`; rule at x 51 rows 1–62 in the secondary colour; label
`rect(53, 2, 74, 8)` (`chip`, accent); rows at y 14 / 24 / 34: the category
label at x 53 in gray (`spleen_5x8`), "NAME value" in `rect(73, y, 54, 8)`
scrolled with `HOCKEY_STARS_SCROLL` (renamed to the shared stars scroll) in
the player's team colour; rule y 43 from x 52; with a series line:
`rect(53, 46, 74, 8)` centred white, records at y 54; without: records at
y 51 — away at x 53, home right-aligned to x 126, each in its team colour.
The line-score fallback draws the shared three rows in the leaders' rows.
Tests: per-screen pixel tests in `tests/game.rs` (incl. the edge rule over
every frame of a blink and a flash), the court widget against the mockup
renders of the same moments, and the NBA wire fixtures in
`tests/golden_frames/manifest.txt` (today's `nba__*` rows re-blessed for
the new screens, the new pairs added). Mutation contract: the glow, the
blink and the caption window are pure functions of snapshot anchors and
the wall rail; no renderer state.

### 6.6 Config, input, app
Nothing structural: `sports.nba` exists, the league registry and
`MAX_SOURCES` are unchanged. `probe.rs` mode names gain `nba_final`; the
default flips only if D12 says so (defaults, patch tests, the README's
defaults table). The device's `Accept` becomes `ext=2` (`api_client.rs`).

### 6.7 Frontend
Nothing, unless D12 changes the NBA row's default copy. No rebuild of
`index.html.gz` otherwise.

### 6.8 Mock, staging, release
`tools/espn/mock.example.yml` + `infra/fly/mock.staging.yml` replace
`nba-in-july` with an `nba-demo` slate of 6.0's pairs (pregame → run →
bonus → halftime → overtime → final with leaders → playoff final); a local
backend over the mock serves every game, JSON and struct, each struct
payload decoding through `scoreboard_wire` at all three levels. Then the
bench unit on `publish-fw --channel dev` through a real NBA night (the
staging board is offline since 2026-10-12; releases go to stable from the
bench), then `stable`. **Deploy the backend before any firmware channel**: a device
asking `ext=2` of an older backend draws a bare court. Watch the friend's
unit's first NBA rotation in the Fly logs.

### 6.9 Docs
SPEC.md (§6's screens list, §4/§11's measured snapshot), BUDGET.md (§5's
numbers), `scoreboard-espn/DESIGN.md` (the NBA summary lane beside
hockey's), PARITY.md (the NBA live and final screens leave the parity set:
Rust-only designs, like hockey's), BACKLOG: close 41 (validated on live
games), note 104's NBA half (the line-score fallback still says "5"),
cross-reference 103, 115 and 116 (§7), add the injury line and any 6.0
finding. ARCHITECTURE.md is gitignored and updated outside the repo.

## 7. Cross-sport work found while designing (each its own change)

- **C1 Black primaries — landed 2026-11-08** (ecd6b2e): a hue-less dark
  primary yields to the alternate; found on the SA@OKC final, where the
  Spurs read as the loser's gray.
- **C2 Timeout blocks as a widget** — football's three bars and the NBA's
  seven are one `widgets::timeout_blocks`; lands inside 6.5 as its own
  commit with football's goldens unchanged (D11).
- **C3 The arrow moves to `widgets`, `mix`/`scale` to the crate root** —
  same change shape as C2; landed with it.
- **C4 The procedural court is BACKLOG 116's first step** — the `Court`
  projection type is the shape the rink's procedural rewrite takes next
  (`Rink {x, y, w, h}` with the same `at`/`plot_over`), after which
  `hockey_layout.aseprite` and its Lua go.
- **C5 Snapshot views as a sum — BACKLOG 103** — the NBA live view becomes
  the largest (§5); measure after 6.4.
- **C6 Line-score OT headers — BACKLOG 104** — the NBA's fallback rows
  still label overtime "5"; hockey's header function is the model.
- **C7 Play-flash eligibility is now a per-sport table in two sports**
  (hockey: goals and penalties; NBA: stoppages and news); a shared
  "eligible play" concept in `scoreboard-espn::common` would hold both
  tables. After 6.2.
- **C8 RGB888 — BACKLOG 115** — the run glow and the mark alpha quantise
  through 565 like hockey's power-play glow; the owner noticed the stair on
  the panel. The court composites in eight bits the day the canvas does.
- **C9 The extension level** (D9) is the first time a sport's existing
  payload grows after deployment; the `ext_level` mechanism is now the
  documented way to do it (crate docs, §4).

## 8. Risks

- **Summary cost on the backend** (D3): 110 → 513 KB per live game per
  5-s window, overlapping hockey's season from October to April. Bounded by
  the JSON cache and the device poll interval; re-measure resident memory
  after 6.3 (§5).
- **Unobserved states:** DELAYED / POSTPONED / SUSPENDED, a live playoff
  (the collector has only regular-season NBA so far: the series and
  game-number lines are designed from the 2026 playoff captures), the
  play-in's shape live. The extractor treats unknown status names as a
  quirk and degrades, never rejects a live game for them.
- **The frozen "0.0" clock** (21/133 games) is shown as ESPN states it, in
  the normal colour (§3.2); a running clock the device never extrapolates
  cannot be repaired on the device.
- **ESPN re-types plays in place** (placeholders, challenge outcomes) and
  corrects scores downward; the marks, runs and caption are rebuilt from
  the plays on every poll, so nothing accumulates stale state on the
  device; the flash's change detection sees the re-typed play as new once.
- **Type ids** are pinned from five games plus the first fortnight (6.0);
  a new timeout or review type would simply not flash until added.
- **Names**: the family-name rule is heuristic (suffixes, particles,
  hyphens); the caption scrolls anything wider than 56 px, so a wrong token
  reads oddly rather than breaking anything. A player's PTS can lag the
  play by a poll (the boxscore updates behind the plays); accepted.
- **The ledger hold** (§3.3) keeps a stale ledger through a summary outage
  for the same game; it is right for the one-poll glitch and shows minutes-
  old timeouts during a longer backend failure. Acceptable against a
  ledger that blinks out.
- **Both fleets' flash changes** (§3.7) without a firmware change: the
  gift fleet flashes less. Say so in the release note.

## 9. Ops

### 9.1 Collector (deployed 2026-11-08)
`infra/config/targets.yml`: the `nba` scoreboard target existed since
October 2026; `follow_summaries: true` added (commit cfd4e88) and deployed
2026-11-08 03:15 UTC (`ansible-playbook deploy.yml --tags targets` from
WSL; session #20 reloaded and the first NBA summary rows landed within the
minute). Consumer: §2's re-check and 6.0's corpus. Check coverage before
6.0 with `python -m tools.espn coverage --league nba` from a machine that
reaches the NUC's Postgres.

### 9.2 Delegation plan
Orchestrator owns §3 semantics, reviews every diff, runs every test,
commits with explicit pathspecs. Lanes: wire (6.1), espn extractor (6.2 —
strongest model), backend (6.3), model + render (6.4–6.5 together: the view
shape is their shared seam; the widgets commits first), config/app
(6.6). 6.1 → 6.2 → 6.3 is a chain; 6.4–6.5 can start against 6.1's types
with hand-built fixtures and the mockup harness's real moments
(`data/nba-design/mockup-harness/src/real.rs` is already the plays of five
games rebuilt into moments).

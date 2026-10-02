# NHL — end-to-end spec

Status: **draft for owner review** (2026-10-01). Nothing here is implemented
yet except the collector change in §9.1. Visual decisions reference the
mockup gallery (published Artifact "NHL Panel Mockups"); every mockup was
drawn by a scratch program linking the real `scoreboard-render` crate over
real ESPN captures, so the geometry below is pixel-proven, not sketched.

Read order for an implementer: §1 decisions → §2 data facts → §3 semantics
(normative) → the phase you own in §6. §7 lists cross-sport cleanups that are
deliberately **not** part of the NHL change.

---

## 0. Scope

Add the NHL (`hockey/nhl` on ESPN) as the fifth sport, end to end:
collector → corpus → `scoreboard-espn` extractor → `scoreboard-wire` →
backend routes → `scoreboard-model` → `scoreboard-render` → config / input /
app poller → frontend → mock/staging → docs. The Rust firmware is the only
firmware that gets hockey (the MicroPython tree is frozen), and the backend is
the only data path (the Phase S direct mode is legacy — BACKLOG 100).

Out of scope: college hockey, AHL/PWHL (the league registry shape in §6.4
leaves room), and the MicroPython gift fleet (§1 D7).

## 1. Decisions

Recommendations are marked; the owner's answers replace this table's last
column when given.

| # | Decision | Recommendation | Alternatives |
|---|----------|----------------|--------------|
| D1 | Live layout | **B "Rink" default**, **A "Period ledger" as the `hockey_live` variant** | C "Center ice" (dropped: small clock); A-only |
| D2 | Final screen | **"Three stars"** (own screen, soccer-final silhouette) | Shared line-score final with hockey columns; line score + cycling stars line |
| D3 | Fetch the per-game summary for live/final hockey | **Yes** — on the backend, like soccer's commentary; the device still receives one small wire payload | Scoreboard-only: no strength, no tags, no shootout tracker, no stars |
| D4 | Goal lamp (scoring side's number pulses red) | **Yes, 20 s** | No |
| D5 | Sport identity | **`Sport::Hockey`, league slug `nhl`, endpoint `hockey/nhl`, config `sports.nhl: {enabled}`** (single-league toggle, like NBA) | Multi-league `sports.hockey.leagues[]` — only if a second hockey league is wanted soon |
| D6 | Rotation / menu order | **MLB, NBA, NHL, football…, soccer…** | Append after soccer |
| D7 | MicroPython | **No hockey.** The SPA hides the NHL toggle unless `GET /api/config` returns a `sports.nhl` key (MicroPython's defaults never will) — capability by presence, zero legacy change | Ship hockey parsers to the fleet (not worth it) |
| D8 | Cross-sport fixes found here (§7 C1, C2) | **Separate reviewed changes**, not in the NHL diff | Fold in |

## 2. Data facts (evidence-backed)

Captured 2026-10-01 (four live preseason games, 13 scoreboard polls, 20 live
summaries) plus 66 historical dates (Nov 2025, the 2026 playoffs; 242 finals).
Raw bodies: session scratchpad `nhl-api/` → promote into
`backend/testdata/hockey/nhl/` in Phase 0.

### Scoreboard (`/apis/site/v2/sports/hockey/nhl/scoreboard`, ~22 KB/game)

- `status.type`: `STATUS_SCHEDULED` (pre), `STATUS_IN_PROGRESS` (in),
  `STATUS_END_PERIOD` id 22 (in — **the intermission**; no
  STATUS_INTERMISSION exists), `STATUS_FINAL` (post) with `altDetail`
  `"OT"` / `"SO"` / `"2OT"` (absent in regulation).
- `status.period`: **5 is ambiguous** — shootout in the regular season, 2OT in
  the playoffs. `events[].season.type` (2 regular — preseason is also
  labelled 2 — vs 3 postseason) resolves it live; `altDetail` confirms post.
- `status.displayClock`: time **remaining**, moves only when a play is
  logged (sat at 13:38 for three polls); reads "20:00 - 2nd" for minutes
  before a puck drop. Unreliable post-game ("0:31" on one OT final).
- `competitors[].score`: string; a shootout winner's score **includes the
  +1**. `team.alternateColor` is **never present** (0/484). UTA's color is
  `000000`. `team.logo` is always `…/nhl/500/scoreboard/<abbr>.png`
  (UTA's path is `utah`).
- `records[0].summary` is **W-L-OTL** ("53-22-7"); the first entry's `type`
  is `"ytd"` in Nov 2025 and `"total"` later — accept both. No points.
- `linescores[]`: per period including the current one; each OT period its
  own entry; the shootout is period 5 with 1/0 (winner/loser).
- `statistics[]`: `saves`, `savePct`, `goals`, `ytdGoals`, `assists`,
  `points` — game values live/post, season values pregame, `[]` at puck drop.
  `goals` excludes the shootout goal.
- **Shots on goal = opponent `saves` + own `goals`** — exact on 6/6 finals,
  within 0–2 live.
- `probables[]`: `probableStartingGoalie` with `athlete.shortName` and
  status Confirmed/Expected (no goalie stats), present 482/484.
- `situation` (live only): `lastPlay` only — **no strength, PP, EN, clock**.
- Playoffs: `series.summary` ("CAR leads series 3-2"), `notes[0].headline`
  ("Stanley Cup Final - Game 6" — the only game-number source).

### Summary (`…/summary?event=<id>`, 85 KB at puck drop → 300 KB mid-game → 400–630 KB final)

- `plays[]` (~850 B each, 55–70 % of the body): `type`, `text`,
  `period{number, displayValue}` ("1st", "OT", "2nd OT", "SO"),
  `clock.displayValue` **elapsed** in the period, `team.id`, `scoringPlay`,
  `participants[]` (`athlete.shortName`, role `scorer`/`assister`/…),
  `coordinate{x,y}` (ft from center), `strength` (701 Even, 702 Power Play,
  703 Shorthanded, 903 Empty Net — goals only), `shotInfo` (903 Empty Net,
  905 Shootout), `awayScore`/`homeScore` (**the shootout tally** during the
  shootout).
- `strength` is **relative to the play's team** and present on every play
  type, but a penalty play carries the strength from **before** it, and a
  pulled goalie (6v5) still reads "Even Strength".
- Penalties: Nov 2025 = type 509 "Penalty"; 2026 = the infraction is the
  type, with `type.penaltyMinutes` / `type.penaltyType` (Minor, Major, Bench,
  PS). Junk rows exist ("Penalty Shot Infraction … Minor" during a fight).
- `onIce[]`: entries **include the goalie**; counts give strength (6v6, 6v5
  on a PP, 4v4 in 3-on-3 OT); a pulled goalie = no goalie among the entries
  (goalie ids from `boxscore.players[].statistics[name=goalies]`). Glitched
  to 3 entries during a fight stoppage.
- Finals: `header.competitions[0].status.featuredAthletes` = winningGoalie,
  losingGoalie, firstStar, secondStar, thirdStar (6/6).
- **Not anywhere:** a power-play clock or penalty box, an intermission clock,
  points (scoreboard), reliable shootout box stats.

## 3. Semantics (normative)

### 3.1 States and periods

- pre → Pregame; post → Final; in → Live with `LivePhase::InProgress`, or
  `LivePhase::EndOfPeriod` for `STATUS_END_PERIOD`. (`Halftime` is unused.)
- Period label: 1–3 → `1ST`/`2ND`/`3RD`. Regular season: 4 → `OT`, 5 →
  `SO`. Playoffs: 4 → `OT`, n ≥ 5 → `{n-3}OT`.
- Final label: `FINAL`, `FINAL/OT`, `FINAL/SO`, `F/{n}OT` (n ≥ 2). Derive
  from period + season kind; `altDetail` is the cross-check (a disagreement
  is a quirk, not a rejection).

### 3.2 Clock

- Drawn verbatim from `displayClock` and never extrapolated (NBA rule:
  hockey's clock stops on whistles and the feed only moves on plays).
- `EndOfPeriod` → the clock slot reads `INT` in the accent color; the
  period chip keeps the period that just ended.
- Warning color when `InProgress`, period ≥ 3, and the clock is sub-minute
  (`0:` prefix, or the colonless NBA shape as a belt).
- Goal and penalty times on screen use **elapsed** time (`plays[].clock`),
  hockey's convention for scoring summaries.

### 3.3 Shots on goal

`sog(side) = saves(other) + goals(side)` from the scoreboard. Absent when
either stat is missing (puck drop's `[]`) — the screen then hides the shots
row rather than showing 0–0. Saturates at 255.

### 3.4 Strength (summary)

1. Goalie ids ← `boxscore.players` goalies group.
2. From `onIce`, per side: `skaters = entries − (goalie present ? 1 : 0)`,
   `pulled = no goalie present`. **Sane** iff both sides have 3–6 skaters.
3. If sane: pulled on either side → `EmptyNet(side)` (wins over PP for the
   banner; the skater dots still show the true counts); skaters differ →
   `PowerPlay(more, more_n, fewer_n)`; equal and < 5 → `Matched(n)`;
   otherwise `Even`.
4. If not sane: the latest play that is **not** a penalty, by its
   team-relative `strength` (Power Play → that team; Shorthanded → the
   other), skater counts unknown (0), and `Even` otherwise. Emit a quirk.
5. Display: `Matched(3)` in the regular-season OT is normal and gets **no**
   banner; `Matched(4)` shows "4 ON 4"; `PowerPlay` shows "POWER PLAY" +
   "a ON d"; `EmptyNet` shows "EMPTY NET" + "6 ON 5" (or the true counts).
   **No countdown anywhere** (§2: none exists; reconstructing one is wrong
   with coincidental minors and PP goals).

### 3.5 Goals and the last goal (summary)

- Goals = `plays` with `scoringPlay` and period ≠ SO, in order. Per goal:
  side, period, elapsed seconds, tag (`ENG` if `shotInfo` 903 or strength
  903; else `PPG` 702; `SHG` 703; else `GOAL`), scorer = participant with
  role `scorer` (fallback: first participant) `shortName`, coordinate.
- Coordinates are normalized so **away attacks +x** (football's
  convention: away moves right): away plots at `(|x|, y·sign(x))`, home at
  `(−|x|, −y·sign(x))` — a 180° rotation, which keeps handedness.
- Keep the newest 16 goals. The bottom-strip "last goal" is the newest.

### 3.6 The play flash

Only goals and penalties flash — hits, faceoffs and stoppages are noise on a
wall. Source: the newest goal/penalty play in the summary (else the
scoreboard `situation.lastPlay`, filtered the same way). Text: ESPN's play
text, prefixed `GOAL! ` for goals. Penalty rows without `penaltyMinutes` or
of the junk shape are dropped (quirk). The id is the play id; the shared
`flash_play` change-detection rule applies unchanged.

### 3.7 Goal lamp

When this device observes the goal count of the game on screen increase
between two of its own commits, the store stamps `lamp_side` + `lamp_ms`.
The first commit of a game never lights the lamp — a goal already on the
board when the game rotates in is old news. The renderer pulses that side's score
white↔lamp-red with `pulse(now − lamp_ms, 1000)` for 20 s on the **wall
rail** (it is a duration). No core-1 state: the anchor lives in the
snapshot, exactly like `play.updated_ms`.

### 3.8 Shootout (summary)

Period-SO plays with a team and type Goal/Shot/Missed are attempts, in
order; `scored = Goal`. Per side keep up to 16 (bitmask + count). The next
shooter alternates from the first shooter; none once a `Shootout End` play
exists. The screen shows at least three slots per side and the newest seven
in sudden death. The game score shown stays the tied score; the tally is
derived from the attempts (never `awayScore`/`homeScore`, which flip to the
tally mid-shootout).

### 3.9 Pregame

Shared "Big time" screen with two hockey deltas: the record stack grows a
third row (OTL, `rgb565(60,60,60)`), filling the crest's 24 px exactly; the
per-team line is the probable starting goalie's `shortName`. Info cycle:
primary = venue; secondary = `notes[0].headline` + " - " + `series.summary`
in the playoffs, else empty (NBA's shape). Records accept `type` `total` or
`ytd`.

### 3.10 Final

Three-stars screen (D2): crests + scores (winner color / loser gray), result
label in accent, three rows of star icons (1/2/3 stars, gold) + `shortName`
in the player's team color, a rule, then `series.summary` (playoffs) and
`SHOTS a-h`. The shared line-score data is still committed (period goals with
every OT collapsed into one OT column — sudden death means only the last can
hold a goal — plus an SO column) so the box-score variant stays one
`apply_variant` away if D2 flips.

## 4. Wire format — `scoreboard-wire/src/hockey.rs`

New sport, new payloads, same `WIRE_VERSION = 2` header (the version byte
gates layout *per endpoint*, and no deployed decoder requests
`/hockey/nhl/*`). Layouts below are the proposal; Phase 1 freezes them with
goldens. Conventions as every sport: little-endian, `u8`-prefixed strings,
flags gate optional strings, no trailing bytes.

**Live (state 1)**, fixed section at offset 2:

| off | type | field |
|-----|------|-------|
| 2 | u8 | flags: b0 last play, b1 strength known, b2 shootout, b3 SOG present, b4 playoffs |
| 3 | u8 | period |
| 4 | u8 | phase (`LivePhase` code) |
| 5 | u16 | away score |
| 7 | u16 | home score |
| 9 | u32×4 | colors (away primary/alt, home primary/alt) |
| 25 | u8 | away SOG |
| 26 | u8 | home SOG |
| 27 | u8 | strength (0 even, 1/2 PP away/home, 3 matched, 4/5 EN away/home) |
| 28 | u8 | away skaters (0 unknown) |
| 29 | u8 | home skaters |
| 30 | u8 | goal count N (≤ 16) |
| 31 | u8 | away SO attempts (≤ 16) |
| 32 | u8 | home SO attempts |
| 33 | u16 | away SO scored bitmask |
| 35 | u16 | home SO scored bitmask |
| 37 | u8 | next shooter (0 none, 1 away, 2 home) |

Then N goal records, 6 bytes each: `u8 side|tag` (b0 side, b1–2 tag),
`u8 period`, `u16 elapsed s`, `i8 x`, `i8 y`. Strings: `game_id`, away
abbr, home abbr, `clock`, iff N > 0 the newest goal's scorer, iff b0 play
id + play text.

**Pregame (state 0)**: NBA's pregame plus `u16` OTL per side, flags for
each side's goalie, b4 playoffs; strings `game_id`, abbrs, `venue`,
`info_secondary`, iff flagged away/home goalie.

**Final (state 2)**: flags (b0 stars, b1 series, b2 SOG, b4 playoffs),
`periods_played`, scores, colors, SOG ×2, star sides (2 bits ×3), the two
raw per-period line scores (the shared `write_final` byte shape for the
period part); strings `game_id`, abbrs, iff b0 three star names, iff b1
series summary.

## 5. Budgets

- Snapshot today 2,848 B (`the_snapshot_stays_inside_its_budget_line`);
  four copies resident (channel 8,552 B + store). `HockeyLiveView` ≈ 140 B
  (16 goals × 4 B projected marks + one scorer `Text<PLAYER>` + shootout
  bits), `HockeyFinalView` ≈ 200 B (three `Text<PLAYER>` + series
  `Text<INFO>`), pregame +4 B → **≈ +350 B per copy, ≈ +1.4 KB total**.
  Update the pinned numbers in the same commit; record in BUDGET.md.
- Flash: the extractor tables + renderer ≈ 15–25 KB (soccer's lane was the
  largest at ~1.5 kLOC); the rink + star sprites < 1 KB.
- Backend: `JSON_CACHE_CAPACITY` (16) was sized for soccer summaries; NHL
  adds a summary per live game shown by any device (up to 16 NHL games a
  night, 85–630 KB each). Raise to 48 and note the Fly machine's memory
  headroom in the same commit.

## 6. Phases (each ends green: `cargo test --workspace`, clippy, thumbv8m builds)

### 6.0 Corpus and fixtures
- Collector: §9.1 (done in config; deploy pending).
- Seed `backend/testdata/hockey/nhl/` now from the 2026-10-01 captures:
  `pregame`, `in_progress`, `end_of_period`, `power_play` (scoreboard +
  summary pairs), plus finals `final`, `final_ot`, `final_so`, `final_2ot`,
  `playoff_clincher`. Shootout *live* and OT *live* were not observed:
  synthesize them by truncating the SO/OT final summaries' `plays` and
  rewriting the status block — mark them `synthetic_` in the filename and
  replace them when the corpus catches the real thing.
- After ~two weeks of collection: `python -m tools.espn discover/schema/spec
  --league nhl`, review presence tables against §2, promote the NHL spec
  into `backend/espn_openapi_combined.yaml` (README playbook).
  `tools/extract_fixtures.py` is still sqlite-bound (BACKLOG 61) — either
  port it first or pull NHL fixtures with a short SQL query.

### 6.1 `scoreboard-wire`
`hockey.rs` per §4: `Game/Pregame/Live/Final`, `encode`/`decode`, round-trip
+ truncation + malformed-input tests in `tests.rs`, goldens under
`backend/testdata/wire/hockey/`.

### 6.2 `scoreboard-espn` (hardest lane — strongest model, line-by-line review)
`src/hockey.rs`: two path tables like soccer's (scoreboard event; summary
plays/onIce/boxscore goalies/featuredAthletes), one extractor, owned
`Extract` with `as_game()`. All sixteen DESIGN.md rulings apply; hockey has
no serde predecessor, so "rejection parity" becomes "required-field rules
written down once" in the module docs. Quirks: penalty junk row, onIce
insane, period-5/`altDetail` disagreement, unknown status name, missing
SOG stats. Tests: corpus extraction, chunk-split invariance, field-order
independence, the §3 rules one named test each (strength fallback,
coordinate normalization, OT collapse, SO next-shooter, ENG detection).

### 6.3 Backend
`src/hockey/{mod,handler,adapter,types,wire}.rs`; `AnyLeague::Nhl`
("hockey"/"nhl") in `espn/league.rs` (+ `VALID_LEAGUES`; the existing
`invalid_pairs…` test uses `hockey/nhl` as its *invalid* example — swap it
for another); routes + OpenAPI schemas + `hockey` tag in `lib.rs`; detail
handler fetches the summary for live and final games with soccer's
degrade-to-None rule (`fetch_commentary`'s shape); `wire_corpus.rs` arms;
logo route needs nothing (payload-resolved). Deploy staging → bench → prod
(owner call).

### 6.4 `scoreboard-model`
`Sport::Hockey`; `LeagueId::from_slug` → key `hockey/nhl`, name `NHL`
(an unknown hockey slug falls back like football/soccer, so a second league
is a table row); `GameDetail::Hockey`; `WireFeed` arm; `Mode::HockeyLive`,
`Mode::HockeyFinal`; `HockeyLiveView` (score, SOG `Option`, period/clock
text, clock accent/low, strength + skater counts, projected goal marks,
last-goal top line + scorer + color, shootout state, lamp side + anchor);
`HockeyFinalView`; `PregameInput::hockey`; model `Record` gains
`ot_losses: Option<u16>`; `commit_hockey_*` with the §3.6 flash filter and
§3.7 lamp stamp; bounds test arms; the size pin.

### 6.5 `scoreboard-render`
`game/hockey.rs` (live A and B, final, shootout inside each live layout);
`geometry.rs` tables `HOCKEY_LIVE_A`, `HOCKEY_LIVE_B`, `HOCKEY_FINAL`,
`HockeyLiveVariant` + `RenderSettings.hockey_live` + `apply_variant`
keys `hockey_live` (A|B) and `hockey_final` (A only today); the pregame
record stack's third row (sport-gated); frame dispatch; `prepared.rs`
untouched unless the stars line scrolls (it does — fixed-rail like the
pregame lines). Art: new `firmware/assets/layout/hockey_layout.aseprite`
with `rink__absolute` (placeholder end colors found by value and tinted
per team — football's endzone mechanism) and `star` — compiled by
`tools/compile_layout.py` like the others. All new screens obey the 1 px
edge rule. Tests: per-screen pixel tests in `tests/game.rs`, and the hockey
wire fixtures added to the golden-frame corpus (`tests/golden_frames/manifest.txt`
rows, then a bless).
Mutation contract: the lamp and PP-zone pulse are pure functions of
snapshot anchors and the wall rail; no renderer state.

### 6.6 Config, input, app
- `scoreboard-config`: `SportsConfig.nhl: SportToggle` (default off),
  `SportsPatch.nhl`, `VariantsConfig.hockey_live` (default `B`) and
  `hockey_final`, defaults, patch + tests.
- `scoreboard-input` menu tests gain the NHL row; label from `LeagueId`.
- `firmware-rs/app`: `sources_from_config` (D6 order), `probe.rs` mode
  names, crest pool unchanged.
- No `scoreboard-direct` work: direct mode is legacy (BACKLOG 100).

### 6.7 Frontend
`SportsCard.svelte`: an NHL row in `ENABLED_SPORTS`, rendered only when
`"nhl" in config.sports` (D7). `ScreenLayoutsCard.svelte`: `hockey_live`
(A Period ledger / B Rink). `types.ts`: `nhl`, `hockey_live`,
`hockey_final`. Rebuild and commit `firmware-rs/app/assets/index.html.gz`.
While there, fix the comments that still name MicroPython files as the
mirror source (`screen_geometry.py`, `football.py LEAGUE_NAMES`) — the Rust
geometry and `feed.rs` are the sources now.

### 6.8 Mock, staging, release
`tools/espn/mock.example.yml` + `infra/fly/mock.staging.yml` gain hockey
scenarios (pregame → live PP → intermission → final SO) from the fixtures;
bench on the staging pair; `publish-fw --channel dev`, soak through a real
NHL night, then `stable`.

### 6.9 Docs
ARCHITECTURE.md ("four sports" → five; backend tree; routes), SPEC.md
(sport list, snapshot), BUDGET.md (§5 numbers), `scoreboard-espn/DESIGN.md`
(hockey lane + the summary table), BACKLOG (52 now five copies).

## 7. Cross-sport cleanups found while designing (each its own change)

C1 and C2 landed ahead of NHL on 2026-10-01, together with the switch to
Rust-owned golden frames (PARITY.md, "Team colors — a luminance floor").

- **C1 Dark crests — landed.** The backend logo route serves ESPN's
  `500-dark` artwork (first `/500/` → `/500-dark/`, `dark_crest_path`) and
  falls back to the default on a 404. Audited across every team in every
  league; one team has no dark file (Coventry City).
- **C2 Luminance floor for team colors — landed.** `Rgb888::brightened`
  now also lifts every team color to luma 80 (not the ~96 first mocked: 96
  turns saturated reds pink). NHL's navies (EDM, TOR, TB, SEA, CBJ, WPG, VAN)
  read as blue.
- **C3 Snapshot views as a sum.** Every sport's view lives in the snapshot
  side by side, though only one is ever shown. An enum would make the
  `Mode`/view pairing unrepresentable-when-wrong and save the difference
  between the sum and the largest view ×4. Measure first; take it only if
  RAM wants it.
- **C4 Line-score OT headers.** NBA/football finals label overtime columns
  "5", "6". Hockey needs a per-sport header function anyway ("OT", "SO");
  NBA/football could adopt "OT" for their first overtime.
- **C5 Shootout markers for soccer penalties** — soccer shows only "PENS";
  the hockey marker row is sport-neutral. Needs the penalty-kick sequence
  from the soccer summary.
- **C6 `GameDetail` boilerplate** — `game_id()`/`abbreviations()` are
  12-arm matches that become 15; a shared header accessor per sport module
  would make them 5.
- **C7 Final "decision line"** — if the cycling final (option 3) is ever
  chosen, the same line serves MLB's W/L/SV pitchers (ESPN's
  `featuredAthletes` exists there too).

## 8. Risks

- **Summary cost on the backend** (D3): 85→630 KB per live game per cache
  window. Bounded by the JSON cache (§5) and the device's poll interval; the
  device itself never sees more than the hockey wire payload.
- Unobserved live states: OT/SO status strings, shootout in progress,
  postponed. The extractor treats unknown status names as a quirk and
  degrades, never rejects a live game for them.
- Penalty format already changed once between seasons; detect by
  `penaltyType` presence **or** type 509.
- `onIce` glitches during stoppages (§3.4 fallback).
- `displayClock` reads "20:00 - 2nd" for minutes before puck drop; accepted.

## 9. Ops

### 9.1 Collector (done in config, 2026-10-01)
`infra/config/targets.yml`: the `nhl` scoreboard target existed since July;
`follow_summaries: true` added so live NHL summaries are captured (the
consumer is this design + §6.2's tables). Validated with
`tools.espn.targets.load_targets`. **Deploy** when the NUC is reachable
(it was not on 2026-10-01 — no ARP response at its address):
`ansible-playbook deploy.yml --tags targets` from WSL (see the deploy
memory/README for the `wsl -e` invocation).

### 9.2 Delegation plan
Orchestrator owns §3 semantics, reviews every diff, runs every test, commits
with explicit pathspecs. Lanes: wire (6.1), espn extractor (6.2 — strongest
model), backend (6.3), model+render (6.4–6.5 together: the view shape is
their shared seam), config/app/frontend (6.6–6.7). 6.1 → 6.2 → 6.3
is a chain; 6.4–6.5 can start against 6.1's types with hand-built fixtures.

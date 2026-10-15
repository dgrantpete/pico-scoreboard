# NHL — end-to-end spec

Status: **decided** (owner, 2026-10-12; drafted 2026-10-01). Implemented so
far: the collector change (§9.1) and the wire format (§4, phase 6.1,
frozen 2026-10-14). Visual decisions
reference the mockup gallery (published Artifact "NHL Panel Mockups") and the
goal-mark study (Artifact "Rink Goal Dots"); every mockup was
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

The owner accepted every recommendation on 2026-10-12, and added D9–D10
from the goal-mark study. The last column keeps what was turned down.

| # | Decision | Decided | Not chosen |
|---|----------|---------|------------|
| D1 | Live layout | **B "Rink" default**, **A "Period ledger" as the `hockey_live` variant** | C "Center ice" (dropped: small clock); A-only |
| D2 | Final screen | **"Three stars"** (own screen, soccer-final silhouette) | Shared line-score final with hockey columns; line score + cycling stars line |
| D3 | Fetch the per-game summary for live/final hockey | **Yes** — on the backend, like soccer's commentary; the device still receives one small wire payload. Live it carries strength, goals, the shootout and the flash; on finals the reliable shots (§3.3) — the stars turned out to come from the scoreboard (§2, 2026-10-14) | Scoreboard-only: no strength, no tags, no shootout tracker, unreliable final shots |
| D4 | Goal lamp (scoring side's number pulses red) | **Yes, 20 s** | No |
| D5 | Sport identity | **`Sport::Hockey`, league slug `nhl`, endpoint `hockey/nhl`, config `sports.nhl: {enabled}`** (single-league toggle, like NBA), **on by default** (owner, 2026-10-15: friends' units should start rotating hockey without anyone opening the settings page) | Multi-league `sports.hockey.leagues[]` — only if a second hockey league is wanted soon |
| D6 | Rotation / menu order | **MLB, NBA, NHL, football…, soccer…** | Append after soccer |
| D7 | MicroPython | **No hockey.** The SPA hides the NHL toggle unless `GET /api/config` returns a `sports.nhl` key (MicroPython's defaults never will) — capability by presence, zero legacy change | Ship hockey parsers to the fleet (not worth it) |
| D8 | Cross-sport fixes found here (§7 C1, C2) | **Separate reviewed changes**, not in the NHL diff (both shipped 2026-10-02) | Fold in |
| D9 | Rink goal marks | **Translucent**: each 2×2 mark composited at 141/256 over what is lit, so stacked goals climb 55% → 80% → 91% toward the team color — a heat map (§6.5) | Solid (stacks fuse into blobs); separated (each goal nudged to the nearest free spot: countable, but reads homogeneous); recency fade (neighbors in time get indistinguishable shades) |
| D10 | Rink faceoff dots | **None** — single red pixels read as a red team's goals | Drawn |

## 2. Data facts (evidence-backed)

Two evidence sets. Design time (2026-10-01): four live preseason games (13
scoreboard polls, 20 live summaries) plus 66 historical dates (Nov 2025, the
2026 playoffs; 242 finals). Phase 6.0 (2026-10-14) re-checked every claim
against two weeks of collector captures — 2026-10-01..14, 24,053 distinct
scoreboard bodies, 99 games, their summaries sampled 1-in-20 and scanned
densely around every state below; counts are from that set unless dated
otherwise. The corpus is `backend/testdata/hockey/nhl/`; its `PROVENANCE.md`
names each pair's polls and evidence.

### Scoreboard (`/apis/site/v2/sports/hockey/nhl/scoreboard`, ~22 KB/game)

- `status.type.name`: exactly five occur — `STATUS_SCHEDULED` (id 1, pre),
  `STATUS_IN_PROGRESS` (id 2, in), `STATUS_END_PERIOD` (id 22, in — **the
  intermission**; no STATUS_INTERMISSION exists), `STATUS_FINAL` (id 3,
  post), and `STATUS_DELAYED` (id 7, **in**: one game, ~8.5 min at period 0
  before a late puck drop, then two polls into period 1). Live overtime and
  the live shootout have **no status of their own**: IN_PROGRESS at period 4
  ("0:39 - OT"), and IN_PROGRESS at period 5 ("In SO", clock 0:00).
  END_PERIOD also follows OT ("End of OT"), the shootout ("End of SO"), and
  a regulation 3rd for ~30 s before FINAL; an IN_PROGRESS "End of …" body at
  0:00 precedes most END_PERIODs.
- `altDetail` on finals: `"OT"` ↔ period 4, `"SO"` ↔ period 5, absent ↔
  period 3 (100% in the regular season); `"2OT"` and up in the playoffs.
- `status.period`: **5 is ambiguous** — shootout in the regular season, 2OT
  in the playoffs. `events[].season.type` (2 regular — preseason is also
  labelled 2 — vs 3 postseason) resolves it; it read 2 on 210,583/210,583
  collector events.
- `status.displayClock`: time **remaining**, moves only when a play is
  logged; reads "20:00" while IN_PROGRESS before every period's puck drop.
  Unreliable post-game (one OT final read "1:16" for 3,201 bodies) and on
  some END_PERIOD bodies (20:00; "End of OT" at 0:45, 1:37, 2:09).
- **Finals flap.** In 2/99 games a final went back for a few polls to
  IN_PROGRESS or END_PERIOD, or to SCHEDULED at period 3 with a wrong score;
  STATUS_FINAL with a tied score appeared in 2 games. The summary header
  flaps with it.
- **Scores flicker.** A score went *down* between consecutive polls 33 times
  in 26/99 games: 16 were restored within 90 s (stale CDN copies), 17 stayed
  down (disallowed goals).
- `competitors[].score`: string. A shootout winner's score **includes the
  +1** — and live it often already does while the status still reads "In SO"
  (5/8 shootouts, and every "End of SO" poll), with the shootout linescore
  sometimes lagging it.
- `team.alternateColor`: absent through 2026-10-14 (0/484 at design time),
  then present on **every** team from 2026-10-14 23:44 UTC (WSH `0b1f41`,
  DAL `000000`). UTA's primary is `000000`. `team.logo` is
  `…/nhl/500/scoreboard/<abbr>.png` — UTA's `uta` (it was `utah` in
  2025-26). `team.logoDark` on 66–75% of events.
- `records[0].summary` is **W-L-OTL** ("53-22-7"), always. Its `type` is
  `"total"` pregame and live and `"ytd"` on finals — accept both. No points.
- `linescores[]`: per period including the current one; each OT its own
  entry; the shootout a fifth entry of 1/0 (winner/loser). Live they lag the
  current period on ~1.8% of entries.
- `statistics[]`: `saves`, `savePct`, `goals`, `ytdGoals`, `assists`,
  `points` — game values live and post, season values pregame (plus
  `plusMinus`); `[]` until the first game stats arrive, ~2 min after puck
  drop. `goals` excludes the shootout goal. **Post-final bodies sometimes
  report `saves` "0" for both teams** (259 polls in 35 games, including the
  last post body of 2), which breaks the shots identity below.
- **Shots on goal ≈ opponent `saves` + own `goals`.** Live it agrees with
  the summary boxscore on 98.5% of sides (off by 1–3 otherwise, either
  source lagging); on finals it matched the play-by-play shot count on
  151/188 sides, against the boxscore's 185/188 (Summary, below).
- `probables[]`: `probableStartingGoalie` with `athlete.shortName` and
  status Confirmed/Expected — one per team on every pregame event
  (53,897/53,897), and still present live and final.
- `competitions[0].status.featuredAthletes` (finals): winningGoalie,
  losingGoalie, firstStar, secondStar, thirdStar, each with `athlete` and
  `team.id` — on 98.5% of post events in 99/99 games, first appearing
  0–649 s after the first FINAL poll (median 170 s), with partial sets on
  the way. **This is the stars source**: the summary header has them on
  only 4/94 collector finals.
- `situation` (live only): `lastPlay` only — **no strength, PP, EN,
  clock**; occasionally `{}`.
- Playoffs: `series.summary` ("CAR leads series 3-2"), `notes[0].headline`
  ("Stanley Cup Final - Game 6" — the only game-number source).
- A DELAYED game at period 0 keeps pregame-shaped competitors (probables,
  records, score "0", no linescores) inside the live competition shape.

### Summary (`…/summary?event=<id>`, 85 KB at puck drop → 300 KB mid-game → 400–630 KB final)

- `plays[]` (~850 B each, 55–70 % of the body): `type{id, text}`, `text`,
  `period{number, displayValue}` ("1st", "OT", "2nd OT", "SO"),
  `clock.displayValue` **elapsed** in the period, `team.id` (missing on
  some plays — below), `scoringPlay`, `participants[]` (`athlete{id,
  shortName}` with **no team**; role `scorer`/`assister`/`shooter`/`saver`/…
  or untyped), `coordinate{x,y}` (ft from center), `strength` on every play
  (701 Even, 702 Power Play, 703 Shorthanded, 902 Penalty Shot, 903 Empty
  Net — goals only), `shotInfo` (901 None, 902, 903 Empty Net, 905
  Shootout), `awayScore`/`homeScore` (inconsistent during the shootout).
- `strength` is **relative to the play's team**, but a penalty play carries
  the strength from **before** it, a pulled goalie (6v5) still reads "Even
  Strength", and team-less plays (Stoppage, Period Start/End, End of Game —
  679 in 94 finals) carry "Power Play"/"Shorthanded" too. Empty-net goals
  carry strength 903 **and** shotInfo 903 (33/33).
- Every non-shootout goal has a team, a coordinate and a scorer participant
  (585/585 in 94 finals).
- Penalties (2026-27): the infraction is the type, with
  `type.penaltyMinutes` / `type.penaltyType` (Minor, Major, Double Minor,
  Misconduct, Bench, PS). **Every penalty is first posted as a placeholder**
  — type id 143 (abbreviation "Closing Hand on the Puck", Minor, 2 min,
  text "<Player> Minor[ against <Player>]"), never used for anything else —
  and rewritten to the real infraction under the same play id one or two
  polls (10–30 s) later: 105 of 120 placeholders; 13 vanished, 2 were never
  rewritten. Penalty-shot calls are 133/136/137 ("… on Breakaway",
  penaltyType PS, no minutes). 2025-26 used type 509 "Penalty".
- `boxscore.players[].statistics[name=goalies]` gives the goalie ids — but
  **every player group is empty for the first ~2 min after puck drop**
  (11–313 s; median 122 s) while `onIce` already lists full units.
  `boxscore.teams[].statistics[name=shotsTotal].displayValue` (team → side
  via `team.id` and `header.competitions[0].competitors`) appears in the
  same poll as the scoreboard's statistics or one later, never while those
  are `[]`; on finals it matched the play-by-play shot count on 185/188
  sides.
- Goalie changes: 12 in 94 finals. The relief goalie is on the ice before
  any boxscore group lists him for 0–80 s (median ~52 s); the scoreboard's
  probable goalie was the starter in 180/180 captured team-games. No
  document names a dressed backup before he plays (no `rosters` block;
  `leaders`, `header`, `gameInfo` carry no players).
- `onIce[]`: entries carry only `athleteid`, `whereabouts` ("In Play") and a
  `projections` link — no position, and no ordering puts the goalie first.
  Entries **include the goalie**; counts give strength (6v6, 6v5
  on a PP, 4v4 in 3-on-3 OT); a pulled goalie = no goalie among the entries.
  Insane on 0.14% of sampled polls (a 3-entry side during a fight stoppage,
  an empty side); `[]` at period starts and for ~40 s into OT; may still
  show the 3rd period's unit early in OT; keeps the last unit through an
  intermission (a carried-over power play shows through END_PERIOD); holds
  only the goalies, or nothing, during the shootout.
- Shootout plays (`period.displayValue` "SO"): the attempts are types 505
  Goal, 506 Shot, 507 Missed and 2010 "failed shot attempt" (62 attempts in
  8 shootouts; no other shootout play has a team); 518 Period Start, 521
  Shootout End, 519 Period End and 522 End of Game frame them. An attempt
  can arrive **without a team** (5 seen live, 2 surviving into finals) —
  sometimes with only the `saver` among its participants, once with nothing
  but a coordinate — and its outcome can change in place (Shot → Goal under
  the same id).
- `header.competitions[0].status.featuredAthletes`: as on the scoreboard,
  but late — 4/94 collector finals.
- **Not anywhere:** a power-play clock or penalty box, an intermission
  clock, points (scoreboard), reliable shootout box stats.

## 3. Semantics (normative)

Revised 2026-10-14 against the phase 6.0 evidence (§2). Each rule below that
handles a quirk names it.

### 3.1 States and periods

- Dispatch on `status.type.state`: pre → Pregame; post → Final; in → Live
  with `LivePhase::InProgress`, or `LivePhase::EndOfPeriod` for
  `STATUS_END_PERIOD`. (`Halftime` is unused.) **Except** `STATUS_DELAYED`
  at period 0 → Pregame: the puck has not dropped and the competitors are
  still pregame-shaped (§2); DELAYED at period ≥ 1 is Live `InProgress`. An
  unknown status name is a quirk and follows its `state`.
- Playoffs ⇔ `season.type` 3. **The shootout ⇔ regular season and period
  ≥ 5** — never "period 5" alone, which is 2OT in the playoffs.
- Period label: 1–3 → `1ST`/`2ND`/`3RD`. Regular season: 4 → `OT`, 5 →
  `SO`. Playoffs: 4 → `OT`, n ≥ 5 → `{n-3}OT`.
- Final label: `FINAL`, `FINAL/OT`, `FINAL/SO`, `F/{n}OT` (n ≥ 2). Derive
  from period + season kind; `altDetail` is the cross-check (a disagreement
  is a quirk, not a rejection).
- Flapping finals (§2) are served as ESPN states them: the backend answers
  each request from the body it fetched, and the device follows ESPN back
  within a poll. A FINAL with tied scores is a quirk.

### 3.2 Clock

- Drawn verbatim from `displayClock` and never extrapolated (NBA rule:
  hockey's clock stops on whistles and the feed only moves on plays).
- `EndOfPeriod` → the clock slot reads `INT` in the accent color when
  another period follows, and `END` when none can: after the shootout, or
  after period ≥ 3 with the scores unequal (the half minute of END_PERIOD
  between a decided 3rd and FINAL). The period chip keeps the period that
  just ended.
- During a live shootout the clock slot shows the shootout tally (§3.8)
  instead of `displayClock`, which sits at 0:00.
- Warning color when `InProgress`, period ≥ 3, and the clock is sub-minute
  (`0:` prefix, or the colonless NBA shape as a belt).
- Goal and penalty times on screen use **elapsed** time (`plays[].clock`),
  hockey's convention for scoring summaries.

### 3.3 Shots on goal

The first source that applies:

1. The summary's `boxscore.teams[].statistics[name=shotsTotal]`, present
   for both teams.
2. The scoreboard identity `sog(side) = saves(other) + goals(side)`, both
   teams' `statistics` non-empty — except on a final where both teams report
   `saves` "0" (the post-final quirk, §2), which counts as missing.
3. Absent: the screen hides the shots row rather than show 0–0.

Saturates at 255. Live the two sources agree on 98.5% of sides and neither
is authoritative; on finals the boxscore is (185/188 against 151/188), which
is part of why D3 fetches the summary for finals too.

### 3.4 Strength (summary)

1. Per team: **known goalies** = the `boxscore.players` goalies group ∪ the
   scoreboard's probable starting goalie (its `athlete.id`, handed to the
   summary pass with the season kind); **listed players** = every id in that
   team's `boxscore.players` groups. The goalies group alone is not enough:
   it lists only goalies who have *played*, so it is empty for the first
   ~2 min (puck drop) and lags every goalie change by up to ~80 s, and a
   relief goalie on the ice before the boxscore lists him reads as an empty
   net (11 of 12 changes in the phase 6.0 fortnight).
2. Per side from `onIce` (entries carry ids only — no position): the goalie
   is **present** if a known goalie is among the entries, or — the relief
   goalie — if some entry is not a listed player of that team; otherwise the
   side is **pulled**. `skaters = entries − (goalie present ? 1 : 0)`.
   **Sane** iff the team has at least one known goalie, both sides have 3–6
   skaters, and at most one side is pulled (two empty nets at once is not
   hockey). Measured over the fortnight against the goalies who really
   played: every one of 209 real goalie-less polls found, no false empty net
   (the goalies-group-only rule produced 348).
3. During an intermission (`EndOfPeriod`) a pulled side is not believed:
   `onIce` freezes the last unit, including an extra attacker's, through the
   break. If either side reads pulled there, there is no strength (the group
   is absent); a power play carrying into the next period still shows.
4. If sane: pulled on either side → `EmptyNet(side)` (wins over PP for the
   banner; the skater dots still show the true counts); skaters differ →
   `PowerPlay(more, more_n, fewer_n)`; equal and < 5 → `Matched(n)`;
   otherwise `Even`.
5. If not sane: the latest play that is **not** a penalty (penaltyType
   present, type 509, or the 143 placeholder) and **has a team**, by its
   team-relative `strength` (Power Play → that team; Shorthanded → the
   other; anything else `Even`), skater counts unknown (0). Team-less plays
   are skipped: they carry strengths no side can be read from (§2). Emit a
   quirk — except for `onIce` `[]` at a period start, which is normal.
6. During a live shootout there is no strength (the group is absent):
   `onIce` holds only the goalies.
7. Display: `Matched(3)` in the regular-season OT is normal and gets **no**
   banner; `Matched(4)` shows "4 ON 4"; `PowerPlay` shows "POWER PLAY" +
   "a ON d"; `EmptyNet` shows "EMPTY NET" + "6 ON 5" (or the true counts).
   **No countdown anywhere** (§2: none exists; reconstructing one is wrong
   with coincidental minors and PP goals).

### 3.5 Goals and the last goal (summary)

- Goals = `plays` with `scoringPlay` outside the shootout (§3.1), in order.
  Per goal: side (the play's team), period, elapsed seconds, tag (`ENG` if
  `shotInfo` 903 or strength 903; else `PPG` 702; `SHG` 703; else `GOAL` —
  a 902 penalty-shot goal is a `GOAL`), scorer = participant with role
  `scorer` (fallback: first participant) `shortName`, coordinate (absent
  when ESPN logged none: §4's coordinate bit).
- Coordinates are normalized so **away attacks +x** (football's
  convention: away moves right): away plots at `(|x|, y·sign(x))`, home at
  `(−|x|, −y·sign(x))` — a 180° rotation, which keeps handedness — with
  `sign(0) = +1`, so a goal on the center line keeps its y rather than
  collapsing onto the axis.
- Keep the newest 16 goals. The bottom-strip "last goal" is the newest goal
  kept, and the scorer shown is that goal's: a goal dropped for an
  unplaceable team never leaves a blank scorer beside an older goal.

### 3.6 The play flash

Only goals (§3.5's — shootout attempts are the tracker's, never the flash's)
and penalties flash — hits, faceoffs and stoppages are noise on a wall. Source: the newest eligible goal or penalty play in the summary (else
the scoreboard `situation.lastPlay`, filtered the same way as far as its
fields allow). Text: ESPN's play text, prefixed `GOAL! ` for goals.
**Not eligible:** the placeholder penalty (type 143 — every penalty's first
appearance, §2; its rewrite keeps the play id, which is new to `flash_play`
because the placeholder never flashed, so the real infraction still flashes
exactly once) and penalty rows without `penaltyMinutes` (penalty-shot
calls). Both are expected shapes, not quirks. The id is the play id; the
shared `flash_play` change-detection rule applies unchanged. (Cost, measured:
one real penalty in 99 games that ESPN never rewrote does not flash.)

### 3.7 Goal lamp

The store keeps, for the game on screen, a per-side **high-water mark** of
the score:

- The first commit of a game sets both marks and lights nothing — a goal
  already on the board when the game rotates in is old news. Rotating away
  and back is a first commit again.
- A side's score **above** its mark lights that side (`lamp_side`,
  `lamp_ms = now`; if both rise in one commit, the newest goal's side, or
  home when there are no goals) and raises the mark.
- A score **below** its mark lowers the mark only once it has stayed below
  for `LAMP_SETTLE` (120 s): ESPN's stale copies are restored within 90 s
  and must not relight an old goal, while a disallowed goal stays down and
  must not swallow the next real one (§2's flicker counts). A real goal
  within 120 s of a disallowed one does not light — a missed lamp is benign,
  a false one is not.

The renderer pulses that side's score white↔lamp-red with
`pulse(now − lamp_ms, 1000)` for 20 s on the **wall rail** (it is a
duration). No core-1 state: the anchor lives in the snapshot, exactly like
`play.updated_ms`. Shootout attempts never light it: the game score stays
tied through a shootout (§3.8).

### 3.8 Shootout (summary)

- Attempts: plays in the shootout (§3.1) of type 505 Goal, 506 Shot, 507
  Missed or 2010 failed shot attempt, in order; `scored` = 505. Outcomes
  change in place (Shot → Goal under one id), so every poll derives the
  attempts afresh.
- The shooter's side, first that applies: the play's team; else a
  participant found in a team's goalie group (that team defended, so the
  other shot); else alternation — shooters strictly alternate, so an attempt
  takes its side from the nearest placed attempt, forwards or backwards (a
  run of unplaced attempts at the start is as determined as one in the
  middle). Only when no attempt at all can be placed are they dropped, with
  a quirk.
- Per side keep the **first** 16 attempts (bitmask + count): the tally stays
  right through round 16; past it — beyond the longest NHL shootout on
  record — the markers freeze.
- Next shooter: the side opposite the last attempt's; none once a 521
  `Shootout End` play exists.
- The game score stays the tied score: during a live shootout both scores
  are sent as `min(away, home)`, because ESPN's live score often already
  carries the winner's +1 (§2). The tally is derived from the attempts —
  never from `awayScore`/`homeScore`.
- The screen shows at least three slots per side and the newest seven in
  sudden death.

### 3.9 Pregame

Shared "Big time" screen with two hockey deltas: the record stack grows a
third row (OTL, `rgb565(60,60,60)`), filling the crest's 24 px exactly; the
per-team line is the probable starting goalie's `shortName`. Info cycle:
primary = venue; secondary = `notes[0].headline` + " - " + `series.summary`
in the playoffs, else empty (NBA's shape). Records accept `type` `total` or
`ytd`. Team colors follow the shared rule (`alternateColor` now exists;
nothing hockey-specific). A delayed start lands here (§3.1).

### 3.10 Final

Three-stars screen (D2): crests + scores (winner color / loser gray), result
label in accent, three rows of star icons (1/2/3 stars, gold) + `shortName`
in the player's team color, a rule, then `series.summary` (playoffs only —
a regular-season final never shows one) and
`SHOTS a-h` (§3.3). The stars come from the **scoreboard** event's
`featuredAthletes` (`firstStar`/`secondStar`/`thirdStar`, side from
`team.id`), all three or none; they arrive minutes after FINAL, so the
screen without stars must read as complete, not as a stars screen missing
its stars. The shared line-score data is still committed (period goals with
every OT collapsed into one OT column — sudden death means only the last can
hold a goal — plus an SO column) so the box-score variant stays one
`apply_variant` away if D2 flips.

## 4. Wire format — `scoreboard-wire/src/hockey.rs`

**Frozen** (phase 6.1, 2026-10-14). The crate module is normative; this
section mirrors it for the lanes that implement against it. New sport, new
payloads, same `WIRE_VERSION = 2` header: the version byte gates layout *per
endpoint*, and no deployed decoder has ever requested `/hockey/nhl/*`, so
there is no `ext=1` extension — the first layout carries everything.
Conventions as every sport: little-endian, `u8`-prefixed UTF-8 strings, flags
gate optional groups and strings, a group whose flag is unset is zeroed, no
trailing bytes. Bit 4 of every state's flags is "playoffs": it is what tells
a regular-season shootout from a playoff 2OT at period 5 (§2).

**Live (state 1)** — fixed 36 bytes at offset 2 (`<BBBHHIIIIBBBBBBBBHHB`):
NBA live's 23 bytes, then hockey's.

| off | type | field |
|-----|------|-------|
| 2 | u8 | flags: b0 last play, b1 strength, b2 shootout, b3 SOG, b4 playoffs; b5–7 zero |
| 3 | u8 | period, ESPN's raw number (§3.1) |
| 4 | u8 | phase (`LivePhase`: 0 in progress, 2 end of period = the intermission; 1 is never sent) |
| 5 | u16 | away score (the tied game score during a shootout, §3.8) |
| 7 | u16 | home score |
| 9 | u32×4 | colors (away primary/alt, home primary/alt) |
| 25 | u8 | away SOG (b3) |
| 26 | u8 | home SOG |
| 27 | u8 | strength (b1): 0 even, 1/2 power play away/home (the side with more skaters), 3 matched, 4/5 empty net away/home (the side whose goalie is pulled) |
| 28 | u8 | away skaters (b1; 0 unknown) |
| 29 | u8 | home skaters |
| 30 | u8 | goal count N, 0..=16 |
| 31 | u8 | away SO attempts (b2), 0..=16 |
| 32 | u8 | home SO attempts |
| 33 | u16 | away SO goals: bit i = attempt i scored, first attempt bit 0 |
| 35 | u16 | home SO goals |
| 37 | u8 | next shooter (b2): 0 none (the shootout is over), 1 away, 2 home |

Then N goal records from offset 38, oldest first, 6 bytes each (`<BBHbb`):

| off | type | field |
|-----|------|-------|
| +0 | u8 | b0 side (0 away, 1 home), b1–2 tag (0 GOAL, 1 PPG, 2 SHG, 3 ENG), b3 coordinate present; b4–7 zero |
| +1 | u8 | period |
| +2 | u16 | elapsed seconds in the period |
| +4 | i8 | x, §3.5's normalized feet, −100..=100 (0 when b3 is clear) |
| +5 | i8 | y, −43..=43 (0 when b3 is clear) |

The coordinate bit is for a goal ESPN logs without one: it keeps its slot in
the count and can still be the last goal, it just gets no rink mark.

Strings after the last record: `game_id`, away abbr, home abbr, `clock`,
iff N > 0 the newest goal's scorer, iff b0 play id + play text.

**Pregame (state 0)** — fixed 33 bytes at offset 2 (`<BHHHHIIIIIHH`): NBA
pregame's 29 bytes, then `u16` away OTL at 31 and `u16` home OTL at 33.
Flags: b0/b1 away/home record (W, L and OTL travel together), b2/b3
away/home goalie (football's team-line bits), b4 playoffs; b5–7 zero.
Strings from offset 35: `game_id`, away abbr, home abbr, `venue`,
`info_secondary` (always present, empty outside the playoffs), iff b2 away
goalie, iff b3 home goalie.

**Final (state 2)** — a 4-byte hockey prefix at offset 2, then the shared
final (`common::write_final`, the MLB/NBA/football bytes) whole from offset 6:

| off | type | field |
|-----|------|-------|
| 2 | u8 | flags: b0 stars, b1 series, b2 SOG, b4 playoffs; b3 and b5–7 zero |
| 3 | u8 | away SOG (b2) |
| 4 | u8 | home SOG |
| 5 | u8 | star sides (b0): 2 bits per star, first star lowest — 0 unknown, 1 away, 2 home; b6–7 zero |
| 6 | u8 | periods_played, ESPN's raw count |
| 7 | u8 | away line-score length |
| 8 | u8 | home line-score length |
| 9 | u16 | away score (a shootout winner's includes the +1) |
| 11 | u16 | home score |
| 13 | u32×4 | colors |

Then the raw per-period line scores (away, home: each OT its own entry, the
shootout a fifth period of 1/0), `game_id`, away abbr, home abbr (the shared
final ends here), iff b0 the first, second and third star names, iff b1 the
series summary.

**Encode** clamps rather than fails, as the format does everywhere: the
newest 16 goals; at most 16 SO attempts per side, with goal bits past the
count dropped; coordinates pulled back to the boards. Whatever encodes,
decodes.

**Decode rejects:** a flag bit the state leaves undefined; an unknown phase,
strength, next-shooter or star-side code; a goal record with b4–7 set;
N > 16; SO attempts > 16; an SO goal bit at or past its side's attempt count;
a present coordinate outside ±100 / ±43; truncation anywhere; trailing
bytes. A group whose flag is unset is a placeholder — decoded as absent and
never validated (football's situation rule).

## 5. Budgets

- Snapshot today 2,848 B (`the_snapshot_stays_inside_its_budget_line`);
  four copies resident (channel 8,552 B + store). `HockeyLiveView` ≈ 140 B
  (16 goals × 4 B projected marks + one scorer `Text<PLAYER>` + shootout
  bits), `HockeyFinalView` ≈ 200 B (three `Text<PLAYER>` + series
  `Text<INFO>`), pregame +4 B → **≈ +350 B per copy, ≈ +1.4 KB total**.
  Update the pinned numbers in the same commit; record in BUDGET.md.
- Flash: the extractor tables + renderer ≈ 15–25 KB (soccer's lane was the
  largest at ~1.5 kLOC); the rink + star sprites < 1 KB.
- Wire (§4): a live game with 6 goals and a goal line is ≈ 215 B; 16 goals
  and a 255-byte play line ≈ 460 B, well inside the receive buffer's
  2,048-B detail half (`poll::DETAIL_BYTES`). Pregame and final run
  55–140 B.
- Backend: `JSON_CACHE_CAPACITY` (16) was sized for soccer summaries; NHL
  adds a summary per live game shown by any device (up to 16 NHL games a
  night, 85–630 KB each). Raise to 48 and note the Fly machine's memory
  headroom in the same commit.

**Measured** (2026-10-15; BUDGET.md carries each number and how to
reproduce it):

- Snapshot 3,224 B, +400 B a slot against the +350 B estimate.
- Flash +30,832 B on the device image. None of it is the extractor, which
  runs only on the backend: the model, the screens and the wire decode alone
  pass the 15–25 KB estimate. The rink and star sprites are 955 B.
- Wire: the corpus runs 89–101 B pregame, 60–221 B live and 57–110 B final.
- Backend cache: 48 entries, ≈ 30 MB resident worst case (a release build
  measured 5.4 → 35.4 MB private) against the Fly machine's 256 MB.

## 6. Phases (each ends green: `cargo test --workspace`, clippy, thumbv8m builds)

### 6.0 Corpus and fixtures — done 2026-10-14
- `backend/testdata/hockey/nhl/`: twenty `<name>.json` (scoreboard event)
  + `<name>.summary.json` (summary body) pairs from the collector's first two
  weeks and the design-time playoff captures — pregame, puck drop, in
  progress, intermission, power play, empty net, 4-on-4, 3-on-3 overtime, a
  shootout in progress, an `onIce` glitch, and finals in regulation, OT, SO,
  2OT and a Cup clincher — plus five that pin quirks (a delayed start, the
  placeholder penalty, a team-less and a failed shootout attempt, a shootout
  final before its stars). Every live state was observed for real; nothing
  is synthetic. `PROVENANCE.md` records each pair and how to refresh them.
  Harness note: a `.summary.json` is not an event fixture, so every corpus
  walk skips it.
- The NHL scoreboard schema is in `backend/espn_openapi_combined.yaml`
  (validated 24,055/24,055 bodies). The re-check of §2 rewrote §2 and §3
  (2026-10-14); the counts there are from it.

### 6.1 `scoreboard-wire` — done 2026-10-14
`hockey.rs` per §4: `Game/Pregame/Live/Final`, `encode`/`decode`, round-trip
+ truncation + malformed-input tests and hand-checked hex goldens in
`tests.rs`. The corpus goldens under `backend/testdata/wire/hockey/` come
from real fixtures with 6.3's `wire_corpus.rs` arms.

### 6.2 `scoreboard-espn` — done 2026-10-15 (hardest lane — strongest model, line-by-line review)
`src/hockey.rs`: two path tables like soccer's (scoreboard event; summary
plays/onIce/boxscore goalies/featuredAthletes), one extractor, owned
`Extract` with `as_game()`. All sixteen DESIGN.md rulings apply; hockey has
no serde predecessor, so "rejection parity" becomes "required-field rules
written down once" in the module docs. Quirks (§3 says which are expected
and stay quiet): onIce insane, period-5/`altDetail` disagreement, unknown
status name, FINAL with tied scores, an unplaceable shootout attempt.
Tests: corpus extraction over every pair, chunk-split invariance,
field-order independence, and each §3 rule one named test — the strength
fallback (incl. team-less plays and puck drop's empty goalie groups),
coordinate normalization, OT collapse, ENG/PPG/SHG/penalty-shot tags, the
143 placeholder, shots source order (incl. post-final saves "0"), delayed
start → pregame, the shootout's attempt types, side attribution order,
first-16 rule, next shooter and tied score, and stars from the scoreboard.

### 6.3 Backend — done 2026-10-15 (the deploy is §6.8's)
`src/hockey/{mod,handler,adapter,types,wire}.rs`; `AnyLeague::Nhl`
("hockey"/"nhl") in `espn/league.rs` (+ `VALID_LEAGUES`; the existing
`invalid_pairs…` test uses `hockey/nhl` as its *invalid* example — swap it
for another); routes + OpenAPI schemas + `hockey` tag in `lib.rs`; detail
handler fetches the summary for live and final games with soccer's
degrade-to-None rule (`fetch_commentary`'s shape); `wire_corpus.rs` arms;
logo route needs nothing (payload-resolved). Deploy staging → bench → prod
(owner call).

### 6.4 `scoreboard-model` — done 2026-10-14
`Sport::Hockey`; `LeagueId::from_slug` → key `hockey/nhl`, name `NHL`
(an unknown hockey slug falls back like football/soccer, so a second league
is a table row); `GameDetail::Hockey`; `WireFeed` arm; `Mode::HockeyLive`,
`Mode::HockeyFinal`; `HockeyLiveView` (score, SOG `Option`, period/clock
text, clock accent/low, strength + skater counts, projected goal marks,
last-goal top line + scorer + color, shootout state, lamp side + anchor);
`HockeyFinalView`; `PregameInput::hockey`; model `Record` gains
`ot_losses: Option<u16>`; `commit_hockey_*` with the §3.6 flash filter and
§3.7 lamp stamp; bounds test arms; the size pin.

### 6.5 `scoreboard-render` — done 2026-10-15 (screens 2026-10-14, golden frames 2026-10-15)
`game/hockey.rs` (live A and B, final, shootout inside each live layout);
`geometry.rs` tables `HOCKEY_LIVE_A`, `HOCKEY_LIVE_B`, `HOCKEY_FINAL`,
`HockeyLiveVariant` + `RenderSettings.hockey_live` + `apply_variant`
keys `hockey_live` (A|B) and `hockey_final` (A only today); each hockey
live table carries its own `strip`, wired into
`RenderSettings::bottom_strip`: A uses `COLUMN_STRIP`; B, whose rink frees
the full width like football's field, uses `rect(1, 47, 126, 16)`.
unscii_16 inks rows 1–12 of its cell, so at y 47 the play text sits four
dark rows below the shots bar and the scores' ink (both end at row 43) and
four above the panel's bottom, the dark edge row included: centered in the
space, never touching (owner, 2026-10-12). The rink's goal marks (D9): a
2×2 at the §3.5 projected point, each pixel `mix(under, team, 141)` read
back through `Canvas::pixel_at`, drawn in play order; the newest blinks
opaque white for the lamp window, as in the mockups. The pregame
record stack's third row (sport-gated); frame dispatch; `prepared.rs`
untouched unless the stars line scrolls (it does — fixed-rail like the
pregame lines). Art: new `firmware/assets/layout/hockey_layout.aseprite`
with `rink__absolute` (placeholder end colors found by value and tinted
per team — football's endzone mechanism; boards, goal and blue lines,
center line and creases, no faceoff dots per D10) and `star` — compiled by
`tools/compile_layout.py` like the others. All new screens obey the 1 px
edge rule. Tests: per-screen pixel tests in `tests/game.rs`, and the hockey
wire fixtures added to the golden-frame corpus (`tests/golden_frames/manifest.txt`
rows, then a bless).
Mutation contract: the lamp and PP-zone pulse are pure functions of
snapshot anchors and the wall rail; no renderer state.

### 6.6 Config, input, app
- `scoreboard-config`: `SportsConfig.nhl: SportToggle` (default **on**, D5),
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

### 6.8 Mock, staging, release — mock and configs done 2026-10-15
`tools/espn/mock.example.yml` + `infra/fly/mock.staging.yml` gain hockey
scenarios (pregame → live PP → intermission → final SO) from the fixtures;
bench on the staging pair; `publish-fw --channel dev`, soak through a real
NHL night, then `stable`.

Done: scenario mode serves a fixture's paired `<name>.summary.json`
verbatim as that event's summary (soccer's `commentary` line is unchanged,
and an event cannot have both). The loader rejects a pair whose summary header
disagrees with its event on id or status. The mock rewrites only the
scoreboard event's dates, so a pair that agrees on disk agrees when served,
and the summary keeps its capture-day dates, which the backend never reads.
Both configs carry the same `nhl-demo` slate, twelve teams, each screen
once: `pregame` (both goalies), `power_play`, `end_of_period`,
`shootout_teamless_attempt` (live, 1–1 in the tally, SEA to shoot;
`shootout` shares its event id with `final_so_no_stars`), `final` (three
stars) and `final_so_no_stars`. A local backend over the mock served every
game, JSON and struct, and each struct payload decoded through
`scoreboard_wire` in the state its fixture holds. Still to do: deploy the
staging pair, bench, `publish-fw --channel dev`, the soak, `stable`.

### 6.9 Docs — done 2026-10-15, except ARCHITECTURE.md
ARCHITECTURE.md ("four sports" → five; backend tree; routes), SPEC.md
(sport list, snapshot), BUDGET.md (§5 numbers), `scoreboard-espn/DESIGN.md`
(hockey lane + the summary table), BACKLOG (52 now five copies).

Done: SPEC.md (§3's five sports, §4 and §11's measured snapshot, §6's
Rust-only sport); BUDGET.md (§5's numbers, summarized in §5's "Measured");
DESIGN.md's hockey lane, which landed with 6.2; BACKLOG 52 recounted (five
copies of the league registry, two of them frozen with the MicroPython
tree) and §7's open cleanups as items 103–107. ARCHITECTURE.md is gitignored
and updated outside the repo.

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
  RAM wants it. **Open: BACKLOG 103.**
- **C4 Line-score OT headers.** NBA/football finals label overtime columns
  "5", "6". Hockey needs a per-sport header function anyway ("OT", "SO");
  NBA/football could adopt "OT" for their first overtime. **Hockey's half
  landed with 6.4** (`build_hockey_linescore`); **NBA/football: BACKLOG
  104.**
- **C5 Shootout markers for soccer penalties** — soccer shows only "PENS";
  the hockey marker row is sport-neutral. Needs the penalty-kick sequence
  from the soccer summary. **Open: BACKLOG 105.**
- **C6 `GameDetail` boilerplate** — `game_id()`/`abbreviations()` are
  12-arm matches that become 15; a shared header accessor per sport module
  would make them 5. **Open: BACKLOG 106** (they are 15 now).
- **C7 Final "decision line"** — if the cycling final (option 3) is ever
  chosen, the same line serves MLB's W/L/SV pitchers (ESPN's
  `featuredAthletes` exists there too). **Conditional: BACKLOG 107.**

## 8. Risks

- **Summary cost on the backend** (D3): 85→630 KB per live game per cache
  window. Bounded by the JSON cache (§5) and the device's poll interval; the
  device itself never sees more than the hockey wire payload.
- Still unobserved: a postponed game, and playoff live states (the
  collector has run only in the regular season — period 5 as 2OT live, the
  playoff pregame's series line). The extractor treats unknown status names
  as a quirk and degrades, never rejects a live game for them. Re-check §2
  against the first playoff fortnight.
- Penalty format already changed once between seasons; detect by
  `penaltyType` presence **or** type 509; the 143 placeholder (§2) is a
  2026-27 shape that may change again.
- `onIce` glitches during stoppages and goes stale at the start of OT (§3.4
  fallback; a carried-over unit can show a power play for ~40 s).
- ESPN's own inconsistencies the device shows as ESPN states them: finals
  that flap back for a few polls, and scores that flicker (§2) — the lamp's
  settle window (§3.7) is the only place that compensates.
- `displayClock` reads "20:00" while IN_PROGRESS for minutes before each
  puck drop; accepted.

## 9. Ops

### 9.1 Collector (deployed 2026-10-02)
`infra/config/targets.yml`: the `nhl` scoreboard target existed since July;
`follow_summaries: true` added so live NHL summaries are captured (the
consumer is this design + §6.2's tables). Validated with
`tools.espn.targets.load_targets`. Deployed 2026-10-02 01:41 UTC
(`ansible-playbook deploy.yml --tags targets` from WSL). By 2026-10-12 the
collector held 78 regular-season games (62 regulation, 11 OT, 5 shootout),
73 of them replay-grade (`python -m tools.espn coverage --league nhl`).

### 9.2 Delegation plan
Orchestrator owns §3 semantics, reviews every diff, runs every test, commits
with explicit pathspecs. Lanes: wire (6.1), espn extractor (6.2 — strongest
model), backend (6.3), model+render (6.4–6.5 together: the view shape is
their shared seam), config/app/frontend (6.6–6.7). 6.1 → 6.2 → 6.3
is a chain; 6.4–6.5 can start against 6.1's types with hand-built fixtures.

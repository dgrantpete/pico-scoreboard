# NBA fixture corpus — provenance

Phase 6.0 of `firmware-rs/NBA.md`. Every live and final fixture is a
time-consistent pair, hockey's convention
(`backend/testdata/hockey/nhl/PROVENANCE.md`):

- `<name>.json` — the event object from one scoreboard poll, written as
  `json.dumps(event, indent=2)` of the parsed scoreboard body (key order as
  served; the same convention as every other sport's corpus).
- `<name>.summary.json` — the summary body for the same event at the same
  moment, the **raw bytes as served**. The design poller, the collector and
  the historical fetch all saved bodies unmodified (compact JSON, no
  pretty-printing), so no half was re-serialized and every summary's byte
  count is the one the capture logged.

Pregame has no summary (D3). Harness note: `<name>.summary.json` ends in
`.json`, so every corpus walk (`wire_corpus.rs::fixture_names`, the espn and
direct parity gates) skips the `.summary` stem — a summary is never an event
fixture, for any sport.

## Sources

1. The design captures, `data/nba-design/espn-captures-2026-11-07/`
   (gitignored, local; `ESPN-NBA-FINDINGS.md` §9 indexes them): 268 distinct
   live-slate scoreboard bodies polled every 20 s from 2026-11-07 23:58:55 to
   2026-11-08 01:47:36 UTC (seven games: DEN@IND from the end of Q3 to its
   final, BKN@ATL, NY@CLE, WSH@OKC and ORL@GS from their tips, HOU@PHX and
   SA@POR still scheduled), 175 live and post summaries fetched once a minute
   per live game, 41 historical scoreboards and 5 complete historical
   summaries (fetched 2026-11-08 00:00–00:01 UTC).
2. The home-lab collector (`tools/espn`, the NUC's Postgres), which began
   keeping NBA summaries at 2026-11-08 03:15 UTC: its first night was
   reachable at selection time and held the second half of the same slate
   (NY@CLE and WSH@OKC from late Q4, ORL@GS from Q3 through overtime, HOU@PHX
   from halftime, SA@POR from Q1). Scoreboard and per-game summary polls both
   arrive every 10.1 s (median).

## Selection method (to refresh the corpus)

1. Index every event of every scoreboard body (status, period,
   `displayClock`, both scores, `situation`/`lastPlay` type, `statistics`
   lengths, `leaders`) and every summary (header status, period, clock and
   scores; the ledger per side; play count; the §3.5 run from the running
   scores; the newest play). The scripts live in the session scratchpad, like
   hockey's.
2. Pick candidate scoreboard polls per state from the index and pair each
   with the same event's summary polls within ±120 s. Live pairs must agree
   on status name, period, `displayClock` and both scores; a final summary's
   header carries no period or clock, so final pairs agree on status name and
   both scores. Of the 175 design-capture summaries, 142 pair fully with a
   scoreboard poll within 60 s; 27 differ only in the header clock (a stale
   sample while the clock runs, §2) and 6 have no scoreboard poll near
   enough. Every pair below is a full agreement.
3. From the collector: sample each game's summary stream at a stride of
   twelve distinct bodies to find the windows wanted (the end of the 4th,
   the overtime tip, the final), then mirror each window poll by poll —
   summary and scoreboard bodies — and pair as in step 2. Summary bodies are
   fetched selectively; never in bulk.
4. Prefer the smallest |Δ|, then the clearest evidence. Four fixtures that
   tests and the mock need to be distinct games are: `pregame` (SA@POR),
   `in_progress` (NY@CLE), `halftime` (BKN@ATL) and `final` (DEN@IND).

Δ is the scoreboard poll time minus the summary poll time, in seconds (the
design poller's stamps are whole seconds; the collector's carry
milliseconds). Times are UTC; "game day" is ESPN's `dates=` (US/Eastern)
day. Sizes are bytes: the whole scoreboard body as served / this event's
fixture / the summary. DEN@IND was the NBA Mexico City Game (`neutralSite`,
`shortName` "DEN VS IND", `notes[0].headline` "NBA Mexico City Game").

## Required states

| Fixture | Event | Matchup | Game day | Scoreboard poll | Summary poll | Δ s | Sizes | Evidence |
|---|---|---|---|---|---|---|---|---|
| `pregame` | 401909944 | SA @ POR | 20261107 | 2026-11-07T23:58:55 | — | — | 134,466 / 40,136 / — | `STATUS_SCHEDULED` (id 1), period 0, `detail` "Sat, November 7th at 10:00 PM EST", venue Moda Center. `records[0]` type `total` "7-2" / "6-2". `odds[0]` Draft Kings, `details` "SA -5.5": close spread −5.5 / +5.5, close moneyline −205 / +171, so the `ext=1` lines read "-5.5  -205" and "+5.5  +171". Leaders are season averages (`pointsPerGame` …), 16 statistics per side, no `notes`. SA's primary colour is `000000` (the black-primary rule, §3.10). |
| `tip` | 401909940 | NY @ CLE | 20261107 | 2026-11-08T01:12:56 | 2026-11-08T01:12:56 | +0.0 | 109,729 / 17,545 / 113,189 | Both: `STATUS_IN_PROGRESS` p1 11:53, 0-0. `situation.lastPlay` is the Jumpball (type 615, id `4019099404`), `leaders: []` on both sides, `statistics` already 16 per side (§2's `[]` did not occur on this night). Summary: 1 play (the Jumpball), ledger 7 timeouts, 0 fouls, 4 to give, `NONE` each side; `boxscore.players` 14 + 11 athletes; summary `leaders` empty. |
| `in_progress` | 401909940 | NY @ CLE | 20261107 | 2026-11-08T01:44:56 | 2026-11-08T01:44:56 | +0.0 | 119,360 / 29,165 / 222,936 | Both: p2 11:32, 26-30. `lastPlay` id …180 is type 113 Running Jump Shot "Riley Minix makes 22-foot running jump shot (Evan Mobley assists)", `scoreValue` 3; in the summary the same play is **still the placeholder** type 91 "No Shot (Default Shot)" with text "Riley Minix makes 22-foot shot (Evan Mobley assists)", coordinate (48, −1) — a corner three; the scoreboard re-typed first. 124 plays, 19 makes, run home 5 (under `RUN_SHOW`), ledger 6 timeouts / 0 fouls this quarter / 4 to give / `NONE` each side. |
| `run` | 401909941 | WSH @ OKC | 20261107 | 2026-11-08T01:42:36 | 2026-11-08T01:42:36 | +0.0 | 116,633 / 29,513 / 217,487 | Both: p2 10:06, 19-29. The §3.5 walk over 137 plays gives **home 8-0** (OKC, from 19-21 late in Q1 through the quarter break to 19-29 — the run survives the break). Newest scoring play …193 "Jared McCain makes 1-foot layup (Alex Caruso assists)" at (23, 0). No side in the bonus, 6 timeouts each. `lastPlay` is a defensive rebound. |
| `bonus` | 401909938 | DEN @ IND | 20261107 | 2026-11-08T00:24:56 | 2026-11-08T00:24:56 | +0.0 | 134,948 / 29,631 / 380,089 | Both: p4 3:01, 108-99. Header: DEN `bonusState` **`DOUBLE`** with IND `foulsToGive` 0 (IND 4 team fouls this quarter, 19 in the game); IND `NONE` with DEN `foulsToGive` 1 (3 this quarter). One side shoots. 2 timeouts each. `lastPlay` "Aaron Nesmith personal foul" (type 45). |
| `timeout` | 401909940 | NY @ CLE | 20261107 | 2026-11-08T01:21:56 | 2026-11-08T01:21:56 | +0.0 | 114,781 / 27,813 / 148,817 | Both: p1 6:58, 11-10. `lastPlay` id …67 type **16 "Full Timeout"**, text "Cavaliers Full timeout", `team.id` 5 (CLE), and the summary's newest play is the same timeout; CLE's `timeoutsRemaining` already reads 6 (`timeoutsUsed` 1), NY's 7. |
| `end_of_period` | 401909938 | DEN @ IND | 20261107 | 2026-11-07T23:59:56 | 2026-11-07T23:59:56 | +0.0 | 134,466 / 28,568 / 335,199 | Both: `STATUS_END_PERIOD` (id 22) p3 "0.0", "End of 3rd Quarter", 90-78. `lastPlay` type 412 End Period (team-less). Header: both sides `DOUBLE`, `foulsToGive` 0/0, team fouls this quarter 4 / 5 — the ledger is not reset until the next quarter's first play. 352 plays, 68 makes. |
| `halftime` | 401909939 | BKN @ ATL | 20261107 | 2026-11-08T01:43:16 | 2026-11-08T01:43:16 | +0.0 | 117,253 / 27,805 / 273,260 | Both: **`STATUS_HALFTIME` (id 23)** p2, `detail` "Halftime", 70-66, `displayClock` **"0.1"** on both (not "0.0"). `lastPlay` "End of the 2nd Quarter". Header: both sides `DOUBLE`, `foulsToGive` 0/0, 4 / 6 team fouls this quarter, 5 / 4 timeouts. BKN's primary colour is `000000`. 238 plays, 50 makes. |
| `end_of_fourth` | 401909942 | ORL @ GS | 20261107 | 2026-11-08T04:05:02.069 | 2026-11-08T04:05:03.321 | −1.3 | 138,458 / 27,887 / 457,663 | Collector. Both: `STATUS_END_PERIOD` p4 "0.0", "End of 4th Quarter", **110-110** — the tie that went to overtime. Header: both sides `DOUBLE`, `foulsToGive` 0/0, **`timeoutsRemaining` 0/0 with `timeoutsUsed` 3 / 4** — a genuine state, both teams out of timeouts and in the penalty, which the §3.3 glitch rule must not confuse with `ledger_glitch`. The summary 40 s later (04:05:45) resets the ledger for OT: 2 timeouts each, 4 to give, `NONE`. |
| `overtime` | 401909942 | ORL @ GS | 20261107 | 2026-11-08T04:07:03.944 | 2026-11-08T04:07:04.101 | −0.2 | 138,910 / 28,961 / 458,275 | Collector. Both: `STATUS_IN_PROGRESS` **period 5**, "4:58 - OT" (the first OT poll; no poll read "5:00 - OT"), 110-110, linescores with a fifth 0 / 0 entry. Header: 2 timeouts each (the OT allotment), 0 fouls this quarter, 4 to give, `NONE`. The summary's newest play is a Substitution at OT 5:00; the OT Jumpball (type 615) arrives three polls later. |
| `frozen_clock` | 401909938 | DEN @ IND | 20261107 | 2026-11-08T00:05:56 | 2026-11-08T00:05:56 | +0.0 | 134,975 / 29,659 / 346,627 | Both: `STATUS_IN_PROGRESS` p4 **"0.0"**, 97-82. The scoreboard read "0.0" on 18 consecutive polls, 00:01:36 to 00:08:56, while the score went 90-78 → 99-87 (seven changes); the header clock froze with it, and the plays carry the real clock (`lastPlay` …540 "Cameron Johnson makes 27-foot three point jumper (DeMar DeRozan assists)" is at 10:01 in the summary). The clock came back at 00:10:56 ("12:00", then "9:22"). |
| `ledger_glitch` | 401909938 | DEN @ IND | 20261107 | 2026-11-08T00:11:56 | 2026-11-08T00:11:56 | +0.0 | 134,566 / 28,853 / 346,609 | Both: p4 9:22, 99-87. The one glitch body in the set: **`timeoutsRemaining` 0/0, `timeoutsUsed` 0/0, `foulsToGive` 0/0, `bonusState` `DOUBLE`/`DOUBLE`** — while `teamFouls` 11 / 16 and `teamFoulsCurrent` 1 / 1 keep their values. The polls a minute before and after read 2 / 3 timeouts, 3 to give, `NONE`. `lastPlay` "Nuggets Full timeout". |
| `placeholder_play` | 401909940 | NY @ CLE | 20261107 | 2026-11-08T01:34:56 | 2026-11-08T01:34:56 | +0.0 | 125,510 / 28,087 / 187,629 | Both: p1 2:49, 19-16. `lastPlay` id …131 type **91 "No Shot (Default Shot)"**, text **"makes 2-foot shot"** — no player name at all — `team.id` 18, `scoreValue` 2. The summary's newest play is …129, also type 91 ("Karl-Anthony Towns misses shot", coordinate (24, 6)); …131 is not in `plays[]` yet. |
| `in_progress_no_last_play` | 401909941 | WSH @ OKC | 20261107 | 2026-11-08T01:11:36 | 2026-11-08T01:11:36 | +0.0 | 117,384 / 16,662 / 113,725 | Both: `STATUS_IN_PROGRESS` p1 11:56, 0-0. **`situation: {}`**, `leaders: []`, linescores `[0]`. Summary: 0 plays, the full tip-off ledger (7 / 0 / 4 / `NONE`), 12 + 12 athletes. |
| `in_progress_subminute` | 401909938 | DEN @ IND | 20261107 | 2026-11-08T00:31:56 | 2026-11-08T00:31:56 | +0.0 | 134,947 / 29,630 / 392,962 | Both: p4 **"39.0"**, 112-102 — the colonless "S.t" shape in the 4th (§3.2's warning colour). `lastPlay` "Christian Braun defensive rebound". Header: DEN `DOUBLE` (IND 6 fouls this quarter, 0 to give), 2 / 1 timeouts. |
| `final` | 401909938 | DEN @ IND | 20261107 | 2026-11-08T00:33:56 | 2026-11-08T00:33:56 | +0.0 | 134,306 / 28,393 / 382,053 | Scoreboard `STATUS_FINAL` p4, no `altDetail`, 114-102, `situation` absent. `leaders` on both sides: points N. Jokic 19 / P. Siakam 23, rebounds N. Jokic 13 / I. Zubac 12, assists N. Jokic 8 / A. Nembhard 8, plus `rating`. `records[0].abbreviation` reads **"Game"** ("5-3" / "3-7"). Summary `STATUS_FINAL` 114-102 with the header ledger keys **gone**, 463 plays, 85 makes, `header.gameNote` "NBA Mexico City Game". |
| `final_ot` | 401909942 | ORL @ GS | 20261107 | 2026-11-08T04:28:34.099 | 2026-11-08T04:28:37.863 | −3.8 | 137,240 / 27,793 / 485,103 | Collector. Scoreboard `STATUS_FINAL` **p5, `altDetail` "OT"**, "Final/OT", 119-126, five linescores. Leaders S. Curry 41 / F. Wagner 30. Summary `STATUS_FINAL` 119-126, 576 plays, 86 makes; its last play reads "End of the 1st  Overtime" (two spaces). Two minutes after the final the scoreboard's `displayClock` flipped from "0.0" to "5:00" (§2's post-final garbage). |
| `final_2ot` | 401873197 | SA @ OKC | 20260518 | historical, 2026-11-08 | historical, 2026-11-08 | n/a | 19,947 / 30,641 / 483,600 | Playoffs: `season.type` 3, `notes[0].headline` "West Finals - Game 1", `series.summary` "SA leads series 1-0", OKC's bare `record` "0-1". `STATUS_FINAL` **p6, `altDetail` "2OT"**, "Final/2OT", 122-115, six linescores. Leaders V. Wembanyama 41 / A. Caruso 31. Summary: 579 plays, 82 makes, `header.series` has a `season` entry ("SA wins series 4-1", the regular-season head-to-head) **and** a `playoff` entry ("SA leads series 1-0"). |
| `final_playoff` | 401859966 | SA @ NY | 20260610 | historical, 2026-11-08 | historical, 2026-11-08 | n/a | 19,745 / 30,060 / 436,189 | NBA Finals Game 4: `season.type` 3, `notes[0].headline` "NBA Finals - Game 4", `series.summary` "NY leads series 3-1", bare `record` "1-3" / "3-1". `STATUS_FINAL` p4, 106-107. Leaders V. Wembanyama 24 / J. Brunson 36. Summary: 498 plays, 72 makes, `header.gameNote` "NBA Finals - Game 4", `header.series` `season` "Series tied 1-1" beside `playoff` "NY leads series 3-1". |

Every live and final fixture has its summary half; no live fixture is
scoreboard-only.

Two facts of the plays the lane found while reading them, worth knowing
before trusting a row: `plays[]` is chronological in document order while
`sequenceNumber` is posting order and not monotone (11 of the 20 summaries
have late-posted review or foul rows at their clock with higher numbers),
and the running score can go stale for a row or two — in ORL@GS the heave
miss and the rebound after Cain's 110-110 tying putback (`end_of_fourth`,
`overtime`, `final_ot`) still carry 108-110 before End Period reads 110-110
again, which NBA.md §3.5 treats as a correction that ends the run.

## Extra fixtures (real states that bear on §3)

| Fixture | Event | Matchup | Game day | Scoreboard poll | Summary poll | Δ s | Sizes | Evidence |
|---|---|---|---|---|---|---|---|---|
| `bonus_both` | 401909940 | NY @ CLE | 20261107 | 2026-11-08T01:39:56 | 2026-11-08T01:39:56 | +0.0 | 126,554 / 28,911 / 197,841 | Both: p1 1:03, 24-24. Header: **both sides `DOUBLE`**, `foulsToGive` 0/0, 5 team fouls this quarter each — the caption draws both arrows (§3.6). `lastPlay` "Karl-Anthony Towns makes free throw 1 of 1" (type 97, `scoreValue` 1); the summary's newest scoring play is the same free throw, with the sentinel coordinate. |

Each extra is a self-contained pair. Dropping one means deleting its two
files and its row here.

## To add from the collector's first fortnight

- `double_overtime` — a live period-6 body. Not seen: the design slate and
  the collector's first night had one overtime (ORL@GS), none went to a
  second. The 2026-05-18 capture is a 2OT **final** and is `final_2ot`.
- `final_no_leaders` — a post body without the four leader categories on
  both sides. Not seen in 372 post rows (139 regular-season and 253 play-in,
  playoff and Finals events plus every post poll of this slate).
- A live playoff body (`series`, a game-number headline) and a live
  `NBA Cup - Group Play` body for the §3.8 secondary line.

## §3.7 play types in the capture set

From every play type in the 175 design summaries (five games), the five
historical finals (2,718 plays), the final summaries of the collector's
first-night games (NY@CLE, WSH@OKC, ORL@GS, HOU@PHX; SA@POR's latest) and
every scoreboard `lastPlay`. 104 distinct `type.id`s in all. The
`lastPlay` column counts scoreboard polls.

| Category | id | `type.text` | Plays | lastPlay | Text carries an outcome? | Example text |
|---|---|---|---|---|---|---|
| Timeout | 16 | Full Timeout | 770 | 83 | n/a (the team is in the text and `team.id`) | "Cavaliers Full timeout" |
| Challenge | 213 | Challenge | 33 | 0 | **No** — the first posting, re-typed in place | "Nuggets Coach's Challenge" |
| Challenge | 277 | Coach's Challenge (replaycenter) | 1 (transient) | 0 | **No** — seen only live, team-less; the same play id was 215 in the final summary | "(07:38) [Magic] COACH'S CHALLENGE (CALL REPLAYCENTER)" |
| Challenge | 214 | Coach's Challenge (Supported) | 1 | 0 | Yes | "(10:00) [Hornets] COACH'S CHALLENGE (REPLAY SUPPORTS CALL) [Hornets] charged with a timeout" |
| Challenge | 215 | Coach's Challenge (Overturned) | 50 | 0 | Yes | "(00:09.7) [Nuggets] COACH'S CHALLENGE (CALL OVERTURNED) [Nuggets] retain their timeout" |
| Challenge | 216 | Coach's Challenge (Stands) | 35 | 1 | Yes | "(10:59) [Wizards] COACH'S CHALLENGE (CALL STANDS) [Wizards] charged with a timeout" |
| Review | 278 | Ref-Initiated Review (Supported) | 44 | 0 | Yes | "(00:00.2) [Nuggets] REF-INITIATED REVIEW (REPLAY SUPPORTS CALL)" |
| Review | 279 | Ref-Initiated Review (Overturned) | 6 | 1 | Yes | "(00:41.7) [Spurs] REF-INITIATED REVIEW (CALL OVERTURNED)" |
| Review | 280 | Ref-Initiated Review (Stands) | 2 | 0 | Yes | "(04:26) [Spurs] REF-INITIATED REVIEW (CALL STANDS)" |
| Flagrant | 32 | Flagrant Foul Type 1 | 3 | 0 | n/a | "Mitchell Robinson flagrant foul type 1" |
| Flagrant free throws | 104, 105, 157, 165, 166 | Free Throw - Flagrant 1 of 2 … 3 of 3 | 7 | 0 | n/a (free throws, not the foul) | "Victor Wembanyama makes free throw flagrant 1 of 2" |
| Technical | 35 | Technical Foul | 11 | 0 | n/a | "Jamal Murray technical foul" |
| Technical | 30 | Double Technical Foul | 1 | 0 | n/a | "Double technical foul: Georges Niang and Wendell Carter Jr." |
| Technical | 29 | Defensive 3-Seconds Technical | 20 | 1 | n/a (text ends in a newline) | "Danny Wolf defensive 3-seconds (technical foul)\n" |
| Technical | 48 | Flopping Technical | 37 | 0 | n/a | "Kelly Oubre Jr. flopping technical foul" |
| Technical free throw | 103 | Free Throw - Technical | 65 | 1 | n/a | "DeMar DeRozan misses technical free throw" |
| Ejection | — | — | 0 | 0 | — | No ejection type or text occurred anywhere in the set. |
| Placeholder | 91 | No Shot (Default Shot) | 13 | 25 | — | "Jalen Johnson makes shot", "makes 2-foot shot" (no name) |
| Placeholder | 61 | No Turnover | 4 | 3 | — | "Nolan Traore turnover" |
| Placeholder | — | No Violation, No Timeout | 0 | 0 | — | Not in this set (§2 counted "No Violation" in the October scoreboards). |

Flagrant Foul Type 2 did not occur. The challenge and review plays with an
outcome are the ones whose `type.text` names it in parentheses and whose
text reads "(CALL OVERTURNED)", "(CALL STANDS)" or "(REPLAY SUPPORTS CALL)";
213 and 277 are the in-progress postings and keep their play id when ESPN
re-types them, so the §3.7 change detection sees the outcome as a new play
once.

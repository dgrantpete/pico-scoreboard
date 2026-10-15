# NHL fixture corpus — provenance

Phase 6.0 of `firmware-rs/NHL.md`. Every live and final fixture is a
time-consistent pair:

- `<name>.json` — the event object from one scoreboard poll, written as
  `json.dumps(event, indent=2)` of the parsed scoreboard body (key order as
  served; the same convention as every other sport's corpus).
- `<name>.summary.json` — the summary body for the same event at the same
  moment. Collector captures are the **raw bytes as served** (decompressed
  `bodies.body`, unmodified). Historical captures were saved pretty-printed,
  so their summaries are re-serialized with
  `json.dumps(obj, ensure_ascii=False, separators=(",", ":"))`; that form is
  byte-identical to the raw body for collector captures (checked on two),
  and each historical re-serialization has exactly the raw byte count
  recorded at capture time (`sizes_raw_bytes.tsv`).

Pregame has no summary (D3). Harness note: `<name>.summary.json` ends in
`.json`, so `wire_corpus.rs::fixture_names` will list it as a fixture named
`<name>.summary`; the hockey arm has to skip those stems.

## Selection method (to refresh the corpus)

1. Mirror the collector's NHL metadata (`responses` where `league = 'nhl'`:
   scoreboard rows for the game days wanted, every summary row) and the
   distinct scoreboard bodies (~355 MB compressed for 2026-10-01..14).
   Summary bodies are fetched selectively; never in bulk.
2. Index every event of every scoreboard poll: `status.type`, period,
   `displayClock`, both scores, both `statistics` lengths.
3. Classify a 1-in-20 sample of every event's summary polls with §3.4
   (`onIce` entries against the `boxscore.players[].statistics[name=goalies]`
   ids), find candidate windows for each state, then rescan the window
   poll by poll.
4. Pair the chosen summary poll with the same event's nearest scoreboard
   poll. Live pairs must agree on status name, period, `displayClock` and
   both scores. A final summary's header status carries only `type` (no
   period or clock), so final pairs agree on status name and both scores.
5. Prefer the most recent game day, the clearest evidence, and the smallest
   |Δ|. Every live state below exists in the collector data, so nothing is
   synthetic.

Δ is scoreboard `requested_at` minus summary `requested_at`. Times are UTC;
"game day" is ESPN's `dates=` (US/Eastern) day. Sizes are bytes: the whole
scoreboard body as served / this event's fixture / the summary.

## Required states

| Fixture | Event | Matchup | Game day | Scoreboard poll | Summary poll | Δ s | Sizes | Evidence |
|---|---|---|---|---|---|---|---|---|
| `pregame` | 401892505 | DAL @ COL | 20261014 | 2026-10-15T00:00:02.230 | — | — | 36,675 / 41,362 / — | `STATUS_SCHEDULED` (id 1), period 0. Probables: DAL C. DeSmith *Confirmed*, COL S. Wedgewood *Expected*. `records[0]` type `total`, "2-3-1" / "4-2-0". Statistics: 7 season values incl. `plusMinus`. Both teams carry `team.alternateColor` (`000000`, `005ea3`), which ESPN began sending at 2026-10-14 23:44 UTC. |
| `puck_drop` | 401892494 | STL @ OTT | 20261013 | 2026-10-13T23:39:22.783 | 2026-10-13T23:39:19.236 | +3.5 | 281,856 / 16,871 / 118,986 | Both: `STATUS_IN_PROGRESS` p1 19:26, 0-0. Scoreboard `statistics: []` for both teams, so SOG is absent (§3.3). Summary: 2 plays (Period Start, Face Off), `onIce` 6 entries per side, and the `boxscore` goalies groups are **empty** (0 athletes per team; every player group is empty), so §3.4 cannot identify a goalie. Read literally, the rule gives both sides "pulled". |
| `in_progress` | 401892503 | BOS @ SJ | 20261013 | 2026-10-14T04:31:34.397 | 2026-10-14T04:31:34.484 | −0.1 | 441,109 / 43,436 / 313,649 | Both: p2 9:32, 3-2. Both teams have 6 statistics. SOG 10-19 from saves + goals equals the boxscore `shotsTotal` 10-19. `onIce` has 6 entries per side, each with its goalie: 5v5, `Even`. 5 goals. The last play is Blocked 2nd 10:03 (Even Strength). |
| `end_of_period` | 401892488 | NJ @ DET | 20261013 | 2026-10-13T23:40:23.432 | 2026-10-13T23:40:26.076 | −2.6 | 282,989 / 49,877 / 381,508 | Both: `STATUS_END_PERIOD` (id 22) p2 0:00 "End of 2nd Period", 4-3. `onIce` 6 + G each side. SOG 30-14 equals the boxscore. The last play is Period End 2nd 20:00. |
| `power_play` | 401892472 | UTA @ BUF | 20261010 | 2026-10-11T00:34:11.027 | 2026-10-11T00:34:13.695 | −2.7 | 264,790 / 32,656 / 325,649 | Both: p2 5:23, 1-1. `onIce` UTA has 6 entries with its goalie (5 skaters), BUF 5 with its goalie (4 skaters): `PowerPlay(away, 5, 4)`. The penalty is Owen Power (BUF), Delaying Game – Puck over Glass (type 109, Minor, 2) at 14:20. The last play is a UTA Shot at 14:36, strength "Power Play". SOG from saves + goals is 17-10; the boxscore says 16-10. |
| `empty_net` | 401892496 | PIT @ CHI | 20261013 | 2026-10-14T02:36:36.880 | 2026-10-14T02:36:33.935 | +2.9 | 357,403 / 39,226 / 425,800 | Both: p3 1:04, 3-4. `onIce` PIT has 6 entries and none is a PIT goalie; CHI has 6 with its goalie: `EmptyNet(away)`, 6 v 5. The last plays read "Even Strength" during the 6v5 (§2). SOG 25-19 equals the boxscore. |
| `four_on_four` | 401892498 | COL @ DAL | 20261013 | 2026-10-14T01:16:24.297 | 2026-10-14T01:16:23.584 | +0.7 | 283,093 / 23,872 / 213,761 | Both: p1 5:10, 1-0. Coincidental Roughing minors, Blackwell (DAL) and Malinski (COL), at 13:07 1st. `onIce` 5 entries each with the goalie: `Matched(4)` in regulation. SOG 7-7 equals the boxscore. |
| `overtime` | 401892464 | TOR @ VGK | 20261008 | 2026-10-09T04:51:12.783 | 2026-10-09T04:51:09.035 | +3.7 | 287,899 / 47,837 / 485,122 | Both: `STATUS_IN_PROGRESS` p4 0:39 ("0:39 - OT"; ESPN has no OT status name), 3-3. `onIce` 4 entries each with the goalie: 3-on-3, `Matched(3)`. Includes a 2nd-period goal with strength 902 "Penalty Shot". SOG from saves + goals is 23-41; the boxscore says 22-41. |
| `shootout` | 401892474 | NSH @ OTT | 20261010 | 2026-10-11T02:01:40.041 | 2026-10-11T02:01:37.892 | +2.1 | 291,588 / 40,555 / 481,613 | Both: `STATUS_IN_PROGRESS` p5 0:00 "In SO", 2-2 (the tied game score). The SO attempts so far are OTT Shot (saved), NSH Goal, OTT Goal, NSH Shot (saved): 2 each, tally 1-1, OTT to shoot next. No `Shootout End` play yet. `onIce` holds 1 entry per side, the goalie. Attempt …5810 (Stützle) read "Shot" in the 02:01:07 poll and "Goal" from 02:01:16 on, under the same play id. Pinto's later attempt (…5830) did the same. |
| `onice_glitch` | 401892454 | VGK @ SEA | 20261006 | 2026-10-07T04:11:34.874 | 2026-10-07T04:11:35.170 | −0.3 | 240,898 / 45,379 / 416,175 | Both: p3 10:39, 5-1. Fight stoppage: Montour (SEA) and Stone (VGK) Fighting majors at 9:21 3rd, which are the last two plays. `onIce` SEA has 3 entries with its goalie (2 skaters); VGK has 6 with its goalie. That is not sane under §3.4. The latest non-penalty play is a **team-less** Stoppage tagged "Power Play"; the one before it is a VGK Face Off, "Power Play". |
| `final` | 401892491 | WSH @ CAR | 20261013 | 2026-10-14T02:12:10.659 | 2026-10-14T02:12:11.166 | −0.5 | 321,313 / 59,213 / 502,733 | Scoreboard `STATUS_FINAL` p3, no `altDetail`, 2-4. Summary `STATUS_FINAL`, 2-4, with header `featuredAthletes` winningGoalie, losingGoalie, first/second/thirdStar (one of only 2 collector finals whose summary has all five). SOG 22-35 equals the boxscore. 6 goals, the last one ENG. |
| `final_ot` | 401802535 | COL @ SJ | 20251101 | historical, 2026-10-01 | historical, 2026-10-01 | n/a | n/r / 39,458 / 436,806 | `STATUS_FINAL` p4, `altDetail` "OT", "Final/OT", 2-3. The winner is in OT at 1:48. Summary `featuredAthletes` has all five. SOG 38-23 equals the boxscore. 2025-26 season shape. |
| `final_so` | 401802536 | DAL @ FLA | 20251101 | historical, 2026-10-01 | historical, 2026-10-01 | n/a | n/r / 41,807 / 422,582 | `STATUS_FINAL` p5, `altDetail` "SO", 3-4: FLA's score includes the +1 and its period-5 linescore is 1/0. The SO plays end with `Shootout End`. Summary `featuredAthletes` has all five. SOG 22-28 equals the boxscore. 2025-26 season shape. |
| `final_2ot` | 401874173 | CAR @ VGK | 20260606 | historical, 2026-10-01 | historical, 2026-10-01 | n/a | n/r / 44,236 / 629,298 | Playoffs: `season.type` 3, Stanley Cup Final Game 3, `series.summary` "VGK leads series 2-1". `STATUS_FINAL` p5, `altDetail` "2OT", 4-5. The winning goal has `period.number` **5** and displayValue "2nd OT", so the period number alone does not mean SO. Summary `featuredAthletes` has all five. |
| `playoff_clincher` | 401874176 | CAR @ VGK | 20260614 | historical, 2026-10-01 | historical, 2026-10-01 | n/a | n/r / 36,478 / 489,549 | Playoffs: Stanley Cup Final Game 6, `series.summary` "CAR wins series 4-2". `STATUS_FINAL` p3, 3-0. Summary `featuredAthletes` has all five. The last goal is ENG. |

`final_ot` and `final_so` come from the historical captures because no
collector OT or SO final has stars in its summary. Of the 94 games whose
last collector summary is a final, only 4 carry header `featuredAthletes`,
and all 4 are regulation games. The collector retires an event two polls
after it leaves the live set, roughly 20 s after the final, and the stars
reach the summary later than that. `final_so_no_stars` below is the
collector's current-season SO final.

## Extra fixtures (real quirks that bear on §3)

| Fixture | Event | Matchup | Game day | Scoreboard poll | Summary poll | Δ s | Sizes | Evidence |
|---|---|---|---|---|---|---|---|---|
| `delayed_start` | 401891782 | STL @ DAL | 20261002 | 2026-10-03T01:12:28.347 | 2026-10-03T01:12:27.550 | +0.8 | 75,952 / 14,552 / 84,380 | Both: `STATUS_DELAYED` (**id 7, state `in`**), **period 0**, 0:00, 0-0. Scoreboard `statistics: []` and `situation: {}`. Summary: 0 plays, `onIce: []`, goalies groups empty. Later polls of this game stay `STATUS_DELAYED` after puck drop, through p1 18:55. |
| `penalty_placeholder` | 401892498 | COL @ DAL | 20261013 | 2026-10-14T01:12:21.291 | 2026-10-14T01:12:18.034 | +3.3 | 282,515 / 24,499 / 200,956 | Both: p1 6:53, 1-0. The newest play is penalty …3380 as ESPN first posts every penalty: type **143 "Penalty Shot Infraction"**, penaltyType Minor, 2 min, text "Colin Blackwell Minor". About 20 s later the same id is rewritten to type 45 Roughing, "Colin Blackwell Roughing against Sam Malinski", which is how `four_on_four` shows it. |
| `shootout_teamless_attempt` | 401892492 | SEA @ PHI | 20261013 | 2026-10-14T02:01:45.020 | 2026-10-14T02:01:44.889 | +0.1 | 302,826 / 42,032 / 465,301 | Both: p5 "In SO", 2-2. SO plays: PHI Shot, SEA Goal, PHI Goal, **Shot with no `team`** (no shooter participant, only `saver` D. Vladar of PHI, text " Snap Shot saved by Dan Vladar"), PHI Shot. Five attempts, so SEA shoots next. Counting only plays with a team gives PHI 3 and SEA 1. This game's final summary keeps one team-less attempt. |
| `shootout_failed_attempt` | 401892490 | BUF @ MTL | 20261013 | 2026-10-14T01:27:21.972 | 2026-10-14T01:27:20.026 | +1.9 | 292,797 / 44,248 / 445,518 | Both: p5 "In SO", 3-3. SO plays: MTL Goal, BUF Goal, MTL Goal, BUF **type 2010 "failed shot attempt"** (participants without a `type` role), MTL Shot (…8060, Bolduc). About 20 s later …8060 becomes a Goal under the same id and the shootout ends. |
| `final_so_no_stars` | 401892474 | NSH @ OTT | 20261010 | 2026-10-11T02:03:30.326 | 2026-10-11T02:03:29.577 | +0.7 | 286,964 / 28,861 / 477,117 | The same game as `shootout`, after the final. Scoreboard `STATUS_FINAL` p5, `altDetail` "SO", 2-3, with SO linescore 1/0. Summary `STATUS_FINAL` 2-3 with **no** `featuredAthletes`, which is the usual just-final shape. `onIce: []`. SOG 18-20 equals the boxscore. |

Each extra is a self-contained pair. Dropping one means deleting its two
files and its row here.

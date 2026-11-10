# NCAA men's basketball fixture corpus — provenance

Phase B0 of `firmware-rs/NCAAB.md`. The NBA corpus's convention
(`backend/testdata/basketball/nba/PROVENANCE.md`): every live and final
fixture is a time-consistent pair —

- `<name>.json` — the event object from one scoreboard poll, written as
  `json.dumps(event, indent=2)` of the parsed scoreboard body (key order as
  served);
- `<name>.summary.json` — the same event's summary at the same moment, the
  **raw bytes as served** (the collector stores bodies unmodified).

Pregames have no summary. Live pairs agree on status name, period,
`displayClock` and both scores within 120 s; finals agree on status name and
both scores (a final summary's header carries no clock).

The goldens under `backend/testdata/wire/basketball/mens-college-basketball/`
follow the NBA's naming: `<name>.bin` the v2 bytes, `<name>.ext1.bin` a
pregame's team lines (the rank line), `<name>.ext2.bin` the §3 blocks, and
`<name>.no_summary.ext2.bin` the payload a live fixture's handler serves when
the summary fetch fails. Nothing in them is frozen: no deployed decoder has
polled the college route.

## Sources

1. The home-lab collector (`tools/espn`, the NUC's Postgres), target `ncaab`
   (ESPN's default scoreboard: games with a ranked team), which began keeping
   college summaries at 2026-11-09 21:13 UTC (session 21). Its first night,
   2026-11-10 00:00–02:10 UTC, held five ranked games from tip to final:
   UConn–Wagner, Texas–Grambling, Tennessee–Radford, Miami–North Florida and
   Alabama–Oakland (3,613 summary polls, 2,155 distinct bodies).
2. Historical fetches (2026-11-10, `data/ncaab-design/espn-captures-historical/`,
   gitignored): the scoreboards of 2026-03-14, 03-20, 04-04 and 04-06 and three
   complete summaries.

## The pairs

| Fixture | Event | Moment | Source |
|---|---|---|---|
| `tip` | 401906759 Oakland at Alabama | 1st 19:58, 0–0 | collector, 00:31:36Z |
| `in_progress` | 401906865 Grambling at Texas | 2nd 11:18, 39–77 (lastPlay a TV timeout) | collector, 01:32:21Z |
| `run` | 401921720 North Florida at Miami | 2nd 6:32, 43–78, Miami on an 8–0 run | collector, 01:41:01Z |
| `halftime` | 401906849 Radford at Tennessee | HALFTIME (period 1), 24–33 | collector, 01:05:22Z |
| `end_of_regulation` | 401906865 Grambling at Texas | END_PERIOD (period 2), 62–93 | collector, 02:09:29Z |
| `team_timeout` | 401906849 Radford at Tennessee | 1st 7:58, lastPlay 579 "Radford Timeout" | collector, 00:32:47Z |
| `tv_timeout` | 401906849 Radford at Tennessee | 2nd 15:29, lastPlay 580 Official TV Timeout | collector, 01:17:53Z |
| `placeholder_play` | 401906865 Grambling at Texas | 2nd 19:14, lastPlay 91 "Shot", text "team  makes 15-foot shot" (no player yet) | collector, 01:13:05Z |
| `final` | 401906849 Radford at Tennessee | FINAL, 47–70 | collector, 02:06:55Z |
| `final_ot` | 401851570 Arkansas vs Ole Miss | FINAL/OT, 90–93, SEC Tournament semifinal | historical, 2026-03-14 |
| `final_tournament` | 401856600 UConn vs Michigan | FINAL, 63–69, National Championship (`season.type` 3) | historical, 2026-04-06 |
| `pregame_ranked` | 401909739 Cal Poly at No. 5 Arizona | SCHEDULED, a ranked side, both teams coloured | collector scoreboard, 2026-11-05 23:14Z |
| `pregame_colourless` | 401929171 WCBC at Boise State | SCHEDULED, WCBC has no `color` or `alternateColor` (NCAAB.md D13) | collector scoreboard, 2026-11-04 23:50Z |

## Not yet in the corpus

- **A technical foul as the live `lastPlay`** (type 521): none since the
  collector began keeping college summaries. The flash table's 521 entry is
  covered by a named extractor test until one is caught.
- A live tournament or neutral-site postseason body; a live overtime body;
  a DELAYED or POSTPONED game.

## Method (to refresh the corpus)

`python data/ncaab-design/scripts/build_corpus.py OUT_DIR` (gitignored,
local): indexes every event of every college scoreboard body of the last
eight days and every college summary poll from session 21, pairs them by the
rules above, takes the middle full pair per state, and writes the two
historical finals after checking that their scoreboard and summary agree.
`--index-only` prints the pair counts without writing.

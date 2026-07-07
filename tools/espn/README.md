# ESPN scoreboard sample tooling

Collects raw ESPN scoreboard API responses during live games and turns them
into validated, discriminated-union data models — the empirical basis for the
backend's Rust types (`backend/src/mlb.rs` et al.) and the firmware's display
layouts.

Everything reads/writes one SQLite store at `data/espn/espn.db` (gitignored).
Bodies are content-addressed by sha256 and zlib-compressed, so ~1 GB of raw
responses stores in under 100 MB. Generated artifacts land in
`data/espn/generated/` (also gitignored — they are pure functions of the DB).

```
pip install -r tools/espn/requirements.txt   # from the repo root
python -m tools.espn <subcommand> --help
```

## Subcommands

| Command | Purpose |
|---|---|
| `collect --league mlb --league world-cup --duration 5h --until-all-post` | Poll live scoreboards. Adaptive per-league cadence from `Cache-Control: max-age`; per-league error backoff; a league stops early once all its events have been final for two consecutive polls. |
| `status` | Per-league row/distinct/change counts, date coverage, store size. |
| `schema --league mlb` | Genson-inferred JSON Schema + per-state field presence (streamed over distinct bodies). |
| `spec --league mlb` / `spec --combine mlb nba` | Discriminated-union OpenAPI 3.1 spec per league, and the deduped multi-league combined spec. |
| `validate --league mlb [--spec PATH] [--root NAME]` | Validate stored responses against a spec (Draft 2020-12), with per-state results and `oneOf` drill-down. |
| `migrate` | One-shot import of the retired `espn_data_samples/` DBs (already done; kept for reference). |

`--league` accepts a registry key from `leagues.py` (`mlb`, `nba`, `wnba`,
`world-cup`, `mls`, `epl`, ...) or a raw `sport/slug` pair like
`soccer/fifa.world` — new leagues need no code change to collect.

## Methodology

The goal for each league is a complete, validated model of the scoreboard
payload across all three game states.

1. **Collect during live games** — non-negotiable for capturing `in`-state
   fields. Start ~30 min before first pitch/kick/tip (captures `pre` → `in`)
   and run past the final (`in` → `post`); `--until-all-post` handles the tail.
   A supplemental run against a future date captures pure-`pre` payloads.
2. **Infer** (`schema`) — genson over every distinct 200 body, plus a thorough
   path scan (all array elements, not just `[0]`) counting each path's
   presence per state. The discriminant is always
   `event.competitions[0].status.type.state` ∈ `pre` / `in` / `post`.
3. **Model** (`spec`) — fields present in ≥1% of a state's competitions belong
   to that state's variant; fields present in 100% are required. Output is an
   OpenAPI 3.1 spec with `CompetitionBase` + `PreGame`/`Live`/`Final` variants,
   mirroring the backend's Rust `enum` over the same discriminant.
4. **Validate** (`validate`) — every stored response must pass its league's
   spec; failures are reported per state with `oneOf` branch drill-down.

Expected per-state pattern (from MLB): `pre` carries odds/tickets, `in`
carries `situation` (count, runners, batter/pitcher), `post` carries
headlines/highlights, and linescores/leaders/winner appear at first pitch and
persist ("started" fields → `Option<>` in Rust).

Known API glitch worth modeling: ESPN occasionally serves a 200 scoreboard
whose `events` are all empty objects `{}` (observed live, MLB, 2026-07-06).
Regenerate `schema`/`spec` after each collection run so such variants are
reflected, and treat event fields as fallible at the consumer boundary.

## Endpoint choice (why only the site scoreboard)

A 2026-04 experiment polled nine ESPN surfaces every ~4 s during a live MLB
game and compared effective update latency (EUL) and field coverage:

- `site.api.espn.com/.../scoreboard` — **freshest complete surface**: ~10 s
  EUL, all 8 tracked live fields, `max-age=1`, ~280 KB. This is the only
  endpoint the collector polls.
- `site .../summary` and the header ticker — same coverage, slightly worse EUL
  (~12 s), summary is 2.5× the bytes.
- `sports.core.api` fragments (`situation`, `status`, `competitors`, `plays`)
  — partial coverage each, and *slower* (situation ~20 s EUL; status and
  competitors never changed during the window).
- `cdn.espn.com/core/...` scoreboard — complete but `max-age=300`; static
  during the whole test.

Practical polling numbers: the scoreboard's `max-age` oscillates 1–7 s during
live games; the collector polls at `max_age + 1` (floor 5 s) and one evening
slate yields roughly 1,500–2,500 distinct bodies per league.

## Tray app (temporary, for continuous collection)

`python -m tools.espn tray` runs a Windows system-tray collector: all
registry leagues, 60s idle cadence that automatically tightens to ESPN's
cache cadence (~5-10s) whenever a league has a live game, writing rows with
`source='tray'`. The game day rolls at 5am ET so post-midnight games keep
their stream.

- **Icon**: green = collecting, yellow = paused, red = crashed (with a
  notification; diagnostics in `data/espn/tray.log`).
- **Menu**: per-league checkboxes with live status, idle-interval picker
  (30s/60s/2min/5min), Pause, and Stop (fully quits and frees the tray).
- **Startup**: `python -m tools.espn tray --install-startup` registers it in
  HKCU Run (via `tray_launcher.pyw` + `pythonw.exe`, no console window);
  `--uninstall-startup` removes it. Settings persist in
  `data/espn/tray_config.json`.
- **Full removal when done collecting**: Stop from the menu →
  `--uninstall-startup` → `pip uninstall pystray` → delete
  `data/espn/tray_config.json` and `tray.log`.

## Store provenance

`source` column on `responses`: `live` rows come from this collector;
`legacy:espn_collection` / `legacy:mlb_collection` (MLB, April 2026) and
`legacy:nba_collection` (NBA, April 2026) were imported from the retired
`espn_data_samples/` directory by `migrate`, verified row-for-row (MD5 +
byte-exact spot checks) before the originals were archived and deleted.

## Promoting a spec to the backend

The tracked reference spec is `backend/espn_openapi_combined.yaml`. Promotion
is deliberately manual (the backend is refactored independently):

```
python -m tools.espn spec --combine mlb nba ...
cp data/espn/generated/espn_openapi_combined.yaml backend/espn_openapi_combined.yaml
```

Review the diff before committing — a new required field means live data
changed shape, not just that the model got better.

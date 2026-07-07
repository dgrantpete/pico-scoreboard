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
| `ui` | Local read-only viewer at `http://127.0.0.1:3776` — store dashboard, presence explorer, discover reports, artifact index. |
| `discover --league mlb` | Automatically rank discriminated-union tag candidates by information gain; MDL-scored split hierarchies. Repeat `--league` to pool sports. |
| `schema --league mlb` | Genson-inferred JSON Schema + per-state field presence (streamed over distinct bodies). |
| `spec --league mlb` / `spec --combine mlb nba` | Discriminated-union OpenAPI 3.1 spec per league, and the deduped multi-league combined spec. |
| `validate --league mlb [--spec PATH] [--root NAME]` | Validate stored responses against a spec (Draft 2020-12), with per-state results and `oneOf` drill-down. |
| `tray` | Windows system-tray collector for continuous multi-week collection (see "Tray app" below). |
| `migrate` | One-shot import of the retired `espn_data_samples/` DBs (already done; kept for reference). |

`--league` accepts a registry key from `leagues.py` (`mlb`, `nba`, `wnba`,
`world-cup`, `mls`, `epl`, ...) or a raw `sport/slug` pair like
`soccer/fifa.world` — new leagues need no code change to collect.

## Playbook: from raw requests to a curated, Rust-ready spec

The whole pipeline for one sport, in order. MLB and NBA have been through
every step; use them as references.

### 1. Collect

Two modes, complementary:

- **Always-on**: the tray app polls every registry league whenever the PC is
  on (60s idle, ESPN-adaptive ~5–10s during live games). This is how a new
  sport accumulates coverage passively.
- **Targeted**: for a specific slate, `python -m tools.espn collect --league
  wnba --until-all-post --duration 6h` polls at full adaptive speed and stops
  itself when every game is final.

**How much data is enough?** Before modeling a sport, you want:

- all three states observed — `in` requires polling *during* live games
  (non-negotiable); `pre` requires polling before games start or a
  supplemental run against a future date;
- several distinct game days (one evening = one weather pattern);
- roughly ≥500 distinct bodies (`status` column `distinct`);
- a seasonality sanity check: April-playoff NBA is not regular-season NBA —
  rare payloads (shootouts, doubleheaders, All-Star events) only appear when
  they happen.

### 2. Inspect

`python -m tools.espn status` (or the `ui` dashboard). Columns: `polls` =
requests stored; `distinct` = unique bodies (live games churn, idle leagues
dedup to almost nothing); `changes` = body-to-body transitions within each
(league, date) stream; `non-200` should stay ~0. Confirm the league has rows
across multiple dates and a healthy distinct count before proceeding.

### 3. Discover

```
python -m tools.espn discover --league wnba
```

Reads the report bottom to top of the ranking:

- **gain** (bits) says how much knowing the tag's value predicts the presence
  of other fields; **explains** counts fields that become deterministic
  (≤1% or ≥99% inside every class). **High gain with ~0 explains is identity
  leakage** — on a small corpus, game id/date/venue "predict" everything.
  Such tags are barred from anchoring trees and disappear from candidacy once
  more data pushes them past `--max-cardinality`.
- **aliases** are tags inducing the identical instance partition — one split,
  several encodings (`state`/`name`/`id`/`description`; on pooled runs the
  algorithm noticed `format.regulation.periods` — 9 innings / 4 quarters /
  2 halves — is a sport identifier).
- **missing instances** on a candidate is the glitch bucket: payloads lacking
  the tag (e.g. all-empty `{}` events) are excluded from variants and
  reported, tolerance set by `--tag-presence` (default 99%).
- **split hierarchies** are MDL-scored: `net = data − model`, where data =
  total weighted gain × instances and model charges each extra variant one
  presence-bitmap over the field universe (what a variant literally costs to
  state in the spec). Marginal subtrees prune themselves; finer state
  machines win exactly when the corpus is big enough to justify them.

Useful variations:

- `--max-cardinality 12` when hunting finer tags (fine-grained status,
  play types); `--depth 3` for deeper hierarchies.
- Pool sports with repeated `--league` flags: a synthetic `_league` tag then
  competes as a discriminant, making "sport-first vs state-first" a measured
  comparison. (On mlb+nba+world-cup, every winning tree roots on game status
  with `_league` nested inside — sport is a refinement within states, which
  is why one shared state-keyed contract with per-sport extensions is the
  right backend shape.) Pooled scores are instance-weighted, so
  heavily-polled leagues dominate.
- `--instances 'events[].competitions[0].competitors[]'` re-roots the
  analysis at an array element — the only way to find per-element unions
  (competition-level flattening destroys the pairing between an element's
  tag and that element's fields). Expect honest negatives: competitors come
  back "plain struct with optional fields, not a discriminated union".

### 4. Curate (the human step)

The report proposes; you decide. The decisions that actually come up:

- **Discriminant granularity.** Soccer's 6-state machine
  (First Half/Halftime/…) and the coarse `pre/in/post` scored a near-tie at
  one evening of data. Pick fine if the display should render halftime
  differently; pick coarse if all sports should share one 3-variant contract.
  MDL tells you what the data supports; only you know what the firmware
  should *show*.
- **Rare classes**: `--min-class-pct` (default 1%) keeps glitch/rare classes
  from becoming variants. Deliberately rare-but-real states (extra time,
  shootouts) may need the threshold lowered once observed.
- **Unseen-variant risk**: a spec can only model what happened while you
  collected. Check the corpus dates against the sport's calendar before
  trusting it.

(Planned: pinning the curated choice in a per-league config that `schema`/
`spec` consume, so curation becomes a reviewed git diff. Today the pipeline
uses `status.type.state`, which discovery has confirmed optimal-or-tied for
every sport measured.)

### 5. Generate and validate

```
python -m tools.espn schema --league wnba
python -m tools.espn spec --league wnba
python -m tools.espn validate --league wnba        # must print PASS, 100%
python -m tools.espn spec --combine mlb nba wnba   # multi-league contract
```

`validate` failing usually means the spec predates newer data — regenerate
`schema` + `spec` after every meaningful collection window. (Real example:
ESPN once served a 200 whose 15 events were all `{}`; the MLB spec validated
100% only after regeneration absorbed that variant.)

### 6. Translate to Rust

Philosophy: **thin projection, not full-fidelity codegen.** The backend
deserializes ~30 fields of the ~700-path payload and serde ignores the rest —
generating 700-field structs would be waste, and off-the-shelf generators
can't express the union anyway (serde can't tag on the nested
`status.type.state`; the `TryFrom<EspnCompetitionDto>` dispatch in
`backend/src/mlb.rs` is the house pattern). Hand-write the small types; let
the presence table answer the only risky question — *is this field safe to
require?*

| Presence in the variant (presence_{league}.json) | Rust |
|---|---|
| 100% | plain field |
| 1–99.99% | `Option<T>` |
| < 1% | exclude; glitch territory |
| absent | exclude from that variant |

Worked example from real data: `situation` is 100% in MLB `in` → required in
`Live`; `situation.batter` is 81.22% → `Option<EspnPlayer>`; soccer has *no*
`situation` paths at all → a soccer module has no situation struct.

Two hard-won rules:

- **Parse events leniently.** `event.id` presence is 99.97%, not 100% — one
  observed glitch payload had every event as `{}`. Deserialize `events` as
  raw values and parse per-event, skipping failures; a strict
  `Vec<EspnEvent>` turns one glitch poll into a whole-scoreboard error.
- **Promotion is a reviewed diff.** The tracked contract is
  `backend/espn_openapi_combined.yaml`:

  ```
  python -m tools.espn spec --combine mlb nba ...
  cp data/espn/generated/espn_openapi_combined.yaml backend/espn_openapi_combined.yaml
  ```

  Review before committing — a new required field means live data changed
  shape, not just that the model got better.

## Local viewer

`python -m tools.espn ui` serves a read-only single-page viewer at
`http://127.0.0.1:3776` (`--port`, `--no-browser`): live store dashboard
(10s auto-refresh, safe alongside the tray), a presence explorer with
required/member/sub-threshold/absent color bands, discover report rendering
(ranking + MDL trees), and the generated-artifact index. It never mutates
anything — pipeline commands stay in the CLI.

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
slate yields roughly 1,500–2,500 distinct bodies per league. Note the
scoreboard carries only `situation.lastPlay`, not full play-by-play — per-play
modeling would need the summary/plays endpoints as a new collector target.

## Store provenance

`source` column on `responses`: `live` rows come from CLI collector runs;
`tray` rows from the tray app; `legacy:espn_collection` /
`legacy:mlb_collection` (MLB, April 2026) and `legacy:nba_collection`
(NBA, April 2026) were imported from the retired `espn_data_samples/`
directory by `migrate`, verified row-for-row (MD5 + byte-exact spot checks)
before the originals were archived and deleted.

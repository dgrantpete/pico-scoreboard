# ESPN scoreboard sample tooling

Collects raw ESPN scoreboard API responses during live games and turns them
into validated, discriminated-union data models — the empirical basis for the
backend's Rust types (`backend/src/{mlb,nba,...}`) and the firmware's display
layouts. The same store feeds per-game **coverage** verdicts that gate future
replay/mock bundles (BACKLOG 62).

Collection runs as a containerized service on the NUC (`compute-center`),
writing to Postgres in the same compose stack — see `infra/`. The dev PC
runs only the analysis CLIs, which read that Postgres over the LAN
(WireGuard when remote). Connection comes from `ESPN_DB_URL` or
`tools/espn/.env` (copy `.env.example` and fill in).

```
pip install -r tools/espn/requirements.txt   # from the repo root
python -m tools.espn <subcommand> --help
```

## Architecture

- **`targets.py`** — the declarative poll surface. `infra/config/targets.yml`
  defines every league as data (sport/league slugs, cadence, flags). Adding a
  sport is a reviewed diff there + `ansible-playbook deploy.yml --tags
  targets`; the running service hot-reloads within ~10 s. No code changes,
  no image rebuild. Parsing is strict: typos fail loudly and the service
  keeps the previous config.
- **`service.py`** — session accounting. One `collector_sessions` row per
  process run × config epoch (config reload = session rollover), a 60 s
  heartbeat column, and an orphan sweep that closes crashed sessions at
  `last_heartbeat`. "Was the collector running at time T" is a query.
- **`collect.py`** — the poll loop: min-heap scheduler, cadence adaptive
  from `Cache-Control: max-age` (floor 5 s) with per-target idle intervals
  when nothing is live, exponential error backoff. Targets with
  `follow_summaries: true` spawn a dynamic per-event **summary** poller for
  each live game (the backend's live soccer handler consumes ESPN summaries
  for commentary — capturing them closes that replay gap), retired two polls
  after the event leaves the live set so the post-final body lands too.
- **`db.py`** — the only SQL in the package. Schema v2: content-addressed
  zlib-compressed bodies, `responses` rows keyed by (target, endpoint,
  event_id/date_param) with a session FK for provenance.
- **`coverage.py`** — per-game capture-quality report with an explicit
  REPLAY-GRADE predicate (≥30 min pregame lead, no live gap >120 s, ≥2
  post-final polls); gaps overlapping collector downtime are labeled `down`.

## Subcommands

| Command | Purpose |
|---|---|
| `serve --targets <yml>` | Run the collector service (normally only the container runs this). |
| `status` | Per-league/endpoint poll counts, store size, and recent sessions with heartbeat age. |
| `coverage [--league X] [--date YYYYMMDD] [--json]` | Per-game replay-grade verdicts. |
| `mock [--config PATH] [--port 8787]` | Serve fake ESPN endpoints for demos/testing — see "Mock-ESPN" below. |
| `bundle --league X --date YYYYMMDD [--force]` | Export a captured day into a self-contained replayable `.espnbundle` (refuses without a replay-grade event unless `--force`). |
| `discover --league mlb` | Rank discriminated-union tag candidates by information gain; MDL-scored split hierarchies. Repeat `--league` to pool sports. |
| `schema --league mlb` | Genson-inferred JSON Schema + per-state field presence. |
| `spec --league mlb` / `spec --combine mlb nba` | DU OpenAPI 3.1 spec per league, and the combined multi-league spec. |
| `validate --league mlb [--spec PATH] [--root NAME]` | Validate stored responses against a spec (Draft 2020-12). |

`--league` on the analysis commands accepts a registry key from `leagues.py`
(`mlb`, `nba`, `world-cup`, ...) or a raw `sport/slug` pair. The *collector*
doesn't use the registry — its vocabulary is `targets.yml` alone.

## Mock-ESPN (`mockdata.py` / `mockserver.py` / `bundle.py`)

Injects fake game data at the ESPN boundary: the mock serves the two
upstream routes the backend fetches, and the REAL backend is pointed at it
via `APP_ESPN__BASE_URL` — no mock code exists in the backend, and every
transform/wire/firmware path runs for real. Driven by a hot-reloaded
`mock.yml` (copy `mock.example.yml`), two modes per league:

- **scenario** — compose a slate from `backend/testdata/` fixture events;
  dates are shifted to "today" (pre-state fixtures take `start_in`); a
  `commentary` line serves as that event's summary.
- **replay** — time-warped re-serve of a captured stream from the Postgres
  store (`source: store`) or an exported `.espnbundle` (`source: bundle`),
  with `speed`, `loop`, and `start_offset`; served dates are rewritten
  through the warp so start times stay consistent at any speed. The
  backend's own 5 s JSON TTL quantizes what devices see.

Local rig: `python -m tools.espn mock` + `APP_ESPN__BASE_URL=http://127.0.0.1:8787
cargo run` in backend/, then point a device's `api.url` at the PC (reboot
required — the firmware reads it once at init). Logos keep coming from the
real CDN (payload hrefs are absolute; `espn.logo_url` is only a prefix
guard).

Deployed rig (friend's-house demos): the Fly staging pair — public backend
`pico-scoreboard-api-staging-dgrantpete.fly.dev` (unmodified prod image,
`backend/fly.staging.toml`, deploy `python tools/build.py deploy --staging`)
pointed over flycast at the PRIVATE mock app `pico-mock-espn-dgrantpete`
(`infra/fly/`, deploy from repo root:
`fly deploy . --config infra/fly/mock-espn.fly.toml`). The staging demo
config + bundles are baked into the mock image — edit
`infra/fly/mock.staging.yml` and redeploy to change the show. The mock app
must never hold a public IP (`fly ips list` to audit).

## Deploying / operating the service

All in `infra/` (see `infra/deploy.yml` header for the exact commands):

- Full deploy (code or compose changes): `ansible-playbook deploy.yml` from
  WSL — syncs the package, rebuilds the image (git sha baked in as the
  session `version`), `docker compose up -d`.
- Targets-only change: `ansible-playbook deploy.yml --tags targets` — copies
  one file; the service hot-reloads. This is the everyday path.
- Logs: `ssh <nuc> docker logs pico-scoreboard-collector-1`. Liveness from
  anywhere: `python -m tools.espn status` (heartbeat age in the session
  footer).
- Host addressing and secrets live in gitignored `infra/inventory.yml`,
  `infra/.env`, `tools/espn/.env` (committed `.example` templates). Host
  provisioning (users/docker/hardening/tunnels) belongs to the homelab repo.

## Playbook: from raw requests to a curated, Rust-ready spec

The pipeline for one sport: **collect** (add the league to `targets.yml`;
the always-on service does the rest — check `status` for distinct-body
growth and `coverage` for capture quality) → **discover** → curate →
**schema** + **spec** → **validate** (must print PASS, 100%) → hand-write
the thin Rust projection (see the presence-table rules below). MLB, NBA, WNBA,
and FIFA World are in the tracked contract
(`backend/espn_openapi_combined.yaml`); use MLB and NBA as references.

How much data before modeling: all three states observed (`in` requires
polling during live games), several distinct game days, roughly ≥500
distinct bodies, and a seasonality sanity check (playoff payloads differ
from regular season; rare states only appear when they happen).

Field-presence → Rust rules: 100% in a variant = plain field; 1–99.99% =
`Option<T>`; <1% = exclude (glitch territory); absent = exclude from that
variant. Parse events leniently (one observed glitch poll had every event as
`{}`). Promotion is a reviewed diff: the tracked contract is
`backend/espn_openapi_combined.yaml` — copy it from `data/espn/generated/`
and review before committing.

## Endpoint choice (why scoreboard + per-live-event summary)

A 2026-04 experiment polled nine ESPN surfaces during a live MLB game:
`site.api.espn.com/.../scoreboard` was the freshest complete surface (~10 s
effective update latency, all tracked live fields, `max-age` oscillating
1–7 s live). The summary endpoint carries what the scoreboard lacks
(commentary/play detail) at similar latency but 2.5× the bytes — which is
why summaries are collected per **live** event only, on targets that opt in
with `follow_summaries` (soccer: the backend consumes them; mlb: replay
richness).

## History

The v1 store was local sqlite fed by a Windows tray collector; it is frozen
at `data/espn/espn.db` (analysis of that corpus, its import into Postgres,
and the `extract_fixtures.py` port are BACKLOG 61). `ui.py` (the local
read-only viewer) is temporarily unregistered pending its Postgres port
(BACKLOG 60).

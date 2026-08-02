"""CLI for the ESPN scoreboard sample tooling.

The collector runs as a service (`serve`, normally containerized on the NUC);
everything else is an analysis command reading the shared Postgres store.
Connection comes from ESPN_DB_URL or tools/espn/.env (see .env.example).

    python -m tools.espn serve --targets infra/config/targets.yml
    python -m tools.espn status
    python -m tools.espn coverage --league mlb
"""

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

import yaml

from .bundle import export_bundle
from .config import database_url
from .coverage import coverage_report, print_report as print_coverage
from .db import Store
from .discover import DEFAULT_INSTANCES, discover, print_report
from .leagues import League, resolve
from .mockdata import MockError
from .mockserver import run as run_mock
from .schema import build_schema
from .service import serve
from .spec import build_spec, combine_specs, prefix_for
from .validate import run_validation

REPO_ROOT = Path(__file__).resolve().parents[2]
GENERATED_DIR = REPO_ROOT / "data" / "espn" / "generated"
DEFAULT_TARGETS = REPO_ROOT / "infra" / "config" / "targets.yml"
DEFAULT_MOCK_CONFIG = Path(__file__).resolve().parent / "mock.yml"
DEFAULT_TESTDATA = REPO_ROOT / "backend" / "testdata"
DEFAULT_BUNDLES = REPO_ROOT / "data" / "espn" / "bundles"


def _store(args: argparse.Namespace) -> Store:
    return Store(args.db_url or database_url())


def cmd_serve(args: argparse.Namespace) -> int:
    return serve(Path(args.targets), args.db_url or database_url())


def cmd_status(args: argparse.Namespace) -> int:
    store = _store(args)
    try:
        rows = store.league_stats()
        if not rows:
            print("store is empty")
        else:
            print(
                f"{'sport':<12}{'league':<26}{'endpoint':<11}{'polls':>7}{'distinct':>10}"
                f"{'dates':>7}{'changes':>9}{'non-200':>9}  first .. last (UTC)"
            )
            for sport, league, endpoint, polls, distinct, dates, first, last, non_200, changed in rows:
                stamps = f"{first:%Y-%m-%d %H:%M} .. {last:%Y-%m-%d %H:%M}"
                print(
                    f"{sport:<12}{league:<26}{endpoint:<11}{polls:>7}{distinct:>10}{dates:>7}"
                    f"{changed:>9}{non_200:>9}  {stamps}"
                )
            bodies, raw, stored = store.body_totals()
            print(f"\n{bodies} unique bodies, {raw / 1e6:.1f} MB raw -> {stored / 1e6:.1f} MB stored")

        sessions = store.recent_sessions()
        if sessions:
            print("\nsessions (newest first):")
            for sid, started, beat, ended, reason, host, version, names in sessions:
                if ended is None:
                    age = (datetime.now(timezone.utc) - beat).total_seconds()
                    state = f"RUNNING, heartbeat {age:.0f}s ago"
                else:
                    state = f"{ended:%Y-%m-%d %H:%M} ({reason})"
                targets = ", ".join(names) if names else "0 targets"
                print(f"  #{sid} {started:%Y-%m-%d %H:%M} .. {state}  [{host} {version}] {targets}")
    finally:
        store.close()
    return 0


def cmd_mock(args: argparse.Namespace) -> int:
    # DSN is lazy: only a `source: store` replay entry ever needs it.
    return run_mock(
        Path(args.config),
        args.port,
        Path(args.testdata),
        lambda: args.db_url or database_url(),
    )


def cmd_bundle(args: argparse.Namespace) -> int:
    league = _resolve(args.league)
    out = Path(args.out) / f"{_slug_key(league.slug)}_{args.date}.espnbundle"
    store = _store(args)
    try:
        export_bundle(store, league.sport, league.slug, args.date, out, force=args.force)
    except MockError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    finally:
        store.close()
    return 0


def cmd_coverage(args: argparse.Namespace) -> int:
    store = _store(args)
    try:
        reports = coverage_report(store, args.league, args.date)
    finally:
        store.close()
    if args.json:
        print(json.dumps(reports, indent=2))
    else:
        print_coverage(reports)
    return 0


def cmd_discover(args: argparse.Namespace) -> int:
    leagues = [_resolve(arg) for arg in args.league]
    store = _store(args)
    try:
        report = discover(
            store,
            leagues,
            instances=args.instances,
            tag_presence=args.tag_presence,
            max_cardinality=args.max_cardinality,
            min_class_pct=args.min_class_pct,
            beam_width=args.beam,
            max_depth=args.depth,
            min_split_gain=args.min_split_gain,
        )
    finally:
        store.close()
    print_report(report, top=args.top)
    GENERATED_DIR.mkdir(parents=True, exist_ok=True)
    name = "discover_" + "_".join(_slug_key(lg.slug) for lg in leagues)
    if args.instances != DEFAULT_INSTANCES:
        name += "_" + re.sub(r"\W+", "_", args.instances).strip("_")
    out = GENERATED_DIR / f"{name}.json"
    out.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"\nwrote {out}")
    return 0


def _slug_key(slug: str) -> str:
    """Filesystem-safe form of a league slug, e.g. 'fifa.world' -> 'fifa_world'."""
    return slug.replace(".", "_")


def _schema_paths(slug: str) -> tuple[Path, Path]:
    key = _slug_key(slug)
    return GENERATED_DIR / f"schema_{key}.json", GENERATED_DIR / f"presence_{key}.json"


def _spec_path(slug: str) -> Path:
    return GENERATED_DIR / f"espn_openapi_{_slug_key(slug)}.yaml"


def _write_yaml(path: Path, doc: dict) -> None:
    with path.open("w", encoding="utf-8") as f:
        yaml.dump(doc, f, sort_keys=False, width=120, default_flow_style=False)


def _resolve(arg: str) -> League:
    try:
        return resolve(arg)
    except ValueError as exc:
        raise SystemExit(f"error: {exc}")


def cmd_schema(args: argparse.Namespace) -> int:
    league = _resolve(args.league)
    store = _store(args)
    try:
        schema, presence = build_schema(store, league)
    finally:
        store.close()
    GENERATED_DIR.mkdir(parents=True, exist_ok=True)
    schema_path, presence_path = _schema_paths(league.slug)
    schema_path.write_text(json.dumps(schema, indent=2), encoding="utf-8")
    presence_path.write_text(json.dumps(presence, indent=2), encoding="utf-8")
    print(f"wrote {schema_path}\nwrote {presence_path}")
    return 0


def cmd_spec(args: argparse.Namespace) -> int:
    if args.combine:
        specs = {}
        for arg in args.combine:
            league = _resolve(arg)
            spec_path = _spec_path(league.slug)
            if not spec_path.exists():
                print(f"error: missing {spec_path}; run `spec --league {arg}` first", file=sys.stderr)
                return 2
            specs[prefix_for(league.slug)] = yaml.safe_load(spec_path.read_text(encoding="utf-8"))
        combined = combine_specs(specs)
        GENERATED_DIR.mkdir(parents=True, exist_ok=True)
        out = GENERATED_DIR / "espn_openapi_combined.yaml"
        _write_yaml(out, combined)
        n = len(combined["components"]["schemas"])
        print(f"wrote {out} ({n} component schemas)")
        return 0

    league = _resolve(args.league)
    schema_path, presence_path = _schema_paths(league.slug)
    for path in (schema_path, presence_path):
        if not path.exists():
            print(f"error: missing {path}; run `schema --league {args.league}` first", file=sys.stderr)
            return 2
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    presence = json.loads(presence_path.read_text(encoding="utf-8"))
    doc = build_spec(schema, presence, league)
    GENERATED_DIR.mkdir(parents=True, exist_ok=True)
    out = _spec_path(league.slug)
    _write_yaml(out, doc)
    print(f"wrote {out} ({len(doc['components']['schemas'])} component schemas)")
    return 0


def cmd_validate(args: argparse.Namespace) -> int:
    league = _resolve(args.league)
    spec_path = Path(args.spec) if args.spec else _spec_path(league.slug)
    if not spec_path.exists():
        print(f"error: spec not found: {spec_path}", file=sys.stderr)
        return 2
    if args.root:
        root_name = args.root
    elif args.spec:
        # combined/external specs prefix components per league; per-league specs don't
        root_name = f"{prefix_for(league.slug)}ScoreboardResponse"
    else:
        root_name = "ScoreboardResponse"
    store = _store(args)
    try:
        ok = run_validation(store, league, spec_path, root_name)
    finally:
        store.close()
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument(
        "--db-url",
        default=None,
        help="Postgres DSN (default: ESPN_DB_URL env var, else tools/espn/.env)",
    )

    parser = argparse.ArgumentParser(
        prog="python -m tools.espn",
        description="Collect and analyze raw ESPN scoreboard API responses.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("serve", parents=[common], help="run the collector service")
    p.add_argument(
        "--targets",
        default=str(DEFAULT_TARGETS),
        help=f"declarative poll-targets YAML, hot-reloaded (default: {DEFAULT_TARGETS})",
    )
    p.set_defaults(func=cmd_serve)

    p = sub.add_parser("status", parents=[common], help="summarize the store + sessions")
    p.set_defaults(func=cmd_status)

    p = sub.add_parser(
        "mock", parents=[common], help="serve fake ESPN endpoints (scenario/replay via mock.yml)"
    )
    p.add_argument(
        "--config",
        default=str(DEFAULT_MOCK_CONFIG),
        help=f"mock config YAML, hot-reloaded (default: {DEFAULT_MOCK_CONFIG})",
    )
    p.add_argument("--port", type=int, default=8787, help="listen port (default 8787)")
    p.add_argument(
        "--testdata",
        default=str(DEFAULT_TESTDATA),
        help=f"fixture root for scenario mode (default: {DEFAULT_TESTDATA})",
    )
    p.set_defaults(func=cmd_mock)

    p = sub.add_parser(
        "bundle", parents=[common], help="export a captured day into a replayable .espnbundle"
    )
    p.add_argument("--league", required=True, help="registry key or raw sport/slug")
    p.add_argument("--date", required=True, help="YYYYMMDD game day to export")
    p.add_argument(
        "--out",
        default=str(DEFAULT_BUNDLES),
        help=f"output directory (default: {DEFAULT_BUNDLES})",
    )
    p.add_argument(
        "--force", action="store_true", help="export even with no replay-grade event"
    )
    p.set_defaults(func=cmd_bundle)

    p = sub.add_parser(
        "coverage", parents=[common], help="per-game capture quality and replay-grade verdicts"
    )
    p.add_argument("--league", help="ESPN league slug (e.g. mlb, fifa.world); default: all")
    p.add_argument("--date", help="restrict to one YYYYMMDD game day")
    p.add_argument("--json", action="store_true", help="machine-readable output")
    p.set_defaults(func=cmd_coverage)

    p = sub.add_parser(
        "discover",
        parents=[common],
        help="automatically rank discriminated-union tag candidates by information gain",
    )
    p.add_argument(
        "--league",
        action="append",
        required=True,
        help="registry key or raw sport/slug; repeat to pool leagues into one corpus"
        " (a synthetic _league tag then competes as a discriminant, comparing"
        " sport-first vs state-first hierarchies)",
    )
    p.add_argument(
        "--instances",
        default=DEFAULT_INSTANCES,
        help="path selecting the objects to analyze, e.g. 'events[]' or"
        " 'events[].competitions[0].competitors[]' — re-root at an array element"
        f" to find per-element unions (default: {DEFAULT_INSTANCES})",
    )
    p.add_argument(
        "--tag-presence",
        type=float,
        default=0.99,
        help="min fraction of instances a candidate tag must appear in (default 0.99;"
        " tolerates glitch payloads like empty events)",
    )
    p.add_argument(
        "--max-cardinality",
        type=int,
        default=6,
        help="max distinct values for a candidate tag (default 6)",
    )
    p.add_argument(
        "--min-class-pct",
        type=float,
        default=1.0,
        help="classes below this %% of instances get no variant (default 1.0)",
    )
    p.add_argument("--top", type=int, default=10, help="how many ranked candidates to print")
    p.add_argument(
        "--beam",
        type=int,
        default=3,
        help="explore this many distinct-partition candidates per level (default 3)",
    )
    p.add_argument(
        "--depth", type=int, default=2, help="max split-hierarchy depth (default 2)"
    )
    p.add_argument(
        "--min-split-gain",
        type=float,
        default=1.0,
        help="a nested split must gain at least this many bits to be kept (default 1.0)",
    )
    p.set_defaults(func=cmd_discover)

    p = sub.add_parser(
        "schema", parents=[common], help="infer JSON Schema + field presence for a league"
    )
    p.add_argument("--league", required=True, help="registry key or raw sport/slug")
    p.set_defaults(func=cmd_schema)

    p = sub.add_parser(
        "spec", parents=[common], help="build a DU-aware OpenAPI 3.1 spec from generated schema data"
    )
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--league", help="registry key or raw sport/slug")
    g.add_argument(
        "--combine",
        nargs="+",
        metavar="LEAGUE",
        help="merge several leagues' generated specs into espn_openapi_combined.yaml",
    )
    p.set_defaults(func=cmd_spec)

    p = sub.add_parser(
        "validate", parents=[common], help="validate stored bodies against an OpenAPI spec"
    )
    p.add_argument("--league", required=True, help="registry key or raw sport/slug")
    p.add_argument("--spec", help="OpenAPI YAML (default: generated per-league spec)")
    p.add_argument("--root", help="root component schema (default: <Prefix>ScoreboardResponse)")
    p.set_defaults(func=cmd_validate)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())

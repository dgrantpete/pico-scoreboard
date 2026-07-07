"""CLI for the ESPN scoreboard sample tooling.

Run from the repo root:
    python -m tools.espn collect --league world-cup --league mlb --duration 5h --until-all-post
    python -m tools.espn status
"""

import argparse
import json
import re
import sys
from datetime import datetime
from pathlib import Path

import yaml

from .collect import Collector
from .db import Store
from .discover import DEFAULT_INSTANCES, discover, print_report
from .leagues import GAME_DAY_TZ, League, resolve
from .migrate import migrate
from .schema import build_schema
from .ui import DEFAULT_PORT, serve
from .spec import build_spec, combine_specs, prefix_for
from .validate import run_validation

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_DB = REPO_ROOT / "data" / "espn" / "espn.db"
GENERATED_DIR = REPO_ROOT / "data" / "espn" / "generated"

_DURATION_RE = re.compile(r"^(\d+(?:\.\d+)?)([smh]?)$")


def parse_duration(text: str) -> float:
    match = _DURATION_RE.match(text)
    if not match:
        raise argparse.ArgumentTypeError(f"invalid duration {text!r} (try 30s, 90m, 5h)")
    value, unit = float(match.group(1)), match.group(2)
    return value * {"": 1, "s": 1, "m": 60, "h": 3600}[unit]


def cmd_collect(args: argparse.Namespace) -> int:
    try:
        leagues = {arg: resolve(arg) for arg in args.league}
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    date_param = args.date or datetime.now(GAME_DAY_TZ).strftime("%Y%m%d")
    store = Store(args.db)
    try:
        print(f"collecting {', '.join(leagues)} dates={date_param} -> {store.path}", flush=True)
        Collector(
            store,
            leagues,
            date_param,
            duration=args.duration,
            until_all_post=args.until_all_post,
            fixed_interval=args.fixed_interval,
        ).run()
    finally:
        store.close()
    return 0


def cmd_migrate(args: argparse.Namespace) -> int:
    legacy_dir = Path(args.legacy_dir)
    if not legacy_dir.is_dir():
        print(f"error: legacy dir not found: {legacy_dir}", file=sys.stderr)
        return 2
    store = Store(args.db)
    try:
        return 0 if migrate(store, legacy_dir, dry_run=args.dry_run) else 1
    finally:
        store.close()


def cmd_discover(args: argparse.Namespace) -> int:
    leagues = [_resolve(arg) for arg in args.league]
    store = Store(args.db)
    try:
        report = discover(
            store,
            leagues,
            instances=args.instances,
            tag_presence=args.tag_presence,
            max_cardinality=args.max_cardinality,
            min_class_pct=args.min_class_pct,
            source_like=args.source_like,
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


def cmd_ui(args: argparse.Namespace) -> int:
    return serve(
        Path(args.db), GENERATED_DIR, args.port, open_browser=not args.no_browser
    )


def cmd_tray(args: argparse.Namespace) -> int:
    from . import tray  # lazy: pystray only needed for this subcommand

    if args.install_startup:
        value = tray.install_startup()
        print(f"installed HKCU\\...\\Run\\{tray.APP_NAME} = {value}")
        return 0
    if args.uninstall_startup:
        removed = tray.uninstall_startup()
        print("startup entry removed" if removed else "startup entry was not installed")
        return 0
    return tray.main(args.db)


def cmd_status(args: argparse.Namespace) -> int:
    if not Path(args.db).exists():
        print(f"no database at {args.db}", file=sys.stderr)
        return 1
    store = Store(args.db)
    try:
        rows = store.league_stats()
        if not rows:
            print("store is empty")
            return 0
        print(
            f"{'sport':<12}{'league':<16}{'polls':>7}{'distinct':>10}{'dates':>7}"
            f"{'changes':>9}{'non-200':>9}  first .. last (UTC)"
        )
        for sport, league, polls, distinct, dates, first, last, non_200, changed in rows:
            print(
                f"{sport:<12}{league:<16}{polls:>7}{distinct:>10}{dates:>7}"
                f"{changed or 0:>9}{non_200:>9}  {first} .. {last}"
            )
        bodies, raw, stored = store.body_totals()
        print(f"\n{bodies} unique bodies, {raw / 1e6:.1f} MB raw -> {stored / 1e6:.1f} MB stored")
    finally:
        store.close()
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
    store = Store(args.db)
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
    store = Store(args.db)
    try:
        ok = run_validation(store, league, spec_path, root_name, source_like=args.source_like)
    finally:
        store.close()
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument(
        "--db",
        default=str(DEFAULT_DB),
        help="path to the unified store (default: data/espn/espn.db)",
    )

    parser = argparse.ArgumentParser(
        prog="python -m tools.espn",
        description="Collect and analyze raw ESPN scoreboard API responses.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("collect", parents=[common], help="poll live scoreboards into the store")
    p.add_argument(
        "--league",
        action="append",
        required=True,
        help="registry key (e.g. mlb, world-cup) or raw sport/slug; repeatable",
    )
    p.add_argument("--date", help="YYYYMMDD ?dates= value (default: today in US/Eastern)")
    p.add_argument("--duration", type=parse_duration, help="stop after this long (30s / 90m / 5h)")
    p.add_argument(
        "--until-all-post",
        action="store_true",
        help="stop a league once all its events have been final for two consecutive polls",
    )
    p.add_argument(
        "--fixed-interval",
        type=float,
        help="poll every N seconds instead of honoring Cache-Control max-age",
    )
    p.set_defaults(func=cmd_collect)

    p = sub.add_parser(
        "migrate", parents=[common], help="one-shot import of the legacy espn_data_samples DBs"
    )
    p.add_argument(
        "--legacy-dir",
        default=str(REPO_ROOT / "espn_data_samples"),
        help="directory containing the legacy *_collection DBs",
    )
    p.add_argument("--dry-run", action="store_true", help="read and verify without writing")
    p.set_defaults(func=cmd_migrate)

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
    p.add_argument(
        "--source-like", default="%", help="restrict to responses whose source matches (SQL LIKE)"
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
        "ui", parents=[common], help="serve the local read-only pipeline viewer"
    )
    p.add_argument("--port", type=int, default=DEFAULT_PORT, help=f"listen port (default {DEFAULT_PORT})")
    p.add_argument("--no-browser", action="store_true", help="don't open the browser on start")
    p.set_defaults(func=cmd_ui)

    p = sub.add_parser(
        "tray", parents=[common], help="run the system-tray collector (console mode for testing)"
    )
    p.add_argument(
        "--install-startup",
        action="store_true",
        help="register the tray app in HKCU Run so it starts at login, then exit",
    )
    p.add_argument(
        "--uninstall-startup", action="store_true", help="remove the HKCU Run entry, then exit"
    )
    p.set_defaults(func=cmd_tray)

    p = sub.add_parser("status", parents=[common], help="summarize the store")
    p.set_defaults(func=cmd_status)

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
    p.add_argument(
        "--source-like",
        default="%",
        help="restrict to responses whose source matches this SQL LIKE pattern",
    )
    p.set_defaults(func=cmd_validate)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())

"""Inferred JSON Schema + per-state field presence for one league.

Streams every distinct 200-OK body once through both a genson SchemaBuilder
(producing a complete draft-07 schema) and a presence counter (recording, per
game state, how often each field path under competitions[0] appears). Parsed
bodies are never accumulated — each is fed to both consumers and discarded.
"""

import json
from collections import defaultdict

from genson import SchemaBuilder

from .db import Store
from .leagues import League


def collect_paths(obj, prefix: str = "") -> set[str]:
    """Every dotted field path in obj, descending into all array elements."""
    paths = set()
    if isinstance(obj, dict):
        for k, v in obj.items():
            p = f"{prefix}.{k}" if prefix else k
            paths.add(p)
            paths.update(collect_paths(v, p))
    elif isinstance(obj, list) and obj:
        for item in obj:
            paths.update(collect_paths(item, f"{prefix}[]"))
    return paths


def build_schema(store: Store, league: League) -> tuple[dict, dict]:
    """Return (inferred draft-07 schema, presence document) for a league.

    The presence document is `{"_state_totals": {...}, "fields": {path: {state:
    {count, total, pct}}}}`, matching what the spec builder consumes.
    """
    builder = SchemaBuilder()
    state_totals: defaultdict[str, int] = defaultdict(int)
    state_field_counts: defaultdict[str, defaultdict[str, int]] = defaultdict(
        lambda: defaultdict(int)
    )

    count = 0
    for body in store.iter_bodies(league.slug, http_status=200, distinct=True):
        resp = json.loads(body)
        builder.add_object(resp)
        for event in resp.get("events", []):
            comps = event.get("competitions", [])
            if not comps:
                continue
            comp = comps[0]
            state = comp.get("status", {}).get("type", {}).get("state", "unknown")
            state_totals[state] += 1
            for path in collect_paths(comp):
                state_field_counts[state][path] += 1
        count += 1

    schema = builder.to_schema()
    schema["$schema"] = "http://json-schema.org/draft-07/schema#"

    all_paths = set()
    for counts in state_field_counts.values():
        all_paths.update(counts)

    fields = {}
    for path in sorted(all_paths):
        entry = {}
        for state in ("pre", "in", "post"):
            total = state_totals.get(state, 0)
            n = state_field_counts.get(state, {}).get(path, 0)
            pct = round(n / total * 100, 2) if total else 0
            entry[state] = {"count": n, "total": total, "pct": pct}
        fields[path] = entry

    presence = {"_state_totals": dict(state_totals), "fields": fields}
    print(
        f"{league.slug}: {count} distinct bodies, states {dict(state_totals)},"
        f" {len(fields)} field paths"
    )
    return schema, presence

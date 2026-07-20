"""Validate stored responses against a league's OpenAPI spec.

Loads an OpenAPI YAML, builds a Draft202012Validator (OpenAPI 3.1 is JSON Schema
2020-12) rooted at one component schema with a referencing Registry so internal
$refs resolve, then streams distinct bodies through it. Reports per-state
pass/fail, drilling into oneOf failures to surface the variant that was intended.
"""

import json
from collections import Counter
from pathlib import Path

import yaml
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from referencing.jsonschema import DRAFT202012

from .db import Store
from .leagues import League

_BASE_URI = "https://local/spec.yaml"


def _build_validator(spec: dict, root_name: str) -> Draft202012Validator:
    components = spec["components"]["schemas"]
    if root_name not in components:
        raise KeyError(
            f"root schema {root_name!r} not in spec; available: {sorted(components)}"
        )
    registry = Registry().with_resources(
        (
            f"{_BASE_URI}#/components/schemas/{name}",
            Resource(contents=schema, specification=DRAFT202012),
        )
        for name, schema in components.items()
    )
    root_schema = {"$id": _BASE_URI, **components[root_name], "components": {"schemas": components}}
    return Draft202012Validator(schema=root_schema, registry=registry)


def _explain(err) -> tuple[str, str]:
    """(path, message) for one error, drilling past the oneOf state-const noise."""
    path = ".".join(str(p) for p in err.absolute_path) or "(root)"
    if not err.context:
        return path, err.message[:200]
    best = None
    for sub in err.context:
        sub_path = ".".join(str(p) for p in sub.absolute_path)
        if "state" in sub_path or "'const'" in sub.message:
            continue
        if best is None or len(sub_path) > len(".".join(str(p) for p in best.absolute_path)):
            best = sub
    if best is None:
        return path, "oneOf: no variant matched (state const mismatch)"
    best_path = ".".join(str(p) for p in best.absolute_path) or "(root)"
    return f"{path} -> {best_path}", best.message[:200]


def _state(event: dict) -> str:
    comps = event.get("competitions", [])
    if not comps:
        return "unknown"
    return comps[0].get("status", {}).get("type", {}).get("state", "unknown")


def run_validation(
    store: Store,
    league: League,
    spec_path: Path,
    root_name: str,
) -> bool:
    """Validate a league's distinct bodies; returns True iff every body passes."""
    spec = yaml.safe_load(Path(spec_path).read_text(encoding="utf-8"))
    validator = _build_validator(spec, root_name)
    print(f"{league.slug}: spec {spec_path}, root {root_name}")

    response_pass = response_fail = 0
    state_totals: Counter[str] = Counter()
    state_passes: Counter[str] = Counter()
    state_fails: Counter[str] = Counter()
    error_counter: Counter[tuple[str, str]] = Counter()
    failing_examples = []

    bodies = store.iter_bodies(league.slug, http_status=200, distinct=True)
    for i, body in enumerate(bodies):
        resp = json.loads(body)
        events = resp.get("events", [])
        for event in events:
            state_totals[_state(event)] += 1

        errors = list(validator.iter_errors(resp))
        if not errors:
            response_pass += 1
            for event in events:
                state_passes[_state(event)] += 1
            continue

        response_fail += 1
        for event in events:
            state_fails[_state(event)] += 1
        if len(failing_examples) < 10:
            failing_examples.append((i, [_explain(e) for e in errors[:5]]))
        for err in errors:
            error_counter[_explain(err)] += 1

    total = response_pass + response_fail
    pct = response_pass / total * 100 if total else 0
    print(f"\nresponses: {response_pass} pass / {response_fail} fail ({pct:.1f}%)")
    for state in ("pre", "in", "post", "unknown"):
        st = state_totals.get(state, 0)
        if not st:
            continue
        passed = state_passes.get(state, 0)
        failed = state_fails.get(state, 0)
        print(f"  {state:<8}{passed:>7} pass /{failed:>7} fail  ({passed / st * 100:.1f}%)")

    if error_counter:
        print("\ntop error paths:")
        for (path, msg), count in error_counter.most_common(20):
            print(f"  [{count:>5}x] {path}\n           {msg}")
        print(f"\nfirst {len(failing_examples)} failing bodies:")
        for idx, errs in failing_examples:
            print(f"  #{idx}:")
            for path, msg in errs:
                print(f"    - {path}: {msg}")

    print("\nPASS: every body validates." if not response_fail else f"\nFAIL: {response_fail} bodies failed.")
    return response_fail == 0

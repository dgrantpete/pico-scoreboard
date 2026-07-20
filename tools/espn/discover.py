"""Automatic discriminated-union discovery over field presence.

Treats every event's competitions[0] as an instance and searches for scalar
fields whose *value* predicts the presence/absence of other fields — the
statistical signature of a discriminated union. Every low-cardinality scalar
reachable through dicts (never arrays: a tag must be single-valued per
instance) with near-total coverage is a candidate; candidates are ranked by
aggregate information gain (the decision-tree criterion, applied to each
field's presence and summed). Instances missing a candidate's tag — e.g.
glitch payloads whose events are empty objects — fall into an explicitly
reported "missing" bucket rather than disqualifying the discriminant; the
coverage requirement is tunable via tag_presence.

Two streaming passes over distinct bodies, then instances collapse into
weighted (tag values, presence bitmask) signatures, so scoring and per-class
recursion are cheap in-memory bit math.
"""

import json
import math
import re
from collections import Counter, defaultdict

from .db import Store
from .leagues import League
from .schema import collect_paths

DEFAULT_INSTANCES = "events[].competitions[0]"
SYNTHETIC_LEAGUE_TAG = "_league"  # injected on pooled runs; exempt from cardinality cap
DEFAULT_TAG_PRESENCE = 0.99
DEFAULT_MAX_CARDINALITY = 6
DEFAULT_MIN_CLASS_PCT = 1.0
# a field counts as "explained" when its presence is mixed overall but
# near-deterministic inside every class; matches spec.py's 1%/100% thresholds
DETERMINISM_PCT = 1.0
_BLACKLISTED = object()


def _scalar_items(obj: dict, prefix: str = ""):
    """(path, value) for scalar fields reachable through dicts only."""
    for key, value in obj.items():
        path = f"{prefix}.{key}" if prefix else key
        if isinstance(value, dict):
            yield from _scalar_items(value, path)
        elif isinstance(value, (str, bool, int)):
            yield path, value


def _entropy(p: float) -> float:
    if p <= 0.0 or p >= 1.0:
        return 0.0
    return -(p * math.log2(p) + (1.0 - p) * math.log2(1.0 - p))


_SEGMENT_RE = re.compile(r"^([A-Za-z_]\w*)(?:\[(\d*)\])?$")


def parse_instance_path(path: str) -> list[tuple[str, int | str | None]]:
    """'events[].competitions[0].situation.lastPlay' -> walk steps.

    Each step is (key, selector): selector None follows a plain dict key,
    'all' iterates every element of an array, an int picks one element.
    Re-rooting instances at an array element ('plays[]') is what makes
    per-element unions discoverable — competition-level flattening destroys
    the pairing between an element's tag and that same element's fields.
    """
    steps = []
    for segment in path.split("."):
        match = _SEGMENT_RE.match(segment)
        if not match:
            raise ValueError(f"bad instance-path segment {segment!r} in {path!r}")
        key, index = match.group(1), match.group(2)
        selector = None if index is None else ("all" if index == "" else int(index))
        steps.append((key, selector))
    return steps


def _walk_instances(obj, steps: list):
    if not steps:
        if isinstance(obj, dict):
            yield obj
        return
    if not isinstance(obj, dict):
        return
    key, selector = steps[0]
    value = obj.get(key)
    if selector is None:
        yield from _walk_instances(value, steps[1:])
    elif selector == "all":
        if isinstance(value, list):
            for item in value:
                yield from _walk_instances(item, steps[1:])
    elif isinstance(value, list) and len(value) > selector:
        yield from _walk_instances(value[selector], steps[1:])


def _iter_instances(store: Store, league: League, steps: list):
    for body in store.iter_bodies(league.slug, http_status=200, distinct=True):
        try:
            doc = json.loads(body)
        except ValueError:
            continue
        if isinstance(doc, dict):
            yield from _walk_instances(doc, steps)


def _iter_pooled(store: Store, leagues: list[League], steps: list):
    """Instances across leagues. When pooling, each instance gets a synthetic
    `_league` scalar so sport identity competes as a discriminant candidate —
    letting the beam compare sport-first vs state-first hierarchies, which is
    the quantitative form of 'how much model/display structure is shared'."""
    pooled = len(leagues) > 1
    for league in leagues:
        for instance in _iter_instances(store, league, steps):
            if pooled:
                instance[SYNTHETIC_LEAGUE_TAG] = league.slug
            yield instance


class _Signatures:
    """Instances collapsed to weighted (candidate values, presence bitmask) rows."""

    def __init__(self, paths: list[str], candidates: list[str]):
        self.paths = paths
        self.candidates = candidates
        self.rows: Counter[tuple[tuple, int]] = Counter()
        self._path_index = {p: i for i, p in enumerate(paths)}

    def add(self, comp: dict) -> None:
        mask = 0
        for path in collect_paths(comp):
            mask |= 1 << self._path_index[path]
        scalars = dict(_scalar_items(comp))
        values = tuple(scalars.get(c) for c in self.candidates)
        self.rows[(values, mask)] += 1

    def total(self) -> int:
        return sum(self.rows.values())

    def filtered(self, cand_index: int, value) -> "_Signatures":
        sub = _Signatures(self.paths, self.candidates)
        for (values, mask), count in self.rows.items():
            if values[cand_index] == value:
                sub.rows[(values, mask)] += count
        return sub


def _path_counts(rows: list[tuple[int, int]], n_paths: int) -> list[int]:
    """Presence count per path index over weighted (mask, count) rows."""
    counts = [0] * n_paths
    for mask, count in rows:
        while mask:
            low = mask & -mask
            counts[low.bit_length() - 1] += count
            mask ^= low
    return counts


def _score_candidate(sigs: _Signatures, cand_index: int) -> dict | None:
    """Information-gain score for one candidate tag, or None if degenerate."""
    buckets: defaultdict = defaultdict(list)
    missing = 0
    for (values, mask), count in sigs.rows.items():
        value = values[cand_index]
        if value is None:
            missing += count
        else:
            buckets[value].append((mask, count))
    if len(buckets) < 2:
        return None

    n_paths = len(sigs.paths)
    tag_path = sigs.candidates[cand_index]
    classes = {}
    base_counts = [0] * n_paths
    tagged = 0
    for value, rows in buckets.items():
        n_c = sum(count for _, count in rows)
        counts = _path_counts(rows, n_paths)
        classes[value] = (n_c, counts)
        tagged += n_c
        for i, c in enumerate(counts):
            base_counts[i] += c

    gain = 0.0
    field_gains = {}
    explained = []
    lo, hi = DETERMINISM_PCT / 100.0, 1.0 - DETERMINISM_PCT / 100.0
    for i, path in enumerate(sigs.paths):
        if path == tag_path:
            continue
        p = base_counts[i] / tagged
        h = _entropy(p)
        if h == 0.0:
            continue
        h_cond = sum(
            (n_c / tagged) * _entropy(counts[i] / n_c) for n_c, counts in classes.values()
        )
        g = h - h_cond
        if g <= 0.0:
            continue
        gain += g
        field_gains[path] = g
        if lo < p < hi and all(
            counts[i] / n_c <= lo or counts[i] / n_c >= hi for n_c, counts in classes.values()
        ):
            explained.append(path)

    return {
        "tag": tag_path,
        "gain": gain,
        "classes": {
            str(value): {"count": n_c, "pct": round(n_c / tagged * 100, 2)}
            for value, (n_c, counts) in sorted(classes.items(), key=lambda kv: -kv[1][0])
        },
        "class_values": list(buckets),
        "missing": missing,
        "explained_fields": sorted(explained, key=lambda p: -field_gains[p]),
        "field_gains": field_gains,
        "class_presence": {
            str(value): {
                sigs.paths[i]: round(counts[i] / n_c * 100, 2)
                for i in range(n_paths)
                if sigs.paths[i] in field_gains
            }
            for value, (n_c, counts) in classes.items()
        },
    }


def _partition_key(rows: list, index: int) -> tuple:
    """Fingerprint of the instance partition a candidate induces; candidates
    with equal keys are aliases (e.g. status.type.state/name/id/description)
    and only one of them needs scoring or beam exploration."""
    labels: dict = {}
    return tuple(labels.setdefault(values[index], len(labels)) for (values, _), _ in rows)


def _rank(sigs: _Signatures, *, tag_presence: float, max_cardinality: int) -> list[dict]:
    total = sigs.total()
    if total == 0:
        return []
    rows = list(sigs.rows.items())
    groups: dict[tuple, list[tuple[int, int]]] = {}
    for index in range(len(sigs.candidates)):
        values = set()
        covered = 0
        for (row_values, _), count in rows:
            if row_values[index] is not None:
                covered += count
                values.add(row_values[index])
        if covered < tag_presence * total or len(values) < 2:
            continue
        if len(values) > max_cardinality and sigs.candidates[index] != SYNTHETIC_LEAGUE_TAG:
            continue
        groups.setdefault(_partition_key(rows, index), []).append((index, covered))

    scored = []
    for members in groups.values():
        index, covered = members[0]
        result = _score_candidate(sigs, index)
        if result is not None and result["gain"] > 0.0:
            result["index"] = index
            result["coverage_pct"] = round(covered / total * 100, 2)
            result["aliases"] = [sigs.candidates[i] for i, _ in members[1:]]
            scored.append(result)
    return sorted(scored, key=lambda r: -r["gain"])


def _beam(
    sigs: _Signatures,
    *,
    depth: int,
    beam_width: int,
    tag_presence: float,
    max_cardinality: int,
    min_class_pct: float,
    min_split_gain: float,
    ranked: list[dict] | None = None,
) -> list[dict]:
    """Beam search over split hierarchies, MDL-scored.

    Trees are ranked by net description-length savings: the data term is total
    weighted gain × instances (bits no longer needed to encode field-presence
    surprises), the model term charges each variant beyond the first one
    presence-bitmap over the object's field universe — which is literally what
    a variant costs to state in the generated spec. A subtree attaches only
    when its data savings exceed its variant cost, so marginal splits prune
    themselves and finer state machines win exactly when the corpus is large
    enough to justify them.

    Only candidates that deterministically explain at least one field may
    anchor a split: a tag with high raw gain but zero explained fields is
    identity leakage (game id/date/venue on a small corpus), not a union."""
    if ranked is None:
        ranked = _rank(sigs, tag_presence=tag_presence, max_cardinality=max_cardinality)
    floor = max(5, min_class_pct / 100.0 * sigs.total())
    n_paths = len(sigs.paths)
    trees = []
    for r in [r for r in ranked if r["explained_fields"]][:beam_width]:
        tagged = sum(info["count"] for info in r["classes"].values())
        node = {
            "tag": r["tag"],
            "aliases": r["aliases"],
            "gain_bits": r["gain"],
            "total_bits": r["gain"],
            "instances": tagged,
            "leaves": 0,
            "classes": {},
        }
        for value in r["class_values"]:
            info = r["classes"][str(value)]
            cls = {"count": info["count"], "pct": info["pct"], "subtree": None}
            node["leaves"] += 1
            if depth > 1 and info["count"] >= floor:
                subtrees = _beam(
                    sigs.filtered(r["index"], value),
                    depth=depth - 1,
                    beam_width=beam_width,
                    tag_presence=tag_presence,
                    max_cardinality=max_cardinality,
                    min_class_pct=min_class_pct,
                    min_split_gain=min_split_gain,
                )
                if subtrees:
                    best = subtrees[0]
                    savings = best["total_bits"] * best["instances"]
                    cost = (best["leaves"] - 1) * n_paths
                    if best["total_bits"] >= min_split_gain and savings > cost:
                        cls["subtree"] = best
                        node["leaves"] += best["leaves"] - 1
                        node["total_bits"] += (info["count"] / tagged) * best["total_bits"]
            node["classes"][str(value)] = cls
        node["data_bits"] = node["total_bits"] * tagged
        node["model_bits"] = (node["leaves"] - 1) * n_paths
        node["net_bits"] = node["data_bits"] - node["model_bits"]
        trees.append(node)
    return sorted(trees, key=lambda t: (-t["net_bits"], t["leaves"]))


def discover(
    store: Store,
    leagues: list[League],
    *,
    instances: str = DEFAULT_INSTANCES,
    tag_presence: float = DEFAULT_TAG_PRESENCE,
    max_cardinality: int = DEFAULT_MAX_CARDINALITY,
    min_class_pct: float = DEFAULT_MIN_CLASS_PCT,
    beam_width: int = 3,
    max_depth: int = 2,
    min_split_gain: float = 1.0,
) -> dict:
    """Two-pass discovery; returns the full report document."""
    steps = parse_instance_path(instances)
    # pass 1: path universe + candidate shortlist (coverage checked precisely later)
    total = 0
    all_paths: set[str] = set()
    scalar_counts: Counter[str] = Counter()
    scalar_values: dict[str, object] = {}
    for comp in _iter_pooled(store, leagues, steps):
        total += 1
        all_paths |= collect_paths(comp)
        for path, value in _scalar_items(comp):
            scalar_counts[path] += 1
            seen = scalar_values.get(path)
            if seen is _BLACKLISTED:
                continue
            if seen is None:
                seen = scalar_values[path] = set()
            seen.add(value)
            if len(seen) > max_cardinality and path != SYNTHETIC_LEAGUE_TAG:
                scalar_values[path] = _BLACKLISTED

    candidates = sorted(
        path
        for path, seen in scalar_values.items()
        if seen is not _BLACKLISTED
        and len(seen) >= 2
        and scalar_counts[path] >= tag_presence * total
    )

    sigs = _Signatures(sorted(all_paths), candidates)
    for comp in _iter_pooled(store, leagues, steps):
        sigs.add(comp)

    ranking = _rank(sigs, tag_presence=tag_presence, max_cardinality=max_cardinality)
    floor = max(5, min_class_pct / 100.0 * sigs.total())
    for r in ranking:
        for info in r["classes"].values():
            if info["count"] < floor:
                info["below_min_support"] = True

    trees = _beam(
        sigs,
        depth=max_depth,
        beam_width=beam_width,
        tag_presence=tag_presence,
        max_cardinality=max_cardinality,
        min_class_pct=min_class_pct,
        min_split_gain=min_split_gain,
        ranked=ranking,
    )

    return {
        "league": "+".join(lg.slug for lg in leagues),
        "instance_path": instances,
        "instances": total,
        "paths": len(all_paths),
        "candidates_considered": len(candidates),
        "tag_presence": tag_presence,
        "max_cardinality": max_cardinality,
        "beam_width": beam_width,
        "max_depth": max_depth,
        "min_split_gain": min_split_gain,
        "ranking": [_strip(r) for r in ranking],
        "trees": [_round_tree(t) for t in trees],
    }


def _strip(result: dict) -> dict:
    """Report form of a scored candidate (drops bulky internals)."""
    top = result["explained_fields"][:12]
    return {
        "tag": result["tag"],
        "aliases": result["aliases"],
        "gain_bits": round(result["gain"], 2),
        "coverage_pct": result["coverage_pct"],
        "missing_instances": result["missing"],
        "classes": result["classes"],
        "explained_field_count": len(result["explained_fields"]),
        "top_explained_fields": {
            path: {cls: presence[path] for cls, presence in result["class_presence"].items() if path in presence}
            for path in top
        },
    }


def _round_tree(node: dict) -> dict:
    return {
        "tag": node["tag"],
        "aliases": node["aliases"],
        "gain_bits": round(node["gain_bits"], 2),
        "total_bits": round(node["total_bits"], 2),
        "instances": node["instances"],
        "data_bits": round(node["data_bits"]),
        "model_bits": node["model_bits"],
        "net_bits": round(node["net_bits"]),
        "leaves": node["leaves"],
        "classes": {
            cls: {
                "count": info["count"],
                "pct": info["pct"],
                "subtree": _round_tree(info["subtree"]) if info["subtree"] else None,
            }
            for cls, info in node["classes"].items()
        },
    }


def print_report(report: dict, top: int = 10) -> None:
    print(
        f"{report['league']} ({report['instance_path']}):"
        f" {report['instances']} instances, {report['paths']} field paths,"
        f" {report['candidates_considered']} candidate tags"
        f" (coverage >= {report['tag_presence'] * 100:.0f}%, cardinality <= {report['max_cardinality']})"
    )
    if not report["ranking"]:
        print("no discriminant found — this object looks like plain optional fields, not a union")
        return

    print(f"\n{'rank':<6}{'gain':>8}  {'coverage':>9}  {'explains':>9}  tag = values")
    for i, r in enumerate(report["ranking"][:top], 1):
        values = ", ".join(r["classes"])
        alias_note = f"  (aliases: {', '.join(r['aliases'])})" if r["aliases"] else ""
        print(
            f"{i:<6}{r['gain_bits']:>8}  {r['coverage_pct']:>8}%  {r['explained_field_count']:>9}"
            f"  {r['tag']} = {{{values}}}{alias_note}"
        )

    if not report["trees"]:
        print(
            "\nno candidate deterministically explains any field — this object is a"
            " plain struct with optional fields, not a discriminated union"
        )
        return

    print(
        f"\nsplit hierarchies (beam {report['beam_width']}, depth {report['max_depth']},"
        f" MDL-ranked: net = data savings - variant description cost):"
    )
    for i, tree in enumerate(report["trees"], 1):
        print(
            f"  tree {i}: net {tree['net_bits']:,} bits"
            f" (data {tree['data_bits']:,} - model {tree['model_bits']:,}),"
            f" {tree['leaves']} leaf variants"
        )
        _print_tree(tree, indent="    ")

    winner = report["ranking"][0]
    print(f"\ndiscriminant: {winner['tag']}")
    if winner["missing_instances"]:
        print(
            f"  {winner['missing_instances']} instances lack the tag (glitch/partial payloads)"
            " — excluded from variants"
        )
    for cls, info in winner["classes"].items():
        note = "  [below min support — no variant]" if info.get("below_min_support") else ""
        print(f"  variant {cls!r}: {info['count']} instances ({info['pct']}%){note}")
    print("\n  top fields explained by the discriminant (presence % per variant):")
    for path, per_class in winner["top_explained_fields"].items():
        cells = "  ".join(f"{cls}={pct:g}%" for cls, pct in per_class.items())
        print(f"    {path}: {cells}")


def _print_tree(node: dict, indent: str) -> None:
    alias_note = f" (= {', '.join(node['aliases'])})" if node["aliases"] else ""
    print(f"{indent}{node['tag']}{alias_note} gain {node['gain_bits']}")
    for cls, info in node["classes"].items():
        if info["subtree"]:
            print(f"{indent}  {cls!r} ({info['pct']}%) ->")
            _print_tree(info["subtree"], indent + "    ")
        else:
            print(f"{indent}  {cls!r} ({info['pct']}%)")

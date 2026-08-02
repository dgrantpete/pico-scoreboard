"""DU-aware OpenAPI 3.1 spec construction from an inferred schema + presence data.

`build_spec` turns one league's inferred schema and field-presence into an
OpenAPI document whose Competition schema is a discriminated union over
status.type.state: a CompetitionBase of fields common to every state plus
PreGame/Live/Final variants. A field enters a state's variant when it is present
in >=1% of that state's competitions, and is marked required only at 100%.

`combine_specs` merges several per-league specs, sharing component schemas that
are structurally identical across all leagues (fixpoint-pruned so a shared
schema never references a league-specific one) and prefixing the rest by league.
"""

import copy
import re

from .leagues import League

PRESENCE_THRESHOLD = 1.0
REQUIRED_THRESHOLD = 100.0

_STATE_VARIANTS = (
    ("PreGameCompetition", "pre"),
    ("LiveCompetition", "in"),
    ("FinalCompetition", "post"),
)


def prefix_for(slug: str) -> str:
    """Component-name prefix for a league slug: 'mlb'->'Mlb', 'fifa.world'->'FifaWorld'."""
    return "".join(part.capitalize() for part in re.split(r"[^a-z0-9]+", slug.lower()) if part)


def _strip_draft_metadata(schema) -> None:
    if isinstance(schema, dict):
        schema.pop("$schema", None)
        for v in schema.values():
            _strip_draft_metadata(v)
    elif isinstance(schema, list):
        for item in schema:
            _strip_draft_metadata(item)


def _extract_competition_schema(inferred: dict) -> dict:
    events = inferred["properties"]["events"]
    event = events["items"] if isinstance(events.get("items"), dict) else events["items"][0]
    comps = event["properties"]["competitions"]
    return comps["items"] if isinstance(comps.get("items"), dict) else comps["items"][0]


def _presence_states(field_name: str, presence: dict) -> set[str]:
    entry = presence.get(field_name)
    if entry is None:
        return set()
    return {
        state
        for state in ("pre", "in", "post")
        if entry.get(state, {}).get("pct", 0) >= PRESENCE_THRESHOLD
    }


def _is_required(field_name: str, state: str, presence: dict) -> bool:
    entry = presence.get(field_name)
    if entry is None:
        return False
    return entry.get(state, {}).get("pct", 0) >= REQUIRED_THRESHOLD


def _build_variant(state_const: str, variant_props: dict, variant_required: set[str]) -> dict:
    """A variant is allOf: [CompetitionBase, {extra fields + status.type.state const pin}]."""
    variant = {"type": "object", "properties": dict(variant_props)}
    if variant_required:
        variant["required"] = sorted(variant_required)

    if "status" not in variant["properties"]:
        variant["properties"]["status"] = {"type": "object"}
    status_schema = copy.deepcopy(variant["properties"]["status"])
    status_schema.setdefault("properties", {})
    type_schema = status_schema["properties"].setdefault("type", {"type": "object"})
    type_schema.setdefault("properties", {})["state"] = {"const": state_const}
    type_schema["required"] = sorted(set(type_schema.get("required", [])) | {"state"})
    status_schema["properties"]["type"] = type_schema
    status_schema["required"] = sorted(set(status_schema.get("required", [])) | {"type"})
    variant["properties"]["status"] = status_schema
    variant["required"] = sorted(set(variant.get("required", [])) | {"status"})

    return {"allOf": [{"$ref": "#/components/schemas/CompetitionBase"}, variant]}


def build_spec(schema: dict, presence_doc: dict, league: League) -> dict:
    """Build a discriminated-union OpenAPI 3.1 document for one league."""
    presence = presence_doc.get("fields", presence_doc)
    comp_schema = _extract_competition_schema(schema)
    comp_props = comp_schema.get("properties", {})
    comp_required = set(comp_schema.get("required", []))

    common_key = frozenset({"pre", "in", "post"})
    categorization: dict[frozenset[str], list[str]] = {
        common_key: [],
        frozenset({"pre"}): [],
        frozenset({"in"}): [],
        frozenset({"post"}): [],
        frozenset({"pre", "in"}): [],
        frozenset({"pre", "post"}): [],
        frozenset({"in", "post"}): [],
    }
    for field_name in comp_props:
        states = _presence_states(field_name, presence)
        # A field absent from presence data (edge case) is treated as common.
        categorization[frozenset(states) if states else common_key].append(field_name)

    common_fields = categorization[common_key]
    base_props = {f: comp_props[f] for f in common_fields}
    base_required = sorted(f for f in common_fields if f in comp_required)
    competition_base = {"type": "object", "properties": base_props}
    if base_required:
        competition_base["required"] = base_required

    def fields_for_variant(state: str) -> list[str]:
        fields = list(categorization[frozenset({state})])
        for pair in (frozenset({"pre", "in"}), frozenset({"pre", "post"}), frozenset({"in", "post"})):
            if state in pair:
                fields.extend(categorization[pair])
        return fields

    variants = {}
    for variant_name, state_const in _STATE_VARIANTS:
        fields = fields_for_variant(state_const)
        variant_props = {f: comp_props[f] for f in fields}
        variant_required = {f for f in fields if _is_required(f, state_const, presence)}
        variants[variant_name] = _build_variant(state_const, variant_props, variant_required)

    event_schema = copy.deepcopy(schema["properties"]["events"]["items"])
    event_schema["properties"]["competitions"]["items"] = {
        "$ref": "#/components/schemas/Competition"
    }

    scoreboard_response = copy.deepcopy(schema)
    scoreboard_response["properties"]["events"]["items"] = {"$ref": "#/components/schemas/Event"}

    components_schemas = {
        "ScoreboardResponse": scoreboard_response,
        "Event": event_schema,
        "Competition": {
            "oneOf": [
                {"$ref": "#/components/schemas/PreGameCompetition"},
                {"$ref": "#/components/schemas/LiveCompetition"},
                {"$ref": "#/components/schemas/FinalCompetition"},
            ]
        },
        "CompetitionBase": competition_base,
        "PreGameCompetition": variants["PreGameCompetition"],
        "LiveCompetition": variants["LiveCompetition"],
        "FinalCompetition": variants["FinalCompetition"],
    }
    _strip_draft_metadata(components_schemas)

    label = league.slug.upper()
    return {
        "openapi": "3.1.0",
        "info": {
            "title": f"ESPN {label} Scoreboard API",
            "description": (
                f"Empirically-derived OpenAPI 3.1 spec for ESPN's {label} scoreboard endpoint. "
                "The Competition schema is a discriminated union over status.type.state "
                "(pre/in/post), built from analysis of real response data."
            ),
            "version": "1.0.0",
        },
        "servers": [{"url": "https://site.api.espn.com/apis/site/v2"}],
        "paths": {
            f"/sports/{league.sport}/{league.slug}/scoreboard": {
                "get": {
                    "summary": f"Get {label} scoreboard for a date",
                    "parameters": [
                        {
                            "name": "dates",
                            "in": "query",
                            "description": "Date in YYYYMMDD format",
                            "schema": {"type": "string", "pattern": r"^\d{8}$"},
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Scoreboard response",
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/ScoreboardResponse"}
                                }
                            },
                        }
                    },
                }
            }
        },
        "components": {"schemas": components_schemas},
    }


def _find_ref_names(obj) -> set[str]:
    names = set()
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == "$ref" and isinstance(v, str) and v.startswith("#/components/schemas/"):
                names.add(v.split("/")[-1])
            else:
                names.update(_find_ref_names(v))
    elif isinstance(obj, list):
        for item in obj:
            names.update(_find_ref_names(item))
    return names


def _rewrite_refs(obj, rename_map: dict[str, str]) -> None:
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == "$ref" and isinstance(v, str) and v.startswith("#/components/schemas/"):
                old = v.split("/")[-1]
                if old in rename_map:
                    obj[k] = f"#/components/schemas/{rename_map[old]}"
            else:
                _rewrite_refs(v, rename_map)
    elif isinstance(obj, list):
        for item in obj:
            _rewrite_refs(item, rename_map)


def _shared_set(comps_by_prefix: dict[str, dict]) -> set[str]:
    """Names whose component is deep-equal across every league and refs only shared names."""
    prefixes = list(comps_by_prefix)
    common = set.intersection(*(set(comps_by_prefix[p]) for p in prefixes))
    first = comps_by_prefix[prefixes[0]]
    shared = {
        n for n in common if all(comps_by_prefix[p][n] == first[n] for p in prefixes[1:])
    }
    while True:
        to_remove = {
            n
            for n in shared
            for p in prefixes
            if not _find_ref_names(comps_by_prefix[p][n]) <= shared
        }
        if not to_remove:
            return shared
        shared -= to_remove


def combine_specs(specs_by_prefix: dict[str, dict]) -> dict:
    """Merge per-league specs (keyed by component prefix, e.g. 'Mlb') into one document."""
    comps_by_prefix = {
        prefix: copy.deepcopy(spec["components"]["schemas"])
        for prefix, spec in specs_by_prefix.items()
    }
    shared = _shared_set(comps_by_prefix)

    merged_comps = {n: comps_by_prefix[next(iter(comps_by_prefix))][n] for n in sorted(shared)}
    merged_paths = {}
    for prefix, spec in specs_by_prefix.items():
        comps = comps_by_prefix[prefix]
        rename = {n: n if n in shared else f"{prefix}{n}" for n in comps}
        for schema in comps.values():
            _rewrite_refs(schema, rename)
        paths = copy.deepcopy(spec["paths"])
        _rewrite_refs(paths, rename)
        merged_paths.update(paths)
        for old, new in rename.items():
            if old not in shared:
                merged_comps[new] = comps[old]

    labels = " + ".join(specs_by_prefix)
    return {
        "openapi": "3.1.0",
        "info": {
            "title": f"ESPN Scoreboard API ({labels})",
            "description": (
                "Empirically-derived OpenAPI 3.1 spec for ESPN's scoreboard endpoints. "
                "Each sport's Competition schema is a discriminated union over "
                "status.type.state (pre/in/post). Schemas structurally identical across "
                "every sport are merged into shared components."
            ),
            "version": "1.0.0",
        },
        "servers": [{"url": "https://site.api.espn.com/apis/site/v2"}],
        "paths": merged_paths,
        "components": {"schemas": merged_comps},
    }

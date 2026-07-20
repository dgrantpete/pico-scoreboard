"""Declarative poll-target model: what the collector service polls.

`targets.yml` is the single runtime-config surface — adding a sport is a
reviewed diff to that file, never a code change. Parsing is strict on
purpose: an unknown key, duplicate name, or unknown `kind` is an error, so a
typo'd deploy fails loudly instead of silently not collecting.

Endpoint kinds are a discriminated union on `kind`; today only `scoreboard`
exists (summary polling is derived per live event from a scoreboard target's
`follow_summaries`, not declared directly). New kinds add a dataclass and a
`_parse_*` branch without touching existing entries.
"""

from dataclasses import dataclass
from pathlib import Path

import yaml

SITE_API = "https://site.api.espn.com/apis/site/v2/sports"

_DEFAULTS_KEYS = {"idle_interval"}
_SCOREBOARD_KEYS = {
    "name",
    "kind",
    "sport",
    "league",
    "enabled",
    "follow_summaries",
    "idle_interval",
    "fixed_interval",
}


class TargetsError(ValueError):
    """Invalid targets file. Message carries the full context for logs."""


@dataclass(frozen=True)
class ScoreboardTarget:
    name: str
    sport: str
    league: str
    enabled: bool
    follow_summaries: bool
    idle_interval: float | None
    fixed_interval: float | None

    @property
    def scoreboard_url(self) -> str:
        return f"{SITE_API}/{self.sport}/{self.league}/scoreboard"

    def summary_url(self, event_id: str) -> str:
        return f"{SITE_API}/{self.sport}/{self.league}/summary?event={event_id}"

    def as_doc(self) -> dict:
        """JSON form recorded on the collector session row."""
        return {
            "name": self.name,
            "kind": "scoreboard",
            "sport": self.sport,
            "league": self.league,
            "follow_summaries": self.follow_summaries,
            "idle_interval": self.idle_interval,
            "fixed_interval": self.fixed_interval,
        }


@dataclass(frozen=True)
class TargetsFile:
    targets: tuple[ScoreboardTarget, ...]

    def enabled(self) -> tuple[ScoreboardTarget, ...]:
        return tuple(t for t in self.targets if t.enabled)


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise TargetsError(message)


def _interval(entry: dict, key: str, default: float | None) -> float | None:
    value = entry.get(key, default)
    if value is None:
        return None
    _require(
        isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0,
        f"{key} must be a positive number, got {value!r}",
    )
    return float(value)


def _parse_scoreboard(entry: dict, defaults: dict) -> ScoreboardTarget:
    unknown = set(entry) - _SCOREBOARD_KEYS
    _require(not unknown, f"unknown keys {sorted(unknown)} in target {entry.get('name')!r}")
    for key in ("name", "sport", "league"):
        _require(
            isinstance(entry.get(key), str) and entry[key],
            f"target missing required string {key!r}: {entry!r}",
        )
    enabled = entry.get("enabled", True)
    follow = entry.get("follow_summaries", False)
    _require(isinstance(enabled, bool), f"enabled must be a bool in target {entry['name']!r}")
    _require(
        isinstance(follow, bool), f"follow_summaries must be a bool in target {entry['name']!r}"
    )
    return ScoreboardTarget(
        name=entry["name"],
        sport=entry["sport"],
        league=entry["league"],
        enabled=enabled,
        follow_summaries=follow,
        idle_interval=_interval(entry, "idle_interval", defaults.get("idle_interval")),
        fixed_interval=_interval(entry, "fixed_interval", None),
    )


def parse_targets(text: str) -> TargetsFile:
    try:
        doc = yaml.safe_load(text)
    except yaml.YAMLError as exc:
        raise TargetsError(f"YAML syntax error: {exc}")
    _require(isinstance(doc, dict), f"targets file must be a mapping, got {type(doc).__name__}")
    _require(doc.get("version") == 1, f"unsupported targets version {doc.get('version')!r}")
    unknown = set(doc) - {"version", "defaults", "targets"}
    _require(not unknown, f"unknown top-level keys {sorted(unknown)}")

    defaults = doc.get("defaults") or {}
    _require(isinstance(defaults, dict), "defaults must be a mapping")
    unknown = set(defaults) - _DEFAULTS_KEYS
    _require(not unknown, f"unknown defaults keys {sorted(unknown)}")
    defaults = {"idle_interval": _interval(defaults, "idle_interval", None)}

    entries = doc.get("targets")
    _require(isinstance(entries, list), "targets must be a list (use [] for none)")
    targets = []
    for entry in entries:
        _require(isinstance(entry, dict), f"each target must be a mapping, got {entry!r}")
        kind = entry.get("kind")
        _require(kind == "scoreboard", f"unknown target kind {kind!r} in {entry.get('name')!r}")
        targets.append(_parse_scoreboard(entry, defaults))

    names = [t.name for t in targets]
    dupes = {n for n in names if names.count(n) > 1}
    _require(not dupes, f"duplicate target names {sorted(dupes)}")
    return TargetsFile(targets=tuple(targets))


def load_targets(path: Path) -> TargetsFile:
    return parse_targets(path.read_text(encoding="utf-8"))

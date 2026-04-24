"""
MLB live-game data model and JSON deserialization.

Mirrors the backend's outbound types in `backend/src/mlb.rs`. The shapes are
intentionally identical so the wire protocol is a direct structural match.
Field names are snake_case (Rust serde default).

Model classes follow the HUB75 library's value-type convention: plain classes
with underscore-prefixed private fields and `@property` getters. This matches
`Pi-Pico-Hub75-Driver/src/lib/hub75/row_addressing.py` and
`Pi-Pico-Hub75-Driver/src/lib/hub75/gamma.py`.
"""

import json

from .inning_half import Top, Middle, Bottom, End


class DeserializeError(Exception):
    """
    Raised when a JSON payload doesn't match the expected MLB shape.

    Attributes:
        path: JSON path of the offending field (e.g. "$.home.score").
        message: Human-readable description of the mismatch.
    """

    def __init__(self, path: str, message: str) -> None:
        self.path: str = path
        self.message: str = message
        super().__init__(f"{path}: {message}")


def _req_str(d: dict[str, object], key: str, parent_path: str) -> str:
    if key not in d:
        raise DeserializeError(parent_path + "." + key, "missing required field")
    v = d[key]
    if not isinstance(v, str):
        raise DeserializeError(
            parent_path + "." + key,
            f"expected string, got {type(v).__name__}",
        )
    return v


def _req_int(d: dict[str, object], key: str, parent_path: str) -> int:
    if key not in d:
        raise DeserializeError(parent_path + "." + key, "missing required field")
    v = d[key]
    # Explicitly reject bool since `isinstance(True, int)` is True in Python.
    if type(v) is bool:
        raise DeserializeError(
            parent_path + "." + key,
            f"expected int, got {type(v).__name__}",
        )
    if not isinstance(v, int):
        raise DeserializeError(
            parent_path + "." + key,
            f"expected int, got {type(v).__name__}",
        )
    return v


def _req_bool(d: dict[str, object], key: str, parent_path: str) -> bool:
    if key not in d:
        raise DeserializeError(parent_path + "." + key, "missing required field")
    v = d[key]
    if not isinstance(v, bool):
        raise DeserializeError(
            parent_path + "." + key,
            f"expected bool, got {type(v).__name__}",
        )
    return v


def _req_dict(
    d: dict[str, object], key: str, parent_path: str
) -> dict[str, object]:
    if key not in d:
        raise DeserializeError(parent_path + "." + key, "missing required field")
    v = d[key]
    if not isinstance(v, dict):
        raise DeserializeError(
            parent_path + "." + key,
            f"expected object, got {type(v).__name__}",
        )
    return v  # type: ignore[return-value]


class TeamColors:
    """Primary / alternate team colors as packed RGB integers."""

    def __init__(self, primary: int, alternate: int) -> None:
        self._primary = primary
        self._alternate = alternate

    @property
    def primary(self) -> int:
        return self._primary

    @property
    def alternate(self) -> int:
        return self._alternate

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "TeamColors":
        return cls(
            primary=_req_int(d, "primary", path),
            alternate=_req_int(d, "alternate", path),
        )


class TeamState:
    """One team's snapshot: abbreviation, current score, and colors."""

    def __init__(self, abbreviation: str, score: int, colors: TeamColors) -> None:
        self._abbreviation = abbreviation
        self._score = score
        self._colors = colors

    @property
    def abbreviation(self) -> str:
        return self._abbreviation

    @property
    def score(self) -> int:
        return self._score

    @property
    def colors(self) -> TeamColors:
        return self._colors

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "TeamState":
        return cls(
            abbreviation=_req_str(d, "abbreviation", path),
            score=_req_int(d, "score", path),
            colors=TeamColors.from_dict(
                _req_dict(d, "colors", path), path + ".colors"
            ),
        )


class Count:
    """Ball/strike/out count for the current at-bat."""

    def __init__(self, balls: int, strikes: int, outs: int) -> None:
        self._balls = balls
        self._strikes = strikes
        self._outs = outs

    @property
    def balls(self) -> int:
        return self._balls

    @property
    def strikes(self) -> int:
        return self._strikes

    @property
    def outs(self) -> int:
        return self._outs

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "Count":
        return cls(
            balls=_req_int(d, "balls", path),
            strikes=_req_int(d, "strikes", path),
            outs=_req_int(d, "outs", path),
        )


class Bases:
    """Occupancy of the three bases."""

    def __init__(self, first: bool, second: bool, third: bool) -> None:
        self._first = first
        self._second = second
        self._third = third

    @property
    def first(self) -> bool:
        return self._first

    @property
    def second(self) -> bool:
        return self._second

    @property
    def third(self) -> bool:
        return self._third

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "Bases":
        return cls(
            first=_req_bool(d, "first", path),
            second=_req_bool(d, "second", path),
            third=_req_bool(d, "third", path),
        )


class AtBat:
    """Current pitcher / batter matchup."""

    def __init__(self, pitcher: str, batter: str) -> None:
        self._pitcher = pitcher
        self._batter = batter

    @property
    def pitcher(self) -> str:
        return self._pitcher

    @property
    def batter(self) -> str:
        return self._batter

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "AtBat":
        return cls(
            pitcher=_req_str(d, "pitcher", path),
            batter=_req_str(d, "batter", path),
        )


class LastPlay:
    """Most recent play's ESPN id and human-readable description."""

    def __init__(self, id: str, text: str) -> None:
        self._id = id
        self._text = text

    @property
    def id(self) -> str:
        return self._id

    @property
    def text(self) -> str:
        return self._text

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "LastPlay":
        return cls(
            id=_req_str(d, "id", path),
            text=_req_str(d, "text", path),
        )


class Inning:
    """Current inning number and half."""

    _HALF_MAP: dict[str, type] = {
        "top": Top,
        "middle": Middle,
        "bottom": Bottom,
        "end": End,
    }

    def __init__(self, number: int, half: Top | Middle | Bottom | End) -> None:
        self._number = number
        self._half = half

    @property
    def number(self) -> int:
        return self._number

    @property
    def half(self) -> Top | Middle | Bottom | End:
        return self._half

    @classmethod
    def from_dict(cls, d: dict[str, object], path: str) -> "Inning":
        number = _req_int(d, "number", path)
        half_str = _req_str(d, "half", path)
        half_cls = cls._HALF_MAP.get(half_str)
        if half_cls is None:
            raise DeserializeError(
                path + ".half",
                f"expected one of {list(cls._HALF_MAP)}, got '{half_str}'",
            )
        return cls(number, half_cls())


class LiveGame:
    """Top-level live MLB game snapshot returned by `/mlb/games/{id}`."""

    def __init__(
        self,
        game_id: str,
        inning: Inning,
        home: TeamState,
        away: TeamState,
        count: Count,
        bases: Bases,
        at_bat: AtBat | None,
        last_play: LastPlay,
    ) -> None:
        self._game_id = game_id
        self._inning = inning
        self._home = home
        self._away = away
        self._count = count
        self._bases = bases
        self._at_bat = at_bat
        self._last_play = last_play

    @property
    def game_id(self) -> str:
        return self._game_id

    @property
    def inning(self) -> Inning:
        return self._inning

    @property
    def home(self) -> TeamState:
        return self._home

    @property
    def away(self) -> TeamState:
        return self._away

    @property
    def count(self) -> Count:
        return self._count

    @property
    def bases(self) -> Bases:
        return self._bases

    @property
    def at_bat(self) -> AtBat | None:
        return self._at_bat

    @property
    def last_play(self) -> LastPlay:
        return self._last_play

    @classmethod
    def from_json(cls, body: bytes | str) -> "LiveGame":
        try:
            raw = json.loads(body)
        except ValueError as e:
            raise DeserializeError("$", f"invalid JSON: {e}")
        if not isinstance(raw, dict):
            raise DeserializeError(
                "$", f"expected object, got {type(raw).__name__}"
            )
        return cls.from_dict(raw)  # type: ignore[arg-type]

    @classmethod
    def from_dict(cls, d: dict[str, object]) -> "LiveGame":
        path = "$"
        at_bat_raw = d.get("at_bat")
        at_bat: AtBat | None
        if at_bat_raw is None:
            at_bat = None
        elif isinstance(at_bat_raw, dict):
            at_bat = AtBat.from_dict(
                at_bat_raw, path + ".at_bat"  # type: ignore[arg-type]
            )
        else:
            raise DeserializeError(
                path + ".at_bat",
                f"expected object or null, got {type(at_bat_raw).__name__}",
            )
        return cls(
            game_id=_req_str(d, "game_id", path),
            inning=Inning.from_dict(
                _req_dict(d, "inning", path), path + ".inning"
            ),
            home=TeamState.from_dict(_req_dict(d, "home", path), path + ".home"),
            away=TeamState.from_dict(_req_dict(d, "away", path), path + ".away"),
            count=Count.from_dict(_req_dict(d, "count", path), path + ".count"),
            bases=Bases.from_dict(_req_dict(d, "bases", path), path + ".bases"),
            at_bat=at_bat,
            last_play=LastPlay.from_dict(
                _req_dict(d, "last_play", path), path + ".last_play"
            ),
        )

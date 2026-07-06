"""
MLB live-game data model and binary wire-format deserialization.

The backend serves game state as a fixed-layout packed struct (negotiated
via `Accept: application/x-scoreboard-struct`). The NORMATIVE spec lives in
`backend/src/wire.rs`; this module must parse exactly that layout. Version
mismatches fail loudly (a stray JSON body starts with '{' or '[' and fails
the version check immediately).

Parsing allocates only the model objects and their strings — no intermediate
dict tree, no json module. The fixed numeric section is decoded in a single
C-level `struct.unpack_from`; the strings section is a bounds-checked
length-prefixed walk over the response memoryview.

Model classes are plain-attribute value types: instances are only ever built
by `from_struct` and are treated as immutable after construction. The display
thread reads these fields at 20 FPS, so attribute access stays on
MicroPython's fast path — no property descriptors.
"""

import struct

from .inning_half import Top, Middle, Bottom, End, TOP, MIDDLE, BOTTOM, END

# Must match backend/src/wire.rs.
WIRE_VERSION = 1
STRUCT_CONTENT_TYPE = "application/x-scoreboard-struct"

# Fixed section: version, flags, inning_number, inning_half, balls, strikes,
# outs, bases_bitfield, away_score, home_score, then away/home color pairs.
_FIXED_FMT = "<BBBBBBBBHHIIII"
_FIXED_SIZE = struct.calcsize(_FIXED_FMT)  # 28

_FLAG_AT_BAT = 0x01
_BASE_FIRST = 0x01
_BASE_SECOND = 0x02
_BASE_THIRD = 0x04

# Wire code (index) -> inning-half singleton.
_HALVES = (TOP, MIDDLE, BOTTOM, END)


class DeserializeError(Exception):
    """
    Raised when a payload doesn't match the expected wire format.

    Attributes:
        path: byte-offset context of the failure (e.g. "@28").
        message: Human-readable description of the mismatch.
    """

    def __init__(self, path: str, message: str) -> None:
        self.path: str = path
        self.message: str = message
        super().__init__(f"{path}: {message}")


def _check_version(buf, end: int) -> None:
    if end < 1:
        raise DeserializeError("@0", "empty payload")
    version = buf[0]
    if version != WIRE_VERSION:
        raise DeserializeError(
            "@0", f"unsupported wire version {version} (expected {WIRE_VERSION})"
        )


def _read_str(buf, offset: int, end: int, what: str) -> tuple[str, int]:
    """Read one u8-length-prefixed UTF-8 string. Returns (text, next_offset)."""
    if offset >= end:
        raise DeserializeError(f"@{offset}", f"truncated before {what} length")
    n = buf[offset]
    offset += 1
    if offset + n > end:
        raise DeserializeError(
            f"@{offset}", f"truncated inside {what}: need {n} bytes, have {end - offset}"
        )
    return str(buf[offset:offset + n], "utf-8"), offset + n


class TeamColors:
    """Primary / alternate team colors as packed RGB integers."""

    def __init__(self, primary: int, alternate: int) -> None:
        self.primary = primary
        self.alternate = alternate


class TeamState:
    """One team's snapshot: abbreviation, current score, and colors."""

    def __init__(self, abbreviation: str, score: int, colors: TeamColors) -> None:
        self.abbreviation = abbreviation
        self.score = score
        self.colors = colors


class Count:
    """Ball/strike/out count for the current at-bat."""

    def __init__(self, balls: int, strikes: int, outs: int) -> None:
        self.balls = balls
        self.strikes = strikes
        self.outs = outs


class Bases:
    """Occupancy of the three bases."""

    def __init__(self, first: bool, second: bool, third: bool) -> None:
        self.first = first
        self.second = second
        self.third = third


class AtBat:
    """Current pitcher / batter matchup."""

    def __init__(self, pitcher: str, batter: str) -> None:
        self.pitcher = pitcher
        self.batter = batter


class LastPlay:
    """Most recent play's ESPN id and human-readable description."""

    def __init__(self, id: str, text: str) -> None:
        self.id = id
        self.text = text


class Inning:
    """Current inning number and half."""

    def __init__(self, number: int, half: Top | Middle | Bottom | End) -> None:
        self.number = number
        self.half = half


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
        self.game_id = game_id
        self.inning = inning
        self.home = home
        self.away = away
        self.count = count
        self.bases = bases
        self.at_bat = at_bat
        self.last_play = last_play

    @classmethod
    def from_struct(cls, buf) -> "LiveGame":
        """Parse a v1 LiveGame payload (see backend/src/wire.rs for the spec)."""
        end = len(buf)
        _check_version(buf, end)
        if end < _FIXED_SIZE:
            raise DeserializeError(
                "@0", f"truncated fixed section: {end} < {_FIXED_SIZE}"
            )

        (
            _version, flags, inning_number, half_code,
            balls, strikes, outs, bases_bits,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_FIXED_FMT, buf, 0)

        if half_code >= len(_HALVES):
            raise DeserializeError("@3", f"invalid inning half code: {half_code}")

        o = _FIXED_SIZE
        game_id, o = _read_str(buf, o, end, "game_id")
        away_abbr, o = _read_str(buf, o, end, "away abbreviation")
        home_abbr, o = _read_str(buf, o, end, "home abbreviation")

        at_bat: AtBat | None = None
        if flags & _FLAG_AT_BAT:
            pitcher, o = _read_str(buf, o, end, "pitcher")
            batter, o = _read_str(buf, o, end, "batter")
            at_bat = AtBat(pitcher, batter)

        play_id, o = _read_str(buf, o, end, "last play id")
        play_text, o = _read_str(buf, o, end, "last play text")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        return cls(
            game_id=game_id,
            inning=Inning(inning_number, _HALVES[half_code]),
            home=TeamState(home_abbr, home_score, TeamColors(home_primary, home_alternate)),
            away=TeamState(away_abbr, away_score, TeamColors(away_primary, away_alternate)),
            count=Count(balls, strikes, outs),
            bases=Bases(
                bool(bases_bits & _BASE_FIRST),
                bool(bases_bits & _BASE_SECOND),
                bool(bases_bits & _BASE_THIRD),
            ),
            at_bat=at_bat,
            last_play=LastPlay(play_id, play_text),
        )


def parse_game_ids(buf) -> list[str]:
    """Parse a v1 game-id-list payload (see backend/src/wire.rs for the spec)."""
    end = len(buf)
    _check_version(buf, end)
    if end < 2:
        raise DeserializeError("@1", "truncated before id count")
    count = buf[1]

    ids: list[str] = []
    o = 2
    for _ in range(count):
        game_id, o = _read_str(buf, o, end, "game id")
        ids.append(game_id)

    if o != end:
        raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")
    return ids

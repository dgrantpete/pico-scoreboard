"""
MLB game data model and binary wire-format deserialization.

The backend serves game state as a fixed-layout packed struct (negotiated
via `Accept: application/x-scoreboard-struct`). The NORMATIVE spec lives in
`backend/src/wire.rs`; this module must parse exactly that layout. Version
mismatches fail loudly (a stray JSON body starts with '{' or '[' and fails
the version check immediately).

Wire v2 carries three game states behind a common 2-byte header
(byte0 = version, byte1 = state). The list endpoint reuses byte1 as a count
and tags each entry with its own state byte. Parsing allocates only the
model objects and their strings — no intermediate dict tree, no json module.
Each fixed numeric section is decoded in a single C-level
`struct.unpack_from`; the strings section is a bounds-checked
length-prefixed walk over the response memoryview.

Model classes are plain-attribute value types: instances are only ever built
by `from_struct` and are treated as immutable after construction. The display
thread reads these fields at 20 FPS, so attribute access stays on
MicroPython's fast path — no property descriptors.
"""

import struct

from .inning_half import Top, Middle, Bottom, End, TOP, MIDDLE, BOTTOM, END

# Must match backend/src/wire.rs.
WIRE_VERSION = 2
STRUCT_CONTENT_TYPE = "application/x-scoreboard-struct"

# Common detail header: byte0 = version, byte1 = state. State codes are also
# the per-entry tags in the game list and the ETag tokens.
_HDR_SIZE = 2
GAME_STATE_PRE = 0
GAME_STATE_IN = 1
GAME_STATE_POST = 2

# Live fixed section (offset 2): flags, inning_number, inning_half, balls,
# strikes, outs, bases_bitfield, away_score u16, home_score u16, then
# away/home color pairs u32.
_LIVE_FMT = "<BBBBBBBHHIIII"
_LIVE_SIZE = struct.calcsize(_LIVE_FMT)  # 27

# Pregame fixed section (offset 2): flags, temperature u8, away wins/losses
# u16, home wins/losses u16, start_time u32, then away/home color pairs u32.
_PRE_FMT = "<BBHHHHIIIII"
_PRE_SIZE = struct.calcsize(_PRE_FMT)  # 30

# Final fixed section (offset 2): innings_played, away linescore len,
# home linescore len, away_score u16, home_score u16, then away/home color
# pairs u32.
_FINAL_FMT = "<BBBHHIIII"
_FINAL_SIZE = struct.calcsize(_FINAL_FMT)  # 23

_FLAG_AT_BAT = 0x01

_PRE_FLAG_WEATHER = 0x01
_PRE_FLAG_AWAY_RECORD = 0x02
_PRE_FLAG_HOME_RECORD = 0x04
_PRE_FLAG_AWAY_PROBABLE = 0x08
_PRE_FLAG_HOME_PROBABLE = 0x10

_BASE_FIRST = 0x01
_BASE_SECOND = 0x02
_BASE_THIRD = 0x04

# Wire code (index) -> inning-half singleton.
_HALVES = (TOP, MIDDLE, BOTTOM, END)


class DeserializeError(Exception):
    """
    Raised when a payload doesn't match the expected wire format.

    Attributes:
        path: byte-offset context of the failure (e.g. "@29").
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


class PregameTeam:
    """One team's pregame snapshot: abbreviation, colors, record, probable.

    `wins`/`losses` are None when the backend didn't advertise a record for
    this side (record flag off); `pitcher` is None when no probable was
    advertised. Absent numeric fields arrive on the wire as 0 and are surfaced
    as None here so the display never renders a fake 0-0 record.
    """

    def __init__(
        self,
        abbreviation: str,
        colors: TeamColors,
        wins: int | None,
        losses: int | None,
        pitcher: str | None,
    ) -> None:
        self.abbreviation = abbreviation
        self.colors = colors
        self.wins = wins
        self.losses = losses
        self.pitcher = pitcher


class FinalTeam:
    """One team's final snapshot: abbreviation, colors, total score, line score.

    `line` is per-inning runs (inning 1 first), copied out of the response
    buffer with `bytes(...)` so it survives the next request.
    """

    def __init__(
        self,
        abbreviation: str,
        colors: TeamColors,
        score: int,
        line: bytes,
    ) -> None:
        self.abbreviation = abbreviation
        self.colors = colors
        self.score = score
        self.line = line


class LiveGame:
    """Top-level live MLB game snapshot returned by `/baseball/mlb/games/{id}`."""

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
        """Parse a v2 live payload (see backend/src/wire.rs for the spec)."""
        end = len(buf)
        _check_version(buf, end)
        if end < _HDR_SIZE + _LIVE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {_HDR_SIZE + _LIVE_SIZE}"
            )

        (
            flags, inning_number, half_code,
            balls, strikes, outs, bases_bits,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_LIVE_FMT, buf, _HDR_SIZE)

        if half_code >= len(_HALVES):
            raise DeserializeError("@4", f"invalid inning half code: {half_code}")

        o = _HDR_SIZE + _LIVE_SIZE  # 29
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


class PregameGame:
    """Top-level pregame MLB snapshot returned by `/baseball/mlb/games/{id}`."""

    def __init__(
        self,
        game_id: str,
        start_epoch: int,
        venue: str,
        weather_temp: int | None,
        weather_condition: str | None,
        home: PregameTeam,
        away: PregameTeam,
    ) -> None:
        self.game_id = game_id
        self.start_epoch = start_epoch
        self.venue = venue
        self.weather_temp = weather_temp
        self.weather_condition = weather_condition
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf) -> "PregameGame":
        """Parse a v2 pregame payload (see backend/src/wire.rs for the spec)."""
        end = len(buf)
        _check_version(buf, end)
        if end < _HDR_SIZE + _PRE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {_HDR_SIZE + _PRE_SIZE}"
            )

        (
            flags, temperature,
            away_wins, away_losses, home_wins, home_losses,
            start_time,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_PRE_FMT, buf, _HDR_SIZE)

        o = _HDR_SIZE + _PRE_SIZE  # 32
        game_id, o = _read_str(buf, o, end, "game_id")
        away_abbr, o = _read_str(buf, o, end, "away abbreviation")
        home_abbr, o = _read_str(buf, o, end, "home abbreviation")
        venue, o = _read_str(buf, o, end, "venue")

        weather_temp: int | None = None
        weather_condition: str | None = None
        if flags & _PRE_FLAG_WEATHER:
            weather_condition, o = _read_str(buf, o, end, "weather condition")
            weather_temp = temperature

        away_pitcher: str | None = None
        if flags & _PRE_FLAG_AWAY_PROBABLE:
            away_pitcher, o = _read_str(buf, o, end, "away probable pitcher")
        home_pitcher: str | None = None
        if flags & _PRE_FLAG_HOME_PROBABLE:
            home_pitcher, o = _read_str(buf, o, end, "home probable pitcher")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        away_has_record = bool(flags & _PRE_FLAG_AWAY_RECORD)
        home_has_record = bool(flags & _PRE_FLAG_HOME_RECORD)

        return cls(
            game_id=game_id,
            start_epoch=start_time,
            venue=venue,
            weather_temp=weather_temp,
            weather_condition=weather_condition,
            away=PregameTeam(
                away_abbr,
                TeamColors(away_primary, away_alternate),
                away_wins if away_has_record else None,
                away_losses if away_has_record else None,
                away_pitcher,
            ),
            home=PregameTeam(
                home_abbr,
                TeamColors(home_primary, home_alternate),
                home_wins if home_has_record else None,
                home_losses if home_has_record else None,
                home_pitcher,
            ),
        )


class FinalGame:
    """Top-level final MLB snapshot returned by `/baseball/mlb/games/{id}`."""

    def __init__(
        self,
        game_id: str,
        innings_played: int,
        home: FinalTeam,
        away: FinalTeam,
    ) -> None:
        self.game_id = game_id
        self.innings_played = innings_played
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf) -> "FinalGame":
        """Parse a v2 final payload (see backend/src/wire.rs for the spec)."""
        end = len(buf)
        _check_version(buf, end)
        if end < _HDR_SIZE + _FINAL_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {_HDR_SIZE + _FINAL_SIZE}"
            )

        (
            innings_played, away_len, home_len,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_FINAL_FMT, buf, _HDR_SIZE)

        o = _HDR_SIZE + _FINAL_SIZE  # 25
        if o + away_len + home_len > end:
            raise DeserializeError(
                f"@{o}",
                f"truncated linescores: need {away_len + home_len} bytes, "
                f"have {end - o}",
            )
        # Copy out of the shared response buffer: the source memoryview aliases
        # the client's reusable _response_buf and is clobbered by the next
        # request.
        away_line = bytes(buf[o:o + away_len])
        o += away_len
        home_line = bytes(buf[o:o + home_len])
        o += home_len

        game_id, o = _read_str(buf, o, end, "game_id")
        away_abbr, o = _read_str(buf, o, end, "away abbreviation")
        home_abbr, o = _read_str(buf, o, end, "home abbreviation")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        return cls(
            game_id=game_id,
            innings_played=innings_played,
            home=FinalTeam(
                home_abbr, TeamColors(home_primary, home_alternate), home_score, home_line
            ),
            away=FinalTeam(
                away_abbr, TeamColors(away_primary, away_alternate), away_score, away_line
            ),
        )


def parse_game_detail(buf) -> "LiveGame | PregameGame | FinalGame":
    """Parse a v2 detail payload, dispatching on the state header byte."""
    end = len(buf)
    _check_version(buf, end)
    if end < _HDR_SIZE:
        raise DeserializeError("@1", "truncated before state byte")
    state = buf[1]
    if state == GAME_STATE_IN:
        return LiveGame.from_struct(buf)
    if state == GAME_STATE_PRE:
        return PregameGame.from_struct(buf)
    if state == GAME_STATE_POST:
        return FinalGame.from_struct(buf)
    raise DeserializeError("@1", f"unknown game state {state}")


def parse_game_list(buf) -> list[tuple[int, str]]:
    """Parse a v2 game-list payload (see backend/src/wire.rs for the spec).

    Returns (state, id) pairs in backend (chronological) order. `state` is one
    of GAME_STATE_PRE / GAME_STATE_IN / GAME_STATE_POST.
    """
    end = len(buf)
    _check_version(buf, end)
    if end < 2:
        raise DeserializeError("@1", "truncated before game count")
    count = buf[1]

    games: list[tuple[int, str]] = []
    o = 2
    for _ in range(count):
        if o >= end:
            raise DeserializeError(f"@{o}", "truncated before game state")
        state = buf[o]
        o += 1
        if state > GAME_STATE_POST:
            raise DeserializeError(f"@{o - 1}", f"invalid game state {state}")
        game_id, o = _read_str(buf, o, end, "game id")
        games.append((state, game_id))

    if o != end:
        raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")
    return games

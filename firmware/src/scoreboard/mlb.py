"""
MLB game data model and binary wire-format deserialization.

Firmware mirror of the backend's `MlbGame` domain model and its wire
encoding — the NORMATIVE spec is the MLB section of `backend/src/wire.rs`'s
doc comment; `tools/wire_format_check.py` cross-checks this parser against
the Rust encoder's golden bytes. Parsing allocates only the model objects
and their strings — no intermediate dict tree, no json module. Each fixed
numeric section is decoded in a single C-level `struct.unpack_from`; the
strings section is a bounds-checked length-prefixed walk over the response
memoryview.

Shared value types and wire primitives (header/state codes, version check,
string reader, the sport-agnostic game-list parser) live in scoreboard.wire
— same contract as scoreboard/soccer.py and scoreboard/nba.py. Model classes
are plain-attribute value types: instances are only ever built by
`from_struct` and are treated as immutable after construction. The display
thread reads these fields at 20 FPS, so attribute access stays on
MicroPython's fast path — no property descriptors.
"""

import struct

from .inning_half import Top, Middle, Bottom, End, TOP, MIDDLE, BOTTOM, END
from .wire import (
    DeserializeError,
    GAME_STATE_IN,
    GAME_STATE_POST,
    GAME_STATE_PRE,
    HDR_SIZE,
    LastPlay,
    PregameTeam,
    TeamColors,
    TeamState,
    check_version,
    dispatch_detail,
    read_str,
)

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


class Inning:
    """Current inning number and half."""

    def __init__(self, number: int, half: Top | Middle | Bottom | End) -> None:
        self.number = number
        self.half = half


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

    wire_state = GAME_STATE_IN

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
        check_version(buf, end)
        if end < HDR_SIZE + _LIVE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _LIVE_SIZE}"
            )

        (
            flags, inning_number, half_code,
            balls, strikes, outs, bases_bits,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_LIVE_FMT, buf, HDR_SIZE)

        if half_code >= len(_HALVES):
            raise DeserializeError("@4", f"invalid inning half code: {half_code}")

        o = HDR_SIZE + _LIVE_SIZE  # 29
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")

        at_bat: AtBat | None = None
        if flags & _FLAG_AT_BAT:
            pitcher, o = read_str(buf, o, end, "pitcher")
            batter, o = read_str(buf, o, end, "batter")
            at_bat = AtBat(pitcher, batter)

        play_id, o = read_str(buf, o, end, "last play id")
        play_text, o = read_str(buf, o, end, "last play text")

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

    wire_state = GAME_STATE_PRE

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
        check_version(buf, end)
        if end < HDR_SIZE + _PRE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _PRE_SIZE}"
            )

        (
            flags, temperature,
            away_wins, away_losses, home_wins, home_losses,
            start_time,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_PRE_FMT, buf, HDR_SIZE)

        o = HDR_SIZE + _PRE_SIZE  # 32
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")
        venue, o = read_str(buf, o, end, "venue")

        weather_temp: int | None = None
        weather_condition: str | None = None
        if flags & _PRE_FLAG_WEATHER:
            weather_condition, o = read_str(buf, o, end, "weather condition")
            weather_temp = temperature

        away_pitcher: str | None = None
        if flags & _PRE_FLAG_AWAY_PROBABLE:
            away_pitcher, o = read_str(buf, o, end, "away probable pitcher")
        home_pitcher: str | None = None
        if flags & _PRE_FLAG_HOME_PROBABLE:
            home_pitcher, o = read_str(buf, o, end, "home probable pitcher")

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

    wire_state = GAME_STATE_POST

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
        check_version(buf, end)
        if end < HDR_SIZE + _FINAL_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _FINAL_SIZE}"
            )

        (
            innings_played, away_len, home_len,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_FINAL_FMT, buf, HDR_SIZE)

        o = HDR_SIZE + _FINAL_SIZE  # 25
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

        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")

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
    """Parse a v2 MLB detail payload, dispatching on the state header byte."""
    return dispatch_detail(buf, LiveGame, PregameGame, FinalGame)


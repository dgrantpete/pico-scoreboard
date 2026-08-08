"""
NBA game data model and binary wire-format deserialization.

Firmware mirror of the backend's `NbaGame` domain model
(`backend/src/nba/types.rs`) and its wire encoding — the NORMATIVE spec is
the `scoreboard_wire::nba` module of `crates/scoreboard-wire`.
Model classes are plain-attribute value types,
treated as immutable after construction, read by the display thread at
20 FPS — same contract as scoreboard/mlb.py.

Unlike soccer, the game clock travels as ESPN's display string ("10:08",
"53.0" under a minute): a basketball stop-clock cannot be extrapolated
between polls (there is no clock-running signal), so the display re-renders
the string each poll and `phase` says when it is meaningless (breaks).

Shared value types and wire primitives come from scoreboard.wire — same
contract as scoreboard/mlb.py and scoreboard/soccer.py.
"""

import struct

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

# Live fixed section (offset 2): flags, period, phase, away/home score u16,
# then away/home color pairs u32.
_LIVE_FMT = "<BBBHHIIII"
_LIVE_SIZE = struct.calcsize(_LIVE_FMT)  # 23

# Pregame fixed section (offset 2): flags, away wins/losses u16, home
# wins/losses u16, start_time u32, then color pairs.
_PRE_FMT = "<BHHHHIIIII"
_PRE_SIZE = struct.calcsize(_PRE_FMT)  # 29

# Final fixed section (offset 2): periods_played, away linescore len, home
# linescore len, away/home score u16, then color pairs (same layout as the
# MLB final, quarters in place of innings).
_FINAL_FMT = "<BBBHHIIII"
_FINAL_SIZE = struct.calcsize(_FINAL_FMT)  # 23

_FLAG_LAST_PLAY = 0x01

_PRE_FLAG_AWAY_RECORD = 0x01
_PRE_FLAG_HOME_RECORD = 0x02

# Live phase codes (wire byte at offset 4): breaks render without a clock.
PHASE_IN_PROGRESS = 0
PHASE_HALFTIME = 1
PHASE_END_OF_PERIOD = 2


def period_name(period: int) -> str:
    """Display name of a period: Q1-Q4, then OT / 2OT / ..."""
    if period <= 4:
        return "Q" + str(period)
    if period == 5:
        return "OT"
    return str(period - 4) + "OT"


class LiveGame:
    """Top-level live NBA snapshot (`/basketball/nba/games/{id}`).

    `clock` is ESPN's display string, exact at fetch time and never
    extrapolated; `phase` is a PHASE_* code (the clock reads "0.0" or a
    reset "12:00" during breaks, so the phase is the only render signal).
    """

    wire_state = GAME_STATE_IN

    def __init__(
        self,
        game_id: str,
        period: int,
        clock: str,
        phase: int,
        home: TeamState,
        away: TeamState,
        last_play: LastPlay | None,
    ) -> None:
        self.game_id = game_id
        self.period = period
        self.clock = clock
        self.phase = phase
        self.home = home
        self.away = away
        self.last_play = last_play

    @classmethod
    def from_struct(cls, buf) -> "LiveGame":
        """Parse an NBA live payload (see crates/scoreboard-wire)."""
        end = len(buf)
        check_version(buf, end)
        if end < HDR_SIZE + _LIVE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _LIVE_SIZE}"
            )

        (
            flags, period, phase,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_LIVE_FMT, buf, HDR_SIZE)

        if phase > PHASE_END_OF_PERIOD:
            raise DeserializeError("@4", f"invalid live phase code: {phase}")

        o = HDR_SIZE + _LIVE_SIZE  # 25
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")
        clock, o = read_str(buf, o, end, "clock")

        last_play: LastPlay | None = None
        if flags & _FLAG_LAST_PLAY:
            play_id, o = read_str(buf, o, end, "last play id")
            play_text, o = read_str(buf, o, end, "last play text")
            last_play = LastPlay(play_id, play_text)

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        return cls(
            game_id=game_id,
            period=period,
            clock=clock,
            phase=phase,
            home=TeamState(home_abbr, home_score, TeamColors(home_primary, home_alternate)),
            away=TeamState(away_abbr, away_score, TeamColors(away_primary, away_alternate)),
            last_play=last_play,
        )


class PregameGame:
    """Upcoming NBA game.

    Duck-typed to the contract `state.set_pregame` reads (venue /
    weather_temp / weather_condition / start_epoch / away / home with
    wins-losses-pitcher), so the NBA pregame screen reuses the whole MLB
    pregame pipeline: real venue and records, and the fields basketball
    never has (weather, probables) are permanently absent — the renderer
    already omits absent fields.
    """

    wire_state = GAME_STATE_PRE

    def __init__(self, game_id: str, start_epoch: int, venue: str,
                 home: PregameTeam, away: PregameTeam) -> None:
        self.game_id = game_id
        self.start_epoch = start_epoch
        self.venue = venue
        self.weather_temp = None
        self.weather_condition = None
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf) -> "PregameGame":
        """Parse an NBA pregame payload (see crates/scoreboard-wire)."""
        end = len(buf)
        check_version(buf, end)
        if end < HDR_SIZE + _PRE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _PRE_SIZE}"
            )

        (
            flags,
            away_wins, away_losses, home_wins, home_losses,
            start_time,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_PRE_FMT, buf, HDR_SIZE)

        o = HDR_SIZE + _PRE_SIZE  # 31
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")
        venue, o = read_str(buf, o, end, "venue")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        away_has_record = bool(flags & _PRE_FLAG_AWAY_RECORD)
        home_has_record = bool(flags & _PRE_FLAG_HOME_RECORD)

        return cls(
            game_id=game_id,
            start_epoch=start_time,
            venue=venue,
            away=PregameTeam(
                away_abbr,
                TeamColors(away_primary, away_alternate),
                away_wins if away_has_record else None,
                away_losses if away_has_record else None,
                None,
            ),
            home=PregameTeam(
                home_abbr,
                TeamColors(home_primary, home_alternate),
                home_wins if home_has_record else None,
                home_losses if home_has_record else None,
                None,
            ),
        )


class FinalTeam:
    """One team's final snapshot: abbreviation, colors, total, line score.

    `line` is per-quarter points (quarter 1 first, overtime periods after),
    copied out of the response buffer with `bytes(...)` so it survives the
    next request.
    """

    def __init__(self, abbreviation: str, colors: TeamColors, score: int,
                 line: bytes) -> None:
        self.abbreviation = abbreviation
        self.colors = colors
        self.score = score
        self.line = line


class FinalGame:
    """Top-level final NBA snapshot."""

    wire_state = GAME_STATE_POST

    def __init__(self, game_id: str, periods_played: int, home: FinalTeam,
                 away: FinalTeam) -> None:
        self.game_id = game_id
        self.periods_played = periods_played
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf) -> "FinalGame":
        """Parse an NBA final payload (see crates/scoreboard-wire)."""
        end = len(buf)
        check_version(buf, end)
        if end < HDR_SIZE + _FINAL_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _FINAL_SIZE}"
            )

        (
            periods_played, away_len, home_len,
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
        # Copy out of the shared response buffer: the source memoryview
        # aliases the client's reusable _response_buf and is clobbered by the
        # next request.
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
            periods_played=periods_played,
            home=FinalTeam(
                home_abbr, TeamColors(home_primary, home_alternate), home_score, home_line
            ),
            away=FinalTeam(
                away_abbr, TeamColors(away_primary, away_alternate), away_score, away_line
            ),
        )


def parse_game_detail(buf) -> "LiveGame | PregameGame | FinalGame":
    """Parse an NBA detail payload, dispatching on the state header byte."""
    return dispatch_detail(buf, LiveGame, PregameGame, FinalGame)

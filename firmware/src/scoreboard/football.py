"""
Football game data model and binary wire-format deserialization.

Firmware mirror of the backend's `FootballGame` domain model
(`backend/src/football/types.rs`) and its wire encoding — the NORMATIVE spec
is the `scoreboard_wire::football` module of `crates/scoreboard-wire`.
Model classes are plain-attribute value types,
treated as immutable after construction, read by the display thread at
20 FPS — same contract as scoreboard/mlb.py.

Like basketball (and unlike soccer), the game clock travels as ESPN's
display string ("10:08", "0:53" under a minute): a stop-clock cannot be
extrapolated between polls, so the display re-renders the string each poll
and `phase` says when it is meaningless (breaks).

Football is a multi-league sport (NFL, college): `parse_game_detail` threads
the polled league's display name into the pregame model exactly like soccer —
the wire deliberately doesn't carry it (the device knows which endpoint it
hit).

Shared value types (TeamColors, TeamState, PregameTeam, LastPlay) and the
wire primitives (version check, string reader, state codes) come from
scoreboard.wire — same contract as scoreboard/mlb.py and scoreboard/nba.py.
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

# Live fixed section (offset 2): flags, period, phase, down, distance,
# yard_line, away/home timeouts, away/home score u16, then away/home color
# pairs u32.
_LIVE_FMT = "<BBBBBBBBHHIIII"
_LIVE_SIZE = struct.calcsize(_LIVE_FMT)  # 28

# Pregame fixed section (offset 2): flags, away wins/losses u16, home
# wins/losses u16, start_time u32, then color pairs (byte-identical to the
# NBA pregame; football adds only two flag bits + trailing rank strings).
_PRE_FMT = "<BHHHHIIIII"
_PRE_SIZE = struct.calcsize(_PRE_FMT)  # 29

# Final fixed section (offset 2): periods_played, away linescore len, home
# linescore len, away/home score u16, then color pairs (byte-identical to the
# NBA final, quarters in place of periods).
_FINAL_FMT = "<BBBHHIIII"
_FINAL_SIZE = struct.calcsize(_FINAL_FMT)  # 23

_FLAG_LAST_PLAY = 0x01
_FLAG_SITUATION = 0x02
_FLAG_POSSESSION_HOME = 0x04
_FLAG_RED_ZONE = 0x08
_FLAG_TIMEOUTS = 0x10

_PRE_FLAG_AWAY_RECORD = 0x01
_PRE_FLAG_HOME_RECORD = 0x02
_PRE_FLAG_AWAY_RANK = 0x04
_PRE_FLAG_HOME_RANK = 0x08

# Live phase codes (wire byte at offset 4): breaks render without a clock.
PHASE_IN_PROGRESS = 0
PHASE_HALFTIME = 1
PHASE_END_OF_PERIOD = 2

# Possession side codes (SIDE_NONE when no situation is present).
SIDE_NONE = 0
SIDE_AWAY = 1
SIDE_HOME = 2

# ESPN league slug -> display name shown on the pregame screen's info line.
# Mirrors the backend's FootballLeague registry (backend/src/espn/league.rs);
# an unknown slug falls back to slug.upper() in poller.football_source.
LEAGUE_NAMES = {
    "nfl": "NFL",
    "college-football": "NCAA FOOTBALL",
}


def period_name(period: int) -> str:
    """Display name of a period: Q1-Q4, then OT / 2OT / ..."""
    if period <= 4:
        return "Q" + str(period)
    if period == 5:
        return "OT"
    return str(period - 4) + "OT"


class LiveGame:
    """Top-level live football snapshot (`/football/{league}/games/{id}`).

    `clock` is ESPN's display string, exact at fetch time and never
    extrapolated; `phase` is a PHASE_* code (the clock reads "0:00" during
    breaks, so the phase is the only render signal). The situation attributes
    are flattened: `possession` is SIDE_NONE exactly when no drive situation is
    present, in which case `down` / `distance` / `yard_line` read 0 and
    `red_zone` is False. `away_timeouts` / `home_timeouts` are None when the
    backend didn't advertise timeouts (so the display leaves the bars undrawn
    rather than render a fake three).
    """

    wire_state = GAME_STATE_IN

    def __init__(
        self,
        game_id: str,
        period: int,
        clock: str,
        phase: int,
        down: int,
        distance: int,
        yard_line: int,
        possession: int,
        red_zone: bool,
        away_timeouts: int | None,
        home_timeouts: int | None,
        home: TeamState,
        away: TeamState,
        last_play: LastPlay | None,
    ) -> None:
        self.game_id = game_id
        self.period = period
        self.clock = clock
        self.phase = phase
        self.down = down
        self.distance = distance
        self.yard_line = yard_line
        self.possession = possession
        self.red_zone = red_zone
        self.away_timeouts = away_timeouts
        self.home_timeouts = home_timeouts
        self.home = home
        self.away = away
        self.last_play = last_play

    @classmethod
    def from_struct(cls, buf) -> "LiveGame":
        """Parse a football live payload (see crates/scoreboard-wire)."""
        end = len(buf)
        check_version(buf, end)
        if end < HDR_SIZE + _LIVE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _LIVE_SIZE}"
            )

        (
            flags, period, phase, down, distance, yard_line,
            away_timeouts, home_timeouts,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_LIVE_FMT, buf, HDR_SIZE)

        if phase > PHASE_END_OF_PERIOD:
            raise DeserializeError("@4", f"invalid live phase code: {phase}")

        # Flatten the situation: absent (bit1 clear) reads as SIDE_NONE with
        # zeroed drive fields, regardless of what the fixed section carries.
        if flags & _FLAG_SITUATION:
            possession = SIDE_HOME if flags & _FLAG_POSSESSION_HOME else SIDE_AWAY
            red_zone = bool(flags & _FLAG_RED_ZONE)
        else:
            down = 0
            distance = 0
            yard_line = 0
            possession = SIDE_NONE
            red_zone = False

        # Timeouts surface as None when the backend didn't advertise them, so
        # the display never renders a fake three (same policy as records).
        if flags & _FLAG_TIMEOUTS:
            away_to: int | None = away_timeouts
            home_to: int | None = home_timeouts
        else:
            away_to = None
            home_to = None

        o = HDR_SIZE + _LIVE_SIZE  # 30
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
            down=down,
            distance=distance,
            yard_line=yard_line,
            possession=possession,
            red_zone=red_zone,
            away_timeouts=away_to,
            home_timeouts=home_to,
            home=TeamState(home_abbr, home_score, TeamColors(home_primary, home_alternate)),
            away=TeamState(away_abbr, away_score, TeamColors(away_primary, away_alternate)),
            last_play=last_play,
        )


class PregameGame:
    """Upcoming football game.

    Duck-typed to the contract `state.set_pregame` reads (venue /
    weather_temp / weather_condition / start_epoch / away / home with
    wins-losses-pitcher), so the football pregame screen reuses the whole MLB
    pregame pipeline: `venue` carries the league display name ("NFL", "NCAA
    FOOTBALL"), the stadium name rides the weather-condition slot (football
    has no weather; the cycle then reads league / venue / kickoff), records
    render like the NBA's, and the display-shaped rank line ("#3 OHIO STATE",
    college only) rides the probable-pitcher slot — it renders in team color
    via the existing PITCHER_* regions with zero shared-code changes. The
    fields football never has (weather temperature) stay absent — the renderer
    already omits absent fields.
    """

    wire_state = GAME_STATE_PRE

    def __init__(self, game_id: str, start_epoch: int, league: str,
                 venue: str, home: PregameTeam, away: PregameTeam) -> None:
        self.game_id = game_id
        self.start_epoch = start_epoch
        self.venue = league
        self.weather_temp = None
        self.weather_condition = venue or None
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf, league: str) -> "PregameGame":
        """Parse a football pregame payload (see crates/scoreboard-wire).

        `league` is the display name of the league this payload was polled
        from — the wire deliberately doesn't carry it (the device knows which
        endpoint it hit).
        """
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

        away_rank: str | None = None
        home_rank: str | None = None
        if flags & _PRE_FLAG_AWAY_RANK:
            away_rank, o = read_str(buf, o, end, "away rank line")
        if flags & _PRE_FLAG_HOME_RANK:
            home_rank, o = read_str(buf, o, end, "home rank line")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        away_has_record = bool(flags & _PRE_FLAG_AWAY_RECORD)
        home_has_record = bool(flags & _PRE_FLAG_HOME_RECORD)

        return cls(
            game_id=game_id,
            start_epoch=start_time,
            league=league,
            venue=venue,
            away=PregameTeam(
                away_abbr,
                TeamColors(away_primary, away_alternate),
                away_wins if away_has_record else None,
                away_losses if away_has_record else None,
                away_rank,
            ),
            home=PregameTeam(
                home_abbr,
                TeamColors(home_primary, home_alternate),
                home_wins if home_has_record else None,
                home_losses if home_has_record else None,
                home_rank,
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
    """Top-level final football snapshot."""

    wire_state = GAME_STATE_POST

    def __init__(self, game_id: str, periods_played: int, home: FinalTeam,
                 away: FinalTeam) -> None:
        self.game_id = game_id
        self.periods_played = periods_played
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf) -> "FinalGame":
        """Parse a football final payload (see crates/scoreboard-wire)."""
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


def parse_game_detail(buf, league: str) -> "LiveGame | PregameGame | FinalGame":
    """Parse a football detail payload, dispatching on the state header byte.

    `league` is the polled league's display name, threaded to the pregame
    model (live/final don't need it).
    """
    return dispatch_detail(buf, LiveGame, PregameGame, FinalGame, league)

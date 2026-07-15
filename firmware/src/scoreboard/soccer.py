"""
Soccer game data model and binary wire-format deserialization.

Firmware mirror of the backend's `SoccerGame` domain model
(`backend/src/soccer/types.rs`) and its wire encoding — the NORMATIVE spec
is the "Soccer game detail" section of `backend/src/wire.rs`'s doc comment;
`tools/wire_format_check.py` cross-checks this parser against the Rust
encoder's golden bytes. Model classes are plain-attribute value types,
treated as immutable after construction, read by the display thread at
20 FPS — same contract as scoreboard/mlb.py.

The match clock travels as *elapsed seconds* (floor-minute convention,
monotonic across the match: the 2nd half starts at 2700), not as ESPN's
display string — the display thread extrapolates the running clock between
polls from a ticks_ms anchor (see state.SoccerLiveView), which a string
cannot support.

Shared value types (TeamColors, TeamState, PregameTeam) and the wire
primitives (version check, string reader, state codes) come from
scoreboard.wire — same contract as scoreboard/mlb.py and scoreboard/nba.py.
"""

import struct

from .wire import (
    DeserializeError,
    GAME_STATE_IN,
    GAME_STATE_POST,
    GAME_STATE_PRE,
    HDR_SIZE,
    PregameTeam,
    TeamColors,
    TeamState,
    check_version,
    read_str,
)

# Live fixed section (offset 2): flags, half, clock_seconds u16, away/home
# score u16, then away/home color pairs u32.
_LIVE_FMT = "<BBHHHIIII"
_LIVE_SIZE = struct.calcsize(_LIVE_FMT)  # 24

# Pregame fixed section (offset 2): start_time u32, then color pairs.
_PRE_FMT = "<IIIII"
_PRE_SIZE = struct.calcsize(_PRE_FMT)  # 20

# Final fixed section (offset 2): away/home score u16, then color pairs.
_FINAL_FMT = "<HHIIII"
_FINAL_SIZE = struct.calcsize(_FINAL_FMT)  # 20

_FLAG_HALFTIME = 0x01
_FLAG_EVENT = 0x02
_FLAG_EVENT_RED = 0x04
_FLAG_EVENT_AWAY = 0x08
_FLAG_EVENT_HOME = 0x10
_FLAG_COMMENTARY = 0x20


# Last-event kinds (matches the backend's goal/red-card filter: yellow cards
# are ticker noise for a 128x64 panel).
EVENT_GOAL = 0
EVENT_RED_CARD = 1

# Event side codes.
SIDE_NONE = 0
SIDE_AWAY = 1
SIDE_HOME = 2

# Stoppage threshold (minutes) per period: regulation halves end at 45/90,
# extra-time periods at 105/120. Index by min(period, 4).
_BASE_MINUTES = (45, 45, 90, 105, 120)

# ESPN league slug -> display name shown on the pregame screen's info line.
# Mirrors the backend's SoccerLeague registry (backend/src/espn/league.rs);
# an unknown slug falls back to slug.upper() in poller.soccer_source.
LEAGUE_NAMES = {
    "usa.1": "MLS",
    "eng.1": "PREMIER LEAGUE",
    "mex.1": "LIGA MX",
    "fifa.world": "WORLD CUP",
}


def base_minutes(half: int) -> int:
    """The current period's stoppage threshold in minutes."""
    return _BASE_MINUTES[half if half < 4 else 4]


class LastEvent:
    """The most recent goal or red card.

    `clock_text` is display-shaped (e.g. "90'+3'", straight from ESPN);
    `name` is the athlete's short name ("R. Lukaku") or '' when ESPN omits
    athletes; `side` is a SIDE_* code.
    """

    def __init__(self, kind: int, clock_text: str, name: str, side: int) -> None:
        self.kind = kind
        self.clock_text = clock_text
        self.name = name
        self.side = side


class LiveGame:
    """Top-level live soccer snapshot (`/soccer/{league}/games/{id}`).

    `clock_seconds` is elapsed match seconds at fetch time; `halftime` is the
    backend's explicit interval flag (the clock alone cannot distinguish
    halftime from first-half stoppage).
    """

    wire_state = GAME_STATE_IN

    def __init__(
        self,
        game_id: str,
        clock_seconds: int,
        half: int,
        halftime: bool,
        home: TeamState,
        away: TeamState,
        last_event: LastEvent | None,
        comment_id: str = '',
        comment_text: str = '',
    ) -> None:
        self.game_id = game_id
        self.clock_seconds = clock_seconds
        self.half = half
        self.halftime = halftime
        self.home = home
        self.away = away
        self.last_event = last_event
        # Latest play-by-play commentary line (ESPN summary feed); id is the
        # change-detection key ('' = no commentary). Flashed by the poller
        # like MLB's last play.
        self.comment_id = comment_id
        self.comment_text = comment_text

    @classmethod
    def from_struct(cls, buf) -> "LiveGame":
        """Parse a soccer live payload (see backend/src/wire.rs)."""
        end = len(buf)
        check_version(buf, end)
        if end < HDR_SIZE + _LIVE_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _LIVE_SIZE}"
            )

        (
            flags, half, clock_seconds,
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_LIVE_FMT, buf, HDR_SIZE)

        o = HDR_SIZE + _LIVE_SIZE  # 26
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")

        last_event: LastEvent | None = None
        if flags & _FLAG_EVENT:
            event_clock, o = read_str(buf, o, end, "event clock")
            event_name, o = read_str(buf, o, end, "event athlete")
            if flags & _FLAG_EVENT_AWAY:
                side = SIDE_AWAY
            elif flags & _FLAG_EVENT_HOME:
                side = SIDE_HOME
            else:
                side = SIDE_NONE
            kind = EVENT_RED_CARD if flags & _FLAG_EVENT_RED else EVENT_GOAL
            last_event = LastEvent(kind, event_clock, event_name, side)

        comment_id = ''
        comment_text = ''
        if flags & _FLAG_COMMENTARY:
            comment_id, o = read_str(buf, o, end, "commentary id")
            comment_text, o = read_str(buf, o, end, "commentary text")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        return cls(
            game_id=game_id,
            clock_seconds=clock_seconds,
            half=half,
            halftime=bool(flags & _FLAG_HALFTIME),
            home=TeamState(home_abbr, home_score, TeamColors(home_primary, home_alternate)),
            away=TeamState(away_abbr, away_score, TeamColors(away_primary, away_alternate)),
            last_event=last_event,
            comment_id=comment_id,
            comment_text=comment_text,
        )


class PregameGame:
    """Upcoming soccer match.

    Duck-typed to the contract `state.set_pregame` reads (venue /
    weather_temp / weather_condition / start_epoch / away / home with
    wins-losses-pitcher), so the soccer pregame screen reuses the whole MLB
    pregame pipeline: `venue` carries the league display name ("MLS",
    "PREMIER LEAGUE") as the cycling info line, and the fields soccer never
    has (weather, records, probables) are permanently absent — the renderer
    already omits absent fields (the pregame-sparse case).
    """

    wire_state = GAME_STATE_PRE

    def __init__(self, game_id: str, start_epoch: int, league: str,
                 home: PregameTeam, away: PregameTeam) -> None:
        self.game_id = game_id
        self.start_epoch = start_epoch
        self.venue = league
        self.weather_temp = None
        self.weather_condition = None
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf, league: str) -> "PregameGame":
        """Parse a soccer pregame payload (see backend/src/wire.rs).

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
            start_time,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_PRE_FMT, buf, HDR_SIZE)

        o = HDR_SIZE + _PRE_SIZE  # 22
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        return cls(
            game_id=game_id,
            start_epoch=start_time,
            league=league,
            home=pregame_team(home_abbr, TeamColors(home_primary, home_alternate)),
            away=pregame_team(away_abbr, TeamColors(away_primary, away_alternate)),
        )


def pregame_team(abbreviation: str, colors: TeamColors) -> PregameTeam:
    """A soccer pregame side: identity + colors, no record, no probable.

    The abbreviation rides the probable-pitcher slot: soccer has no
    per-side pregame line, and the slot renders as a short team-colored
    identity row under the divider instead of leaving the lower half of the
    screen empty.
    """
    return PregameTeam(abbreviation, colors, None, None, abbreviation)


class FinalTeam:
    """One team's full-time snapshot.

    `scorers` is a pre-formatted display string ("Lukaku 45'+3', De Bruyne
    60'"), '' when the team didn't score. Built by the backend from the
    event details (exposure of final scores/scorers is a backend follow-up —
    see BACKLOG).
    """

    def __init__(self, abbreviation: str, score: int, colors: TeamColors,
                 scorers: str) -> None:
        self.abbreviation = abbreviation
        self.score = score
        self.colors = colors
        self.scorers = scorers


class FinalGame:
    """Top-level full-time soccer snapshot."""

    wire_state = GAME_STATE_POST

    def __init__(self, game_id: str, home: FinalTeam, away: FinalTeam) -> None:
        self.game_id = game_id
        self.home = home
        self.away = away

    @classmethod
    def from_struct(cls, buf) -> "FinalGame":
        """Parse a soccer full-time payload (see backend/src/wire.rs)."""
        end = len(buf)
        check_version(buf, end)
        if end < HDR_SIZE + _FINAL_SIZE:
            raise DeserializeError(
                "@2", f"truncated fixed section: {end} < {HDR_SIZE + _FINAL_SIZE}"
            )

        (
            away_score, home_score,
            away_primary, away_alternate, home_primary, home_alternate,
        ) = struct.unpack_from(_FINAL_FMT, buf, HDR_SIZE)

        o = HDR_SIZE + _FINAL_SIZE  # 22
        game_id, o = read_str(buf, o, end, "game_id")
        away_abbr, o = read_str(buf, o, end, "away abbreviation")
        home_abbr, o = read_str(buf, o, end, "home abbreviation")
        away_scorers, o = read_str(buf, o, end, "away scorers")
        home_scorers, o = read_str(buf, o, end, "home scorers")

        if o != end:
            raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")

        return cls(
            game_id=game_id,
            home=FinalTeam(home_abbr, home_score, TeamColors(home_primary, home_alternate), home_scorers),
            away=FinalTeam(away_abbr, away_score, TeamColors(away_primary, away_alternate), away_scorers),
        )


def parse_game_detail(buf, league: str) -> "LiveGame | PregameGame | FinalGame":
    """Parse a soccer detail payload, dispatching on the state header byte.

    `league` is the polled league's display name, threaded to the pregame
    model (live/final don't need it).
    """
    end = len(buf)
    check_version(buf, end)
    if end < HDR_SIZE:
        raise DeserializeError("@1", "truncated before state byte")
    state = buf[1]
    if state == GAME_STATE_IN:
        return LiveGame.from_struct(buf)
    if state == GAME_STATE_PRE:
        return PregameGame.from_struct(buf, league)
    if state == GAME_STATE_POST:
        return FinalGame.from_struct(buf)
    raise DeserializeError("@1", f"unknown game state {state}")

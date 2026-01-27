"""
Data models for the Pico Scoreboard API.

These classes match the OpenAPI schema exactly. The API returns JSON with a
'state' discriminator field that determines which game type is returned.
"""

# Game states (discriminator)
STATE_PREGAME = "pregame"
STATE_LIVE = "live"
STATE_FINAL = "final"

# Quarters
QUARTER_FIRST = "first"
QUARTER_SECOND = "second"
QUARTER_THIRD = "third"
QUARTER_FOURTH = "fourth"
QUARTER_OT = "OT"
QUARTER_OT2 = "OT2"

# Downs
DOWN_FIRST = "first"
DOWN_SECOND = "second"
DOWN_THIRD = "third"
DOWN_FOURTH = "fourth"

# Possession / Winner
TEAM_HOME = "home"
TEAM_AWAY = "away"
TEAM_TIE = "tie"

# Final status
STATUS_FINAL = "final"
STATUS_FINAL_OT = "final/OT"


class Color:
    """RGB color (0-255 per channel)."""

    def __init__(self, r: int, g: int, b: int):
        self.r = r
        self.g = g
        self.b = b

    def __repr__(self):
        return f"Color(r={self.r}, g={self.g}, b={self.b})"

    @staticmethod
    def from_dict(data: dict) -> "Color":
        return Color(
            r=data["r"],
            g=data["g"],
            b=data["b"]
        )


class Team:
    """Team data for pregame (no score)."""

    def __init__(self, abbreviation: str, color: Color, record: str = None):
        self.abbreviation = abbreviation
        self.color = color
        self.record = record

    def __repr__(self):
        return f"Team({self.abbreviation})"

    @staticmethod
    def from_dict(data: dict) -> "Team":
        return Team(
            abbreviation=data["abbreviation"],
            color=Color.from_dict(data["color"]),
            record=data.get("record")
        )


class TeamWithScore:
    """Team data with score and timeouts (for live/final games)."""

    def __init__(
        self,
        abbreviation: str,
        color: Color,
        score: int,
        timeouts: int,
        record: str = None
    ):
        self.abbreviation = abbreviation
        self.color = color
        self.score = score
        self.timeouts = timeouts
        self.record = record

    def __repr__(self):
        return f"TeamWithScore({self.abbreviation}: {self.score})"

    @staticmethod
    def from_dict(data: dict) -> "TeamWithScore":
        return TeamWithScore(
            abbreviation=data["abbreviation"],
            color=Color.from_dict(data["color"]),
            score=data["score"],
            timeouts=data["timeouts"],
            record=data.get("record")
        )


class Weather:
    """Weather information for pregame."""

    def __init__(self, temp: int, description: str):
        self.temp = temp
        self.description = description

    def __repr__(self):
        return f"Weather({self.temp}F, {self.description})"

    @staticmethod
    def from_dict(data: dict) -> "Weather":
        return Weather(
            temp=data["temp"],
            description=data["description"]
        )


class Situation:
    """Current play situation during live games."""

    def __init__(
        self,
        down: str,
        distance: int,
        yard_line: int,
        possession: str,
        red_zone: bool
    ):
        self.down = down
        self.distance = distance
        self.yard_line = yard_line
        self.possession = possession
        self.red_zone = red_zone

    def __repr__(self):
        return f"Situation({self.down}&{self.distance} at {self.yard_line})"

    @staticmethod
    def from_dict(data: dict) -> "Situation":
        return Situation(
            down=data["down"],
            distance=data["distance"],
            yard_line=data["yard_line"],
            possession=data["possession"],
            red_zone=data["red_zone"]
        )


class PregameGame:
    """Game data before kickoff."""

    def __init__(
        self,
        event_id: str,
        home: Team,
        away: Team,
        start_time: str,
        venue: str = None,
        broadcast: str = None,
        weather: Weather = None
    ):
        self.state = STATE_PREGAME
        self.event_id = event_id
        self.home = home
        self.away = away
        self.start_time = start_time
        self.venue = venue
        self.broadcast = broadcast
        self.weather = weather

    def __repr__(self):
        return f"PregameGame({self.away.abbreviation} @ {self.home.abbreviation})"

    @staticmethod
    def from_dict(data: dict) -> "PregameGame":
        weather = None
        if data.get("weather"):
            weather = Weather.from_dict(data["weather"])

        return PregameGame(
            event_id=data["event_id"],
            home=Team.from_dict(data["home"]),
            away=Team.from_dict(data["away"]),
            start_time=data["start_time"],
            venue=data.get("venue"),
            broadcast=data.get("broadcast"),
            weather=weather
        )


class LiveGame:
    """Game data during play."""

    def __init__(
        self,
        event_id: str,
        home: TeamWithScore,
        away: TeamWithScore,
        quarter: str,
        clock: str,
        clock_running: bool = False,
        situation: Situation = None
    ):
        self.state = STATE_LIVE
        self.event_id = event_id
        self.home = home
        self.away = away
        self.quarter = quarter
        self.clock = clock
        self.clock_running = clock_running
        self.situation = situation

    def __repr__(self):
        return f"LiveGame({self.away.abbreviation} {self.away.score} @ {self.home.abbreviation} {self.home.score})"

    @staticmethod
    def from_dict(data: dict) -> "LiveGame":
        situation = None
        if data.get("situation"):
            situation = Situation.from_dict(data["situation"])

        return LiveGame(
            event_id=data["event_id"],
            home=TeamWithScore.from_dict(data["home"]),
            away=TeamWithScore.from_dict(data["away"]),
            quarter=data["quarter"],
            clock=data["clock"],
            clock_running=data.get("clock_running", False),
            situation=situation
        )


class FinalGame:
    """Game data after completion."""

    def __init__(
        self,
        event_id: str,
        home: TeamWithScore,
        away: TeamWithScore,
        status: str,
        winner: str
    ):
        self.state = STATE_FINAL
        self.event_id = event_id
        self.home = home
        self.away = away
        self.status = status
        self.winner = winner

    def __repr__(self):
        return f"FinalGame({self.away.abbreviation} {self.away.score} @ {self.home.abbreviation} {self.home.score} - {self.status})"

    @staticmethod
    def from_dict(data: dict) -> "FinalGame":
        return FinalGame(
            event_id=data["event_id"],
            home=TeamWithScore.from_dict(data["home"]),
            away=TeamWithScore.from_dict(data["away"]),
            status=data["status"],
            winner=data["winner"]
        )


def parse_game_response(data: dict):
    """
    Parse API JSON response into the appropriate game model.

    The 'state' field discriminates between game types:
    - "pregame" -> PregameGame
    - "live" -> LiveGame
    - "final" -> FinalGame

    Args:
        data: JSON dict from API response

    Returns:
        PregameGame, LiveGame, or FinalGame instance

    Raises:
        ValueError: If state field is missing or unknown
    """
    state = data.get("state")

    if state == STATE_PREGAME:
        return PregameGame.from_dict(data)
    elif state == STATE_LIVE:
        return LiveGame.from_dict(data)
    elif state == STATE_FINAL:
        return FinalGame.from_dict(data)
    else:
        raise ValueError(f"Unknown game state: {state}")

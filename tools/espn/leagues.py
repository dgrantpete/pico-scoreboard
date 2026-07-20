"""Registry of ESPN league slugs (analysis-CLI vocabulary) and the game-day
clock. The collector's poll surface is `targets.py`; this registry only maps
friendly keys to (sport, slug) for discover/schema/spec/validate."""

from dataclasses import dataclass
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

# ESPN's ?dates= parameter is keyed to the US/Eastern game day.
GAME_DAY_TZ = ZoneInfo("America/New_York")

# Until 5am ET the previous day is still "the game day": post-midnight live
# games keep their ?dates= stream, and no league starts before ~7:30am ET.
GAME_DAY_ROLLOVER_HOURS = 5


def game_day() -> str:
    """Current YYYYMMDD game day, evaluated per poll so long-running
    collection rolls streams at 5am ET."""
    now = datetime.now(GAME_DAY_TZ) - timedelta(hours=GAME_DAY_ROLLOVER_HOURS)
    return now.strftime("%Y%m%d")


@dataclass(frozen=True)
class League:
    sport: str  # ESPN sport slug, e.g. "baseball"
    slug: str   # ESPN league slug, e.g. "mlb" or "fifa.world"


KNOWN_LEAGUES = {
    "mlb": League("baseball", "mlb"),
    "nba": League("basketball", "nba"),
    "wnba": League("basketball", "wnba"),
    "ncaab": League("basketball", "mens-college-basketball"),
    "nfl": League("football", "nfl"),
    "ncaaf": League("football", "college-football"),
    "nhl": League("hockey", "nhl"),
    "world-cup": League("soccer", "fifa.world"),
    "mls": League("soccer", "usa.1"),
    "epl": League("soccer", "eng.1"),
    "liga-mx": League("soccer", "mex.1"),
}


def resolve(arg: str) -> League:
    """Resolve a registry key or a raw 'sport/slug' pair to a League."""
    if arg in KNOWN_LEAGUES:
        return KNOWN_LEAGUES[arg]
    if "/" in arg:
        sport, slug = arg.split("/", 1)
        if sport and slug:
            return League(sport, slug)
    raise ValueError(
        f"unknown league {arg!r}; use one of {', '.join(sorted(KNOWN_LEAGUES))} "
        "or a raw 'sport/slug' pair like 'soccer/fifa.world'"
    )

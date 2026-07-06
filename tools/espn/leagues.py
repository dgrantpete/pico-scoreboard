"""Registry of ESPN league slugs for the site.api.espn.com scoreboard API."""

from dataclasses import dataclass
from zoneinfo import ZoneInfo

# ESPN's ?dates= parameter is keyed to the US/Eastern game day.
GAME_DAY_TZ = ZoneInfo("America/New_York")


@dataclass(frozen=True)
class League:
    sport: str  # ESPN sport slug, e.g. "baseball"
    slug: str   # ESPN league slug, e.g. "mlb" or "fifa.world"

    @property
    def scoreboard_url(self) -> str:
        return f"https://site.api.espn.com/apis/site/v2/sports/{self.sport}/{self.slug}/scoreboard"


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

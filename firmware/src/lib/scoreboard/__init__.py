"""
Pico Scoreboard library.

Provides configuration management and API client for fetching NFL game data.

Example usage:
    from lib.scoreboard import Config, ScoreboardApiClient

    cfg = Config()
    client = ScoreboardApiClient(cfg)
    game = client.get_game()

    if game.state == "live":
        print(f"{game.away.abbreviation} {game.away.score}")
        print(f"{game.home.abbreviation} {game.home.score}")
"""

from .config import Config
from .api_client import ScoreboardApiClient, ApiError
from .models import (
    # Game state constants
    STATE_PREGAME,
    STATE_LIVE,
    STATE_FINAL,
    # Game classes
    PregameGame,
    LiveGame,
    FinalGame,
    # Supporting classes
    Color,
    Team,
    TeamWithScore,
    Weather,
    Situation,
    # Parser
    parse_game_response,
)

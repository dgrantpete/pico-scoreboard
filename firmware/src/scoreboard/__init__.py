"""
Pico Scoreboard library.

Provides configuration management and the backend API client.
"""

APP_NAME = "pico-scoreboard"

from .config import Config
from .api_client import ScoreboardApiClient, ApiError

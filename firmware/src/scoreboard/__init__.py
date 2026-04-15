"""
Pico Scoreboard library.

Provides configuration management and the backend API client.
"""

from .config import Config
from .api_client import ScoreboardApiClient, ApiError

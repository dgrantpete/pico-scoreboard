"""
Async HTTP client for the Pico Scoreboard backend API.
"""

import gc
import time
import ujson
import uasyncio as asyncio
import aiohttp
from .config import Config
from .logger import DEBUG, ERROR
from .models import parse_game_response, PregameGame, LiveGame, FinalGame

# Pre-allocated response buffer to avoid heap fragmentation
_MAX_RESPONSE_SIZE = 16_384
_response_buf = bytearray(_MAX_RESPONSE_SIZE)
_response_mv = memoryview(_response_buf)

# Request timeout in seconds
_REQUEST_TIMEOUT = 15


class ApiError(Exception):
    """
    Raised when the API returns an error response (4xx/5xx).

    Attributes:
        status_code: HTTP status code
        error: Error code from response (e.g., "game_not_found")
        message: Human-readable error message
    """

    def __init__(self, status_code: int, error: str, message: str) -> None:
        self.status_code: int = status_code
        self.error: str = error
        self.message: str = message
        super().__init__(f"{status_code}: {error} - {message}")


def _log_api(config, tag, path, status, start_ms):
    if config.log_level >= DEBUG:
        elapsed = time.ticks_diff(time.ticks_ms(), start_ms)
        print(f"[{tag}] GET {path}: status={status} elapsed={elapsed}ms")


class ScoreboardApiClient:
    """
    Async HTTP client for the Pico Scoreboard backend API.

    Fetches game data from the backend, which proxies ESPN's API and
    transforms it into a minimal format suitable for the Pico display.

    Example usage:
        cfg = Config()
        client = ScoreboardApiClient(cfg)
        game = await client.get_game()
        print(game.state, game.home.abbreviation)
    """

    def __init__(self, config: Config) -> None:
        self._config: Config = config
        self._session: aiohttp.ClientSession = aiohttp.ClientSession()

    def _games_path(self) -> str:
        """Return the games API path, using mock path if mock mode is enabled."""
        if self._config.api_mock:
            return "/api/mock/games"
        return "/api/football/nfl/games"

    async def get_game(self, event_id: str) -> PregameGame | LiveGame | FinalGame:
        """
        Fetch game data for the given event_id.

        Args:
            event_id: ESPN event ID (numeric string)

        Returns:
            PregameGame, LiveGame, or FinalGame depending on game state

        Raises:
            ApiError: On 4xx/5xx responses from the API
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
            ValueError: If response contains unknown game state
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{self._games_path()}/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}

        async with self._session.get(url, headers=headers, ssl=True) as resp:
            if resp.status != 200:
                try:
                    data = await resp.json()
                    error = data.get("error", "unknown")
                    message = data.get("message", "Unknown error")
                except (ValueError, KeyError):
                    error = "unknown"
                    message = f"HTTP {resp.status}"
                raise ApiError(resp.status, error, message)

            data = await resp.json()
            return parse_game_response(data)

    async def get_game_safe(self, event_id: str) -> PregameGame | LiveGame | FinalGame | None:
        """
        Fetch game data, returning None on any error.

        This is a convenience wrapper around get_game() that catches all
        exceptions and returns None instead. Useful for polling loops where
        you want to continue even if a single request fails.

        Args:
            event_id: ESPN event ID (numeric string)

        Returns:
            PregameGame, LiveGame, FinalGame, or None on error
        """
        try:
            return await asyncio.wait_for(self.get_game(event_id), timeout=_REQUEST_TIMEOUT)
        except (ApiError, OSError, ValueError) as e:
            if self._config.log_level >= ERROR:
                print(f"[API] fetch failed: event_id={event_id} error={e}")
            return None
        except asyncio.TimeoutError:
            if self._config.log_level >= ERROR:
                print(f"[API] fetch failed: event_id={event_id} error=timeout")
            return None

    async def get_all_games(self) -> list[PregameGame | LiveGame | FinalGame]:
        """
        Fetch all games from the backend.

        Returns:
            List of PregameGame, LiveGame, or FinalGame objects

        Raises:
            ApiError: On 4xx/5xx responses from the API
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
            ValueError: If response contains unknown game state
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{self._games_path()}"
        headers = {"X-Api-Key": self._config.api_key}

        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            if resp.status != 200:
                try:
                    data = await resp.json()
                    error = data.get("error", "unknown")
                    message = data.get("message", "Unknown error")
                except (ValueError, KeyError):
                    error = "unknown"
                    message = f"HTTP {resp.status}"
                _log_api(self._config, "API", self._games_path(), resp.status, _t)
                raise ApiError(resp.status, error, message)

            data = await resp.json()
            _log_api(self._config, "API", self._games_path(), 200, _t)
            return [parse_game_response(game) for game in data]

    async def get_all_games_safe(self) -> list[PregameGame | LiveGame | FinalGame]:
        """
        Fetch all games, returning empty list on any error.

        This is a convenience wrapper around get_all_games() that catches all
        exceptions and returns an empty list instead.

        Returns:
            List of game objects, or empty list on error
        """
        try:
            return await asyncio.wait_for(self.get_all_games(), timeout=_REQUEST_TIMEOUT)
        except (ApiError, OSError, ValueError) as e:
            if self._config.log_level >= ERROR:
                print(f"[API] fetch all failed: error={e}")
            return []
        except asyncio.TimeoutError:
            if self._config.log_level >= ERROR:
                print("[API] fetch all failed: error=timeout")
            return []

    async def get_game_raw(self, event_id: str) -> tuple[int, memoryview]:
        """
        Fetch raw game data bytes without parsing.

        Returns the response body as raw bytes in a pre-allocated buffer,
        avoiding JSON serialization/deserialization overhead on the Pico.

        Args:
            event_id: ESPN event ID (numeric string)

        Returns:
            Tuple of (status_code, body_memoryview)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{self._games_path()}/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}

        async with self._session.get(url, headers=headers, ssl=True) as resp:
            return (resp.status, await resp.readinto(_response_mv))

    async def get_all_games_raw(self) -> tuple[int, memoryview]:
        """
        Fetch raw games list bytes without parsing.

        Returns the response body as raw bytes in a pre-allocated buffer,
        avoiding JSON serialization/deserialization overhead on the Pico.

        Returns:
            Tuple of (status_code, body_memoryview)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{self._games_path()}"
        headers = {"X-Api-Key": self._config.api_key}

        async with self._session.get(url, headers=headers, ssl=True) as resp:
            return (resp.status, await resp.readinto(_response_mv))

    async def get_team_logo_raw(self, team_id: str, width: int | None = None, height: int | None = None,
                                background_color: str | None = None, accept: str | None = None) -> tuple[int, memoryview]:
        """
        Fetch team logo as raw bytes into pre-allocated buffer.

        Args:
            team_id: Team abbreviation (e.g., "dal", "nyy")
            width: Optional width in pixels
            height: Optional height in pixels
            background_color: Optional hex color (e.g., "FF0000")
            accept: Optional Accept header value for format selection

        Returns:
            Tuple of (status_code, body_memoryview)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}/api/football/nfl/{team_id}/logo"
        params = []
        if width is not None:
            params.append(f"width={width}")
        if height is not None:
            params.append(f"height={height}")
        if background_color is not None:
            params.append(f"background_color={background_color}")
        if params:
            url += "?" + "&".join(params)

        headers = {"X-Api-Key": self._config.api_key}
        if accept:
            headers["Accept"] = accept

        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            result = (resp.status, await resp.readinto(_response_mv))
            _log_api(self._config, "LOGO", f"logo/{team_id}", resp.status, _t)
            return result

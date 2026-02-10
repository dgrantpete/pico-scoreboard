"""
HTTP client for the Pico Scoreboard backend API.
"""

import gc
import ujson
from .config import Config
from .models import parse_game_response

try:
    import urequests as requests
except ImportError:
    import requests

# Pre-allocated response buffer to avoid heap fragmentation
_MAX_RESPONSE_SIZE = 16_384
_response_buf = bytearray(_MAX_RESPONSE_SIZE)
_response_mv = memoryview(_response_buf)


def _read_response_body(response):
    """
    Read response body into pre-allocated buffer (zero-copy).

    Args:
        response: urequests Response object

    Returns:
        memoryview of the body data
    """
    content_len = int(response.headers.get("content-length", 0))

    if content_len == 0:
        raise ValueError("Missing or zero Content-Length header")

    if content_len > _MAX_RESPONSE_SIZE:
        raise ValueError(f"Response too large: {content_len} > {_MAX_RESPONSE_SIZE}")

    mv = _response_mv[:content_len]
    bytes_read = 0
    while bytes_read < content_len:
        n = response.raw.readinto(mv[bytes_read:])
        if not n:
            raise OSError(f"Connection closed after {bytes_read}/{content_len} bytes")
        bytes_read += n

    return mv


class ApiError(Exception):
    """
    Raised when the API returns an error response (4xx/5xx).

    Attributes:
        status_code: HTTP status code
        error: Error code from response (e.g., "game_not_found")
        message: Human-readable error message
    """

    def __init__(self, status_code: int, error: str, message: str):
        self.status_code = status_code
        self.error = error
        self.message = message
        super().__init__(f"{status_code}: {error} - {message}")


class ScoreboardApiClient:
    """
    HTTP client for the Pico Scoreboard backend API.

    Fetches game data from the backend, which proxies ESPN's API and
    transforms it into a minimal format suitable for the Pico display.

    Example usage:
        cfg = Config()
        client = ScoreboardApiClient(cfg)
        game = client.get_game()
        print(game.state, game.home.abbreviation)
    """

    def __init__(self, config: Config):
        """
        Initialize the API client.

        Args:
            config: Config instance with API URL and key
        """
        self._config = config

    def _games_path(self) -> str:
        """Return the games API path, using mock path if mock mode is enabled."""
        if self._config.api_mock:
            return "/api/mock/games"
        return "/api/games"

    def get_game(self, event_id: str):
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

        response = requests.get(url, headers=headers)
        try:
            status_code = response.status_code
            body = _read_response_body(response)

            if status_code != 200:
                # Try to parse error response
                try:
                    data = ujson.loads(body)
                    error = data.get("error", "unknown")
                    message = data.get("message", "Unknown error")
                except (ValueError, KeyError):
                    error = "unknown"
                    message = f"HTTP {status_code}"

                raise ApiError(status_code, error, message)

            # Parse successful response
            data = ujson.loads(body)
            return parse_game_response(data)
        finally:
            response.close()

    def get_game_safe(self, event_id: str):
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
            return self.get_game(event_id)
        except (ApiError, OSError, ValueError) as e:
            print(f"API error: {e}")
            return None

    def get_all_games(self):
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

        response = requests.get(url, headers=headers)
        try:
            status_code = response.status_code
            body = _read_response_body(response)

            if status_code != 200:
                # Try to parse error response
                try:
                    data = ujson.loads(body)
                    error = data.get("error", "unknown")
                    message = data.get("message", "Unknown error")
                except (ValueError, KeyError):
                    error = "unknown"
                    message = f"HTTP {status_code}"

                raise ApiError(status_code, error, message)

            # Parse successful response (array of games)
            data = ujson.loads(body)
            return [parse_game_response(game) for game in data]
        finally:
            response.close()

    def get_all_games_safe(self):
        """
        Fetch all games, returning empty list on any error.

        This is a convenience wrapper around get_all_games() that catches all
        exceptions and returns an empty list instead.

        Returns:
            List of game objects, or empty list on error
        """
        try:
            return self.get_all_games()
        except (ApiError, OSError, ValueError) as e:
            print(f"API error: {e}")
            return []

    def get_game_raw(self, event_id: str):
        """
        Fetch raw game data bytes without parsing.

        Returns the response body as raw bytes, avoiding JSON
        serialization/deserialization overhead on the Pico.

        Args:
            event_id: ESPN event ID (numeric string)

        Returns:
            Tuple of (status_code, body_bytes)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{self._games_path()}/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}

        response = requests.get(url, headers=headers)
        try:
            return (response.status_code, _read_response_body(response))
        finally:
            response.close()

    def get_all_games_raw(self):
        """
        Fetch raw games list bytes without parsing.

        Returns the response body as raw bytes, avoiding JSON
        serialization/deserialization overhead on the Pico.

        Returns:
            Tuple of (status_code, body_bytes)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{self._games_path()}"
        headers = {"X-Api-Key": self._config.api_key}

        response = requests.get(url, headers=headers)
        try:
            return (response.status_code, _read_response_body(response))
        finally:
            response.close()

    def get_team_logo_raw(self, team_id: str, width: int = None, height: int = None,
                         background_color: str = None, accept: str = None):
        """
        Fetch team logo as raw bytes.

        Args:
            team_id: Team abbreviation (e.g., "dal", "nyy")
            width: Optional width in pixels
            height: Optional height in pixels
            background_color: Optional hex color (e.g., "FF0000")
            accept: Optional Accept header value for format selection

        Returns:
            Tuple of (status_code, body_bytes)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}/api/teams/{team_id}/logo"
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

        response = requests.get(url, headers=headers)
        try:
            return (response.status_code, _read_response_body(response))
        finally:
            response.close()

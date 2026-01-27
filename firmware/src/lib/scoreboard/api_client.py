"""
HTTP client for the Pico Scoreboard backend API.

Uses a persistent HTTP connection to avoid socket exhaustion and
ENOMEM errors that occur with urequests' per-request connections.
"""

import ujson
from .config import Config
from .models import parse_game_response
from lib.http_client import PersistentHttp, parse_url

# Request timeout in seconds
REQUEST_TIMEOUT_SECS = 10


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
        self._http = None
        self._base_path = ""

    def _ensure_http(self):
        """Lazily initialize the HTTP client when first needed."""
        if self._http is None:
            use_ssl, host, port, base_path = parse_url(self._config.api_url)
            self._http = PersistentHttp(host, port, use_ssl, REQUEST_TIMEOUT_SECS)
            self._base_path = base_path.rstrip("/")
            print(f"API client: initialized for {host}:{port} (ssl={use_ssl})")

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
        self._ensure_http()
        path = f"{self._base_path}{self._games_path()}/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}

        status_code, body = self._http.get(path, headers)

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
        self._ensure_http()
        path = f"{self._base_path}{self._games_path()}"
        headers = {"X-Api-Key": self._config.api_key}

        status_code, body = self._http.get(path, headers)

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
        self._ensure_http()
        path = f"{self._base_path}{self._games_path()}/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}
        return self._http.get(path, headers)

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
        self._ensure_http()
        path = f"{self._base_path}{self._games_path()}"
        headers = {"X-Api-Key": self._config.api_key}
        return self._http.get(path, headers)

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
        self._ensure_http()
        path = f"{self._base_path}/api/teams/{team_id}/logo"
        params = []
        if width is not None:
            params.append(f"width={width}")
        if height is not None:
            params.append(f"height={height}")
        if background_color is not None:
            params.append(f"background_color={background_color}")
        if params:
            path += "?" + "&".join(params)

        headers = {"X-Api-Key": self._config.api_key}
        if accept:
            headers["Accept"] = accept

        return self._http.get(path, headers)

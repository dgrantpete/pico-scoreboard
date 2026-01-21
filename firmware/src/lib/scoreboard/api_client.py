"""
HTTP client for the Pico Scoreboard backend API.

Uses urequests (MicroPython's built-in HTTP library) to fetch game data.
"""

import urequests
from .config import Config
from .models import parse_game_response

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
        url = f"{self._config.api_url}/api/games/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}

        response = None
        try:
            response = urequests.get(url, headers=headers, timeout=REQUEST_TIMEOUT_SECS)

            if response.status_code != 200:
                # Try to parse error response
                try:
                    data = response.json()
                    error = data.get("error", "unknown")
                    message = data.get("message", "Unknown error")
                except (ValueError, KeyError):
                    error = "unknown"
                    message = f"HTTP {response.status_code}"

                raise ApiError(response.status_code, error, message)

            # Parse successful response
            data = response.json()
            return parse_game_response(data)

        finally:
            # CRITICAL: Always close the response to free memory
            # MicroPython has limited RAM, so leaked connections are problematic
            if response is not None:
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
        url = f"{self._config.api_url}/api/games"
        headers = {"X-Api-Key": self._config.api_key}

        response = None
        try:
            response = urequests.get(url, headers=headers, timeout=REQUEST_TIMEOUT_SECS)

            if response.status_code != 200:
                # Try to parse error response
                try:
                    data = response.json()
                    error = data.get("error", "unknown")
                    message = data.get("message", "Unknown error")
                except (ValueError, KeyError):
                    error = "unknown"
                    message = f"HTTP {response.status_code}"

                raise ApiError(response.status_code, error, message)

            # Parse successful response (array of games)
            data = response.json()
            return [parse_game_response(game) for game in data]

        finally:
            # CRITICAL: Always close the response to free memory
            if response is not None:
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
        url = f"{self._config.api_url}/api/games/{event_id}"
        headers = {"X-Api-Key": self._config.api_key}

        response = None
        try:
            response = urequests.get(url, headers=headers, timeout=REQUEST_TIMEOUT_SECS)
            return (response.status_code, response.content)
        finally:
            if response is not None:
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
        url = f"{self._config.api_url}/api/games"
        headers = {"X-Api-Key": self._config.api_key}

        response = None
        try:
            response = urequests.get(url, headers=headers, timeout=REQUEST_TIMEOUT_SECS)
            return (response.status_code, response.content)
        finally:
            if response is not None:
                response.close()

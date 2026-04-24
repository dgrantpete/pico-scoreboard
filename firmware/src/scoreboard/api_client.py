"""
Async HTTP client for the Pico Scoreboard backend API.
"""

import gc
import json
import time
import aiohttp
from .config import Config
from .logger import DEBUG
from .mlb import LiveGame, DeserializeError

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
        error: Short error code string from the response body
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

    Provides the backend-authenticated HTTP session used by the firmware.
    Currently exposes raw logo fetching into a pre-allocated buffer; the
    game-update pipeline will grow additional methods on top of this client.
    """

    def __init__(self, config: Config) -> None:
        self._config: Config = config
        self._session: aiohttp.ClientSession = aiohttp.ClientSession()

    async def get_team_logo_raw(self, path: str, width: int | None = None, height: int | None = None,
                                background_color: str | None = None, accept: str | None = None) -> tuple[int, memoryview]:
        """
        Fetch a team logo as raw bytes into the pre-allocated buffer.

        Args:
            path: Backend URL path for the logo resource (e.g. "/api/foo/bar/logo").
            width: Optional width in pixels.
            height: Optional height in pixels.
            background_color: Optional hex color (e.g. "FF0000").
            accept: Optional Accept header value for format selection.

        Returns:
            Tuple of (status_code, body_memoryview)

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
        """
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{path}"
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
            _log_api(self._config, "LOGO", path, resp.status, _t)
            return result

    async def _fetch_json(self, path: str, tag: str) -> object:
        """Reuses the module-level `_response_mv` buffer to avoid heap churn on the Pico."""
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}{path}"
        headers = {"X-Api-Key": self._config.api_key, "Accept": "application/json"}
        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            filled = await resp.readinto(_response_mv)
            _log_api(self._config, tag, path, resp.status, _t)
            if resp.status >= 400:
                err_code = "unknown_error"
                err_msg = ""
                try:
                    err = json.loads(filled)
                    if isinstance(err, dict):
                        err_code = str(err.get("error", err_code))
                        err_msg = str(err.get("message", ""))
                except ValueError:
                    pass
                raise ApiError(resp.status, err_code, err_msg)
            return json.loads(filled)

    async def get_game_list(
        self, if_none_match: str | None
    ) -> tuple[int, list[str], str | None]:
        # The returned etag is the raw header value (quotes included) so the
        # caller can echo it verbatim as If-None-Match — backend does a strict
        # string match and will not recognize a stripped-quote form.
        gc.collect()
        url = f"{self._config.api_url.rstrip('/')}/mlb/games"
        headers = {"X-Api-Key": self._config.api_key, "Accept": "application/json"}
        if if_none_match is not None:
            headers["If-None-Match"] = if_none_match

        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            etag = None
            if resp.headers:
                for k in resp.headers:
                    if k.lower() == "etag":
                        etag = resp.headers[k]
                        break

            if resp.status == 304:
                _log_api(self._config, "MLB-GAMES", "/mlb/games", resp.status, _t)
                return (304, [], etag)

            filled = await resp.readinto(_response_mv)
            _log_api(self._config, "MLB-GAMES", "/mlb/games", resp.status, _t)

            if resp.status >= 400:
                err_code = "unknown_error"
                err_msg = ""
                try:
                    err = json.loads(filled)
                    if isinstance(err, dict):
                        err_code = str(err.get("error", err_code))
                        err_msg = str(err.get("message", ""))
                except ValueError:
                    pass
                raise ApiError(resp.status, err_code, err_msg)

            raw = json.loads(filled)
            if not isinstance(raw, list):
                raise ApiError(0, "invalid_response", f"expected list, got {type(raw).__name__}")
            ids: list[str] = []
            for item in raw:
                if not isinstance(item, str):
                    raise ApiError(0, "invalid_response", f"expected string game id, got {type(item).__name__}")
                ids.append(item)
            return (resp.status, ids, etag)

    async def get_game_state(self, game_id: str) -> LiveGame | None:
        try:
            raw = await self._fetch_json(f"/mlb/games/{game_id}", "MLB-GAME")
        except ApiError as e:
            if e.status_code == 404:
                return None
            raise
        if not isinstance(raw, dict):
            raise DeserializeError("$", f"expected object, got {type(raw).__name__}")
        return LiveGame.from_dict(raw)

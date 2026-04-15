"""
Async HTTP client for the Pico Scoreboard backend API.
"""

import gc
import time
import uasyncio as asyncio
import aiohttp
from .config import Config
from .logger import DEBUG, ERROR

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

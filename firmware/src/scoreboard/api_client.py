"""
Async HTTP client for the Pico Scoreboard backend API.

No manual gc.collect() calls: MicroPython auto-collects (and retries) when an
allocation fails, so genuine OOMs surface honestly instead of being masked by
scheduled pauses. Memory robustness comes from the pre-allocated response
buffer below, not from collection timing.
"""

import json
import time
import aiohttp
import uasyncio as asyncio
from .config import Config
import scoreboard.logger as logger
from .logger import DEBUG
from .mlb import LiveGame, parse_game_ids, STRUCT_CONTENT_TYPE

# Pre-allocated response buffer to avoid heap fragmentation
_MAX_RESPONSE_SIZE = 16_384
_response_buf = bytearray(_MAX_RESPONSE_SIZE)
_response_mv = memoryview(_response_buf)

# Applied to every request via asyncio.wait_for. Without this a wedged TCP
# connection (backend hang, silent WiFi drop) would stall the poller forever
# — the display watchdog only guards the render thread, not networking.
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


def _log_api(tag, path, status, start_ms):
    # Guarded: runs on every request; skip the f-string build below DEBUG.
    if logger.level >= DEBUG:
        elapsed = time.ticks_diff(time.ticks_ms(), start_ms)
        logger.debug(f"[{tag}] GET {path}: status={status} elapsed={elapsed}ms")


def _raise_api_error(status: int, body) -> None:
    """Parse an error response body and raise the corresponding ApiError."""
    err_code = "unknown_error"
    err_msg = ""
    try:
        err = json.loads(body)
        if isinstance(err, dict):
            err_code = str(err.get("error", err_code))
            err_msg = str(err.get("message", ""))
    except ValueError:
        pass
    raise ApiError(status, err_code, err_msg)


class ScoreboardApiClient:
    """
    Async HTTP client for the Pico Scoreboard backend API.

    Provides the backend-authenticated HTTP session used by the firmware.
    Every request runs under a hard timeout; on timeout the underlying
    connection is dropped so the next request reconnects cleanly.
    """

    def __init__(self, config: Config) -> None:
        self._config: Config = config
        self._session: aiohttp.ClientSession = aiohttp.ClientSession()

    async def _with_timeout(self, coro):
        """Run a request coroutine under _REQUEST_TIMEOUT."""
        try:
            return await asyncio.wait_for(coro, _REQUEST_TIMEOUT)
        except asyncio.TimeoutError:
            # The connection is in an unknown mid-request state — drop it so
            # the next request opens a fresh one.
            await self._session.close()
            raise

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
            Tuple of (status_code, body_memoryview). The memoryview aliases a
            shared module-level buffer — copy it out before the next request.

        Raises:
            OSError: On network errors (WiFi disconnected, DNS failure, etc.)
            asyncio.TimeoutError: If the request exceeds _REQUEST_TIMEOUT.
        """
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

        return await self._with_timeout(self._get_logo_inner(url, path, headers))

    async def _get_logo_inner(self, url: str, path: str, headers: dict) -> tuple[int, memoryview]:
        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            result = (resp.status, await resp.readinto(_response_mv))
            _log_api("LOGO", path, resp.status, _t)
            return result

    async def _get_struct_inner(self, url: str, path: str, tag: str, headers: dict):
        """Fetch a binary wire-format body into the shared buffer.

        Returns the filled memoryview; raises ApiError on 4xx/5xx (error
        bodies are always JSON regardless of the Accept header).
        """
        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            filled = await resp.readinto(_response_mv)
            _log_api(tag, path, resp.status, _t)
            if resp.status >= 400:
                _raise_api_error(resp.status, filled)
            return filled

    async def get_game_list(
        self, if_none_match: str | None
    ) -> tuple[int, list[str], str | None]:
        # The returned etag is the raw header value (quotes included) so the
        # caller can echo it verbatim as If-None-Match — backend does a strict
        # string match and will not recognize a stripped-quote form.
        url = f"{self._config.api_url.rstrip('/')}/mlb/games"
        headers = {"X-Api-Key": self._config.api_key, "Accept": STRUCT_CONTENT_TYPE}
        if if_none_match is not None:
            headers["If-None-Match"] = if_none_match

        return await self._with_timeout(self._get_game_list_inner(url, headers))

    async def _get_game_list_inner(
        self, url: str, headers: dict
    ) -> tuple[int, list[str], str | None]:
        _t = time.ticks_ms()
        async with self._session.get(url, headers=headers, ssl=True) as resp:
            etag = None
            if resp.headers:
                for k in resp.headers:
                    if k.lower() == "etag":
                        etag = resp.headers[k]
                        break

            if resp.status == 304:
                _log_api("MLB-GAMES", "/mlb/games", resp.status, _t)
                return (304, [], etag)

            filled = await resp.readinto(_response_mv)
            _log_api("MLB-GAMES", "/mlb/games", resp.status, _t)

            if resp.status >= 400:
                _raise_api_error(resp.status, filled)

            return (resp.status, parse_game_ids(filled), etag)

    async def get_game_state(self, game_id: str) -> LiveGame | None:
        path = f"/mlb/games/{game_id}"
        url = f"{self._config.api_url.rstrip('/')}{path}"
        headers = {"X-Api-Key": self._config.api_key, "Accept": STRUCT_CONTENT_TYPE}
        try:
            filled = await self._with_timeout(
                self._get_struct_inner(url, path, "MLB-GAME", headers)
            )
        except ApiError as e:
            if e.status_code == 404:
                return None
            raise
        # No awaits between here and the parse: the shared buffer can't be
        # overwritten by another request before from_struct reads it.
        return LiveGame.from_struct(filled)

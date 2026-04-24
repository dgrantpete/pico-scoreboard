"""
MLB polling loop.

Owns all polling state on the `MlbPoller` instance. No module-level or
class-level mutable state — the instance is the single source of truth so
that the firmware cannot accidentally end up with two pollers sharing a
rotation index or ETag.
"""

import time
import uasyncio as asyncio

from .api_client import ScoreboardApiClient
from .config import Config
from .display import get_logo_framebuffer
from .logger import DEBUG, ERROR
from .state import get_write_state, commit_state, set_error


class MlbPoller:
    MAX_FAILURES: int = 5

    def __init__(self, config: Config, api_client: ScoreboardApiClient) -> None:
        self._config: Config = config
        self._api: ScoreboardApiClient = api_client
        self._game_ids: list[str] = []
        self._etag: str | None = None
        self._current_index: int = 0
        self._last_rotation_ms: int | None = None
        self._consecutive_failures: int = 0
        self._animation_reset: bool = True

    async def run(self) -> None:
        while True:
            try:
                await self._tick()
                self._consecutive_failures = 0
            except Exception as e:
                self._consecutive_failures += 1
                if self._config.log_level >= ERROR:
                    print(f"[MLB] poll failed ({self._consecutive_failures}/{self.MAX_FAILURES}): {e}")
                if self._consecutive_failures >= self.MAX_FAILURES:
                    set_error("API ERROR", [str(e)[:20]])

            await asyncio.sleep(self._config.poll_interval_seconds)

    async def _tick(self) -> None:
        now = time.ticks_ms()

        if self._last_rotation_ms is None:
            await self._refresh_list(initial=True)
            self._current_index = 0
            self._last_rotation_ms = now
            self._animation_reset = True
        elif time.ticks_diff(now, self._last_rotation_ms) >= self._config.game_rotation_seconds * 1000:
            await self._rotate(now)
            self._animation_reset = True
        else:
            self._animation_reset = False

        if not self._game_ids:
            state = get_write_state()
            state.mode = 'no_games'
            state.dirty = True
            commit_state()
            return

        await self._poll_current()

    async def _refresh_list(self, initial: bool) -> None:
        if_none_match = None if initial else self._etag
        status, ids, etag = await self._api.get_game_list(if_none_match)
        if status == 304:
            return
        self._game_ids = ids
        self._etag = etag
        if self._current_index >= len(self._game_ids):
            self._current_index = 0
        if self._config.log_level >= DEBUG:
            print(f"[MLB] game list refreshed: count={len(self._game_ids)} etag={self._etag}")

    async def _rotate(self, now: int) -> None:
        await self._refresh_list(initial=False)
        if self._game_ids:
            self._current_index = (self._current_index + 1) % len(self._game_ids)
        self._last_rotation_ms = now

    async def _poll_current(self) -> None:
        game_id = self._game_ids[self._current_index]
        live = await self._api.get_game_state(game_id)
        if live is None:
            # 404 means the game ended between list refresh and state fetch;
            # skip this slot and let the next rotation pick up a fresh list.
            return

        home_logo = await get_logo_framebuffer(
            self._api, f"mlb-{live.home.abbreviation}", f"/mlb/{live.home.abbreviation}/logo"
        )
        away_logo = await get_logo_framebuffer(
            self._api, f"mlb-{live.away.abbreviation}", f"/mlb/{live.away.abbreviation}/logo"
        )

        state = get_write_state()
        state.mode = 'game'
        state.game.game_id = game_id
        state.game.live = live
        state.game.fetched_ms = time.ticks_ms()
        state.home_logo = home_logo
        state.away_logo = away_logo
        if self._animation_reset:
            state.animation_start_ms = time.ticks_ms()
        state.dirty = True
        commit_state()

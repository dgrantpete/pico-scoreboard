"""
MLB polling loop.

Owns all polling state on the `MlbPoller` instance. No module-level or
class-level mutable state — the instance is the single source of truth so
that the firmware cannot accidentally end up with two pollers sharing a
rotation index or ETag.

Button input hooks (called from the Core 0 input task, same asyncio loop):
- skip():        advance to the next game immediately, waking the poll loop.
- toggle_lock(): freeze/unfreeze game rotation (polling continues).
"""

import time
import uasyncio as asyncio

from .api_client import ApiError, ScoreboardApiClient
from .config import Config
from .display import LogoPool, play_text_display_ms
from .mlb import DeserializeError
import scoreboard.logger as logger
from .state import get_write_state, commit_state, set_error, set_toast


def _friendly_error(e: Exception) -> tuple[str, str]:
    """Map an exception to (kind, detail) lines fit for the LED panel."""
    if isinstance(e, asyncio.TimeoutError):
        return ("Timeout", "backend not responding")
    if isinstance(e, ApiError):
        return (f"HTTP {e.status_code}", e.error)
    if isinstance(e, DeserializeError):
        return ("Bad response", f"{e.path} {e.message}")
    if isinstance(e, OSError):
        return ("Network error", str(e))
    return (type(e).__name__, str(e))


class MlbPoller:
    MAX_FAILURES: int = 5

    def __init__(self, config: Config, api_client: ScoreboardApiClient, logo_pool: LogoPool) -> None:
        self._config: Config = config
        self._api: ScoreboardApiClient = api_client
        self._logos: LogoPool = logo_pool
        self._game_ids: list[str] = []
        self._etag: str | None = None
        self._current_index: int = 0
        self._last_rotation_ms: int | None = None
        self._consecutive_failures: int = 0
        self._first_failure_ms: int = 0
        self._animation_reset: bool = True
        self._locked: bool = False
        self._skip_requested: bool = False
        self._wake: asyncio.Event = asyncio.Event()

    @property
    def locked(self) -> bool:
        return self._locked

    def skip(self) -> None:
        """Advance to the next game now (button input). Safe to call anytime."""
        set_toast("SKIPPING...")
        self._skip_requested = True
        self._wake.set()

    def toggle_lock(self) -> None:
        """Toggle rotation lock (button input). The current game keeps polling."""
        self._locked = not self._locked
        set_toast("LOCKED" if self._locked else "UNLOCKED")
        logger.debug(f"[MLB] rotation lock: {self._locked}")

    async def run(self) -> None:
        while True:
            try:
                await self._tick()
                if self._consecutive_failures > 0:
                    logger.error(
                        f"[MLB] recovered after {self._consecutive_failures} failed polls"
                    )
                self._consecutive_failures = 0
            except Exception as e:
                now = time.ticks_ms()
                if self._consecutive_failures == 0:
                    self._first_failure_ms = now
                self._consecutive_failures += 1
                logger.error(
                    f"[MLB] poll failed ({self._consecutive_failures}/{self.MAX_FAILURES}): "
                    f"{type(e).__name__}: {e}"
                )
                if self._consecutive_failures >= self.MAX_FAILURES:
                    kind, detail = _friendly_error(e)
                    failing_mins = time.ticks_diff(now, self._first_failure_ms) // 60_000
                    lines = [kind, detail[:25]]
                    if len(detail) > 25:
                        lines.append(detail[25:50])
                    lines.append(f"failing for {failing_mins}m")
                    set_error("API ERROR", lines)

            # Sleep until the next poll, but wake immediately on skip().
            try:
                await asyncio.wait_for(
                    self._wake.wait(), self._config.poll_interval_seconds
                )
            except asyncio.TimeoutError:
                pass
            self._wake.clear()

    async def _tick(self) -> None:
        now = time.ticks_ms()
        skip = self._skip_requested
        self._skip_requested = False

        rotation_due = (
            self._last_rotation_ms is not None
            and time.ticks_diff(now, self._last_rotation_ms) >= self._config.game_rotation_seconds * 1000
        )

        if self._last_rotation_ms is None:
            await self._refresh_list(initial=True)
            self._current_index = 0
            self._last_rotation_ms = now
            self._animation_reset = True
        elif skip or (rotation_due and not self._locked):
            await self._rotate(now)
            self._animation_reset = True
        else:
            self._animation_reset = False

        if not self._game_ids:
            state = get_write_state()
            state.mode = 'no_games'
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
        logger.debug(f"[MLB] game list refreshed: count={len(self._game_ids)} etag={self._etag}")

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

        home_logo = await self._logos.get(
            f"mlb-{live.home.abbreviation}",
            f"/baseball/mlb/teams/{live.home.abbreviation}/logo",
        )
        away_logo = await self._logos.get(
            f"mlb-{live.away.abbreviation}",
            f"/baseball/mlb/teams/{live.away.abbreviation}/logo",
        )

        state = get_write_state()
        state.mode = 'game'
        state.game.game_id = game_id
        state.game.live = live
        state.game.fetched_ms = time.ticks_ms()

        # Most-recent play flash: the display thread briefly surfaces the play
        # text whenever the id changes. The write buffer's previous play.id is
        # carried forward after each commit, so this comparison is against the
        # last committed value — no poller-local state needed. Game rotation
        # also legitimately trips this (new game, different ids) so viewers
        # can catch up on the newest play.
        new_play_id = live.last_play.id
        if new_play_id != state.game.play.id:
            state.game.play.id = new_play_id
            state.game.play.text = live.last_play.text
            state.game.play.updated_ms = time.ticks_ms()
            # Window sized to the text: one full scroll cycle, measured here
            # on Core 0 so the display thread never measures text.
            state.game.play.display_ms = play_text_display_ms(live.last_play.text)

        state.home_logo = home_logo
        state.away_logo = away_logo
        if self._animation_reset:
            state.animation_start_ms = time.ticks_ms()
        commit_state()

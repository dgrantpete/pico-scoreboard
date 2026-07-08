"""
MLB polling loop.

Owns all polling state on the `MlbPoller` instance. No module-level or
class-level mutable state — the instance is the single source of truth so
that the firmware cannot accidentally end up with two pollers sharing a
rotation index or ETag.

Rotation is live-first: while any listed game is live, only live games
rotate; when zero games are live the slate falls back to finals (first, in
backend order) then pregames. The whole slate being empty is the only thing
that shows `no_games`.

Button input hooks (called from the Core 0 input task, same asyncio loop):
- skip():        advance to the next game immediately, waking the poll loop.
- toggle_lock(): freeze/unfreeze game rotation (polling continues).

Everything here — the poll loop, both button hooks, and the skip state
machine — runs on Core 0's single asyncio loop. The skip flags are therefore
plain booleans with no locking: a hook and `_tick` can never execute
concurrently, only interleave at `await` points, and the state machine below
is designed around exactly those interleavings.
"""

import time
import uasyncio as asyncio

from .api_client import ApiError, ScoreboardApiClient
from .config import Config
from .display import LogoPool, play_text_display_ms
from .mlb import (
    DeserializeError,
    LiveGame,
    PregameGame,
    FinalGame,
    GAME_STATE_PRE,
    GAME_STATE_IN,
    GAME_STATE_POST,
)
import scoreboard.logger as logger
from .state import (
    get_write_state,
    commit_state,
    set_error,
    set_toast,
    set_pregame,
    set_final,
    clear_toast_if_sticky,
    pulse_toast,
)


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

    def __init__(
        self,
        config: Config,
        api_client: ScoreboardApiClient,
        logo_pool: LogoPool,
        utc_offset_s: int | None = None,
    ) -> None:
        self._config: Config = config
        self._api: ScoreboardApiClient = api_client
        self._logos: LogoPool = logo_pool
        # Full slate from the backend, in chronological order: (state, id).
        self._games: list[tuple[int, str]] = []
        # Ids to actually rotate through, derived from _games by _build_rotation.
        self._rotation: list[str] = []
        self._utc_offset_s: int | None = utc_offset_s
        self._etag: str | None = None
        self._current_index: int = 0
        self._last_rotation_ms: int | None = None
        self._consecutive_failures: int = 0
        self._first_failure_ms: int = 0
        self._animation_reset: bool = True
        self._locked: bool = False
        self._skip_requested: bool = False
        # True only for the duration of the tick that consumed a skip request.
        # skip() rejects presses while this (or a pending request) is set.
        self._skip_in_flight: bool = False
        self._wake: asyncio.Event = asyncio.Event()

    @property
    def locked(self) -> bool:
        return self._locked

    def skip(self) -> None:
        """Advance to the next game now (button input). Safe to call anytime.

        A press that lands while a skip is already armed or in flight is
        rejected — not queued — and instead dims the visible toast one cycle
        as feedback. This is what keeps a burst of presses (including ones that
        land during `_poll_current`'s awaits) from advancing the rotation more
        than once.
        """
        if self._skip_requested or self._skip_in_flight:
            pulse_toast()
            return
        set_toast("SKIPPING", sticky=True)
        self._skip_requested = True
        self._wake.set()

    def toggle_lock(self) -> None:
        """Toggle rotation lock (button input). The current game keeps polling."""
        self._locked = not self._locked
        # Non-sticky by default: a LOCKED/UNLOCKED toast fired mid-skip must
        # survive the skip tick's clear_toast_if_sticky() teardown.
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
        # A consumed skip owns the sticky "SKIPPING" toast for exactly this
        # tick. The finally below tears it down on EVERY exit path — success,
        # empty slate, 404, or a mid-flight exception — so the toast's lifetime
        # is precisely the work it was announcing.
        if skip:
            self._skip_in_flight = True
        try:
            rotation_due = (
                self._last_rotation_ms is not None
                and time.ticks_diff(now, self._last_rotation_ms)
                >= self._config.game_rotation_seconds * 1000
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

            if not self._rotation:
                # Only a truly empty slate reaches here (a non-empty slate
                # always yields at least one rotation entry).
                state = get_write_state()
                state.mode = 'no_games'
                commit_state()
                return

            await self._poll_current()
        finally:
            if skip:
                self._skip_in_flight = False
                clear_toast_if_sticky()

    async def _refresh_list(self, initial: bool) -> None:
        if_none_match = None if initial else self._etag
        status, games, etag = await self._api.get_game_list(if_none_match)
        if status == 304:
            return
        self._games = games
        self._etag = etag
        self._build_rotation()
        logger.debug(
            f"[MLB] game list refreshed: slate={len(self._games)} "
            f"rotation={len(self._rotation)} etag={self._etag}"
        )

    def _build_rotation(self) -> None:
        """Rebuild the rotation from the current slate, live-first.

        Live games (if any) rotate alone; otherwise finals rotate first, then
        pregames, both in backend order. To avoid a mid-view jump when an
        unrelated game flips state, the currently-shown id keeps its position
        in the new rotation if it's still present, else the index resets to 0.
        """
        current_id = (
            self._rotation[self._current_index] if self._rotation else None
        )

        live = [gid for st, gid in self._games if st == GAME_STATE_IN]
        if live:
            rotation = live
        else:
            finals = [gid for st, gid in self._games if st == GAME_STATE_POST]
            pregames = [gid for st, gid in self._games if st == GAME_STATE_PRE]
            rotation = finals + pregames

        self._rotation = rotation
        if current_id is not None:
            try:
                self._current_index = rotation.index(current_id)
            except ValueError:
                self._current_index = 0
        else:
            self._current_index = 0

    async def _rotate(self, now: int) -> None:
        await self._refresh_list(initial=False)
        if self._rotation:
            self._current_index = (self._current_index + 1) % len(self._rotation)
        self._last_rotation_ms = now

    async def _poll_current(self) -> None:
        # Every tick re-fetches the current game — including static pre/post
        # screens. That standing re-poll is what lets a pregame card notice its
        # own pre->in flip mid-view (detail comes back as LiveGame -> the board
        # goes live without waiting for the next rotation). The screens don't
        # flicker on an unchanged re-commit: set_pregame/set_final only restamp
        # the animation clock when the displayed (mode, game_id) identity
        # changes, so a repeat commit preserves the scroll in progress.
        game_id = self._rotation[self._current_index]
        detail = await self._api.get_game_state(game_id)
        if detail is None:
            # 404 means the game left today's scoreboard between the list
            # refresh and this fetch; skip this slot and let the next rotation
            # pick up a fresh list.
            return

        # Abbreviations are present on all three game states, so logos are
        # fetched the same way regardless of type.
        home_logo = await self._logos.get(
            f"mlb-{detail.home.abbreviation}",
            f"/baseball/mlb/teams/{detail.home.abbreviation}/logo",
        )
        away_logo = await self._logos.get(
            f"mlb-{detail.away.abbreviation}",
            f"/baseball/mlb/teams/{detail.away.abbreviation}/logo",
        )

        if isinstance(detail, LiveGame):
            self._commit_live(game_id, detail, home_logo, away_logo)
        elif isinstance(detail, PregameGame):
            set_pregame(detail, home_logo, away_logo, self._utc_offset_s)
        elif isinstance(detail, FinalGame):
            set_final(detail, home_logo, away_logo)
        else:
            # parse_game_detail only ever returns the three types above; a new
            # state would surface here rather than being silently dropped.
            raise DeserializeError("@1", f"unhandled game detail {type(detail).__name__}")

    def _commit_live(self, game_id: str, live: LiveGame, home_logo, away_logo) -> None:
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

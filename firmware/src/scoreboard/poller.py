"""
Game polling loop, generalized over leagues (MLB + NBA + configured soccer
leagues).

Owns all polling state on the `GamePoller` instance. No module-level or
class-level mutable state — the instance is the single source of truth so
that the firmware cannot accidentally end up with two pollers sharing a
rotation index or ETag. The API client supports exactly one in-flight
request, so ONE poller instance owns every configured league and merges
their slates into a single rotation.

Rotation is live-first across the whole merged slate: while any listed game
in any league is live, only live games rotate; when zero games are live the
slate falls back to finals (first) then pregames, leagues in configured
order, backend (chronological) order within a league. The whole merged slate
being empty is the only thing that shows `no_games`.

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
from . import mlb
from . import nba
from . import soccer
from .wire import (
    DeserializeError,
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
    set_soccer_live,
    set_soccer_final,
    set_nba_live,
    set_nba_final,
    clear_toast_if_sticky,
    pulse_toast,
    build_play_strip,
    fit_play_text,
    TOAST_LOCK,
    TOAST_UNLOCK,
    TOAST_SPINNER,
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


class LeagueSource:
    """One pollable league: endpoint paths, parser, commits, and identity.

    `parse` is a synchronous callable over the shared response buffer (see
    ScoreboardApiClient.get_game_state). `commit_live` / `commit_final` are
    the sport's state-commit callables — module functions taking the poller
    first (they need its cross-poll state), so the poll loop dispatches on
    the source with no isinstance chain; pregame is sport-agnostic
    (state.set_pregame) and needs no slot here. `key` namespaces logo cache
    slots and rotation identity across leagues — a soccer "POR" crest must
    never collide with another league's "POR".
    """

    def __init__(self, key: str, tag: str, base_path: str, parse,
                 commit_live, commit_final) -> None:
        self.key = key            # e.g. "baseball/mlb", "soccer/usa.1"
        self.tag = tag            # log tag, e.g. "MLB", "USA.1"
        self.base_path = base_path  # e.g. "/baseball/mlb", "/soccer/usa.1"
        self.parse = parse
        self.commit_live = commit_live
        self.commit_final = commit_final

    def list_path(self) -> str:
        return self.base_path + "/games"

    def detail_path(self, game_id: str) -> str:
        return self.base_path + "/games/" + game_id

    def logo_path(self, abbreviation: str) -> str:
        return self.base_path + "/teams/" + abbreviation + "/logo"

    def logo_key(self, abbreviation: str) -> str:
        return self.key + "/" + abbreviation


def _flash_play(play, new_id: str, raw_text: str) -> bool:
    """Stage the shared play-flash slot when `new_id` is new; True = commit.

    One machinery for every sport's flash line (MLB play, NBA play, soccer
    commentary): the write buffer's previous play.id is carried forward
    after each commit, so comparing against it detects new lines with no
    poller-local state. Game rotation also legitimately trips this (new
    game, different ids) so viewers catch up on the newest line. Fit first:
    text, display window, and strip must describe the same string, and the
    strip must always exist (glyph fallback costs >50 ms/frame on long
    lines — measured 2026-07-12).
    """
    if not new_id or new_id == play.id:
        return False
    text = fit_play_text(raw_text)
    play.id = new_id
    play.text = text
    play.updated_ms = time.ticks_ms()
    play.display_ms = play_text_display_ms(text)
    play.strip = build_play_strip(text)
    return True


def _commit_mlb_live(poller, game_id: str, live, home_logo, away_logo) -> None:
    state = get_write_state()
    state.mode = 'mlb_live'
    state.mlb_live.game_id = game_id
    state.mlb_live.live = live
    state.mlb_live.fetched_ms = time.ticks_ms()

    _flash_play(state.play, live.last_play.id, live.last_play.text)

    state.home_logo = home_logo
    state.away_logo = away_logo
    if poller._animation_reset:
        state.animation_start_ms = time.ticks_ms()
    commit_state()


def _commit_soccer_live(poller, game_id: str, live, home_logo, away_logo) -> None:
    # Stale-clock guard: hand the setter the previous poll's clock for
    # the SAME game so local ticking stops when the upstream value stops
    # advancing while claiming in-play (weather delay, stale feed).
    prev = poller._prev_soccer_clock
    prev_clock_s = prev[1] if prev is not None and prev[0] == game_id else None
    set_soccer_live(live, home_logo, away_logo, prev_clock_s)
    poller._prev_soccer_clock = (game_id, live.clock_seconds)

    # Commentary rides the shared flash slot.
    if _flash_play(get_write_state().play, live.comment_id, live.comment_text):
        commit_state()


def _commit_nba_live(poller, game_id: str, live, home_logo, away_logo) -> None:
    set_nba_live(live, home_logo, away_logo)

    # NBA's last play is optional (absent before the opening tip) — no play,
    # no flash, and the shared play slot keeps its previous id so a play
    # that reappears unchanged doesn't re-flash.
    play = live.last_play
    if play is not None and _flash_play(get_write_state().play, play.id, play.text):
        commit_state()


def mlb_source() -> LeagueSource:
    return LeagueSource("baseball/mlb", "MLB", "/baseball/mlb",
                        mlb.parse_game_detail, _commit_mlb_live, set_final)


def nba_source() -> LeagueSource:
    return LeagueSource("basketball/nba", "NBA", "/basketball/nba",
                        nba.parse_game_detail, _commit_nba_live, set_nba_final)


def soccer_source(slug: str) -> LeagueSource:
    league_name = soccer.LEAGUE_NAMES.get(slug, slug.upper())

    def parse(buf):
        return soccer.parse_game_detail(buf, league_name)

    return LeagueSource("soccer/" + slug, slug.upper(), "/soccer/" + slug,
                        parse, _commit_soccer_live, set_soccer_final)


def sources_from_config(config: Config) -> list:
    """The configured league sources: MLB, then NBA, then soccer leagues in
    config order. Adding a single-league sport = one row in the gate table
    (+ its factory); multi-league sports expand like soccer."""
    sources = []
    for enabled, factory in (
        (config.mlb_enabled, mlb_source),
        (config.nba_enabled, nba_source),
    ):
        if enabled:
            sources.append(factory())
    for slug in config.soccer_leagues:
        sources.append(soccer_source(slug))
    return sources


class GamePoller:
    MAX_FAILURES: int = 5

    def __init__(
        self,
        config: Config,
        api_client: ScoreboardApiClient,
        logo_pool: LogoPool,
        sources: list,
        utc_offset_s: int | None = None,
    ) -> None:
        self._config: Config = config
        self._api: ScoreboardApiClient = api_client
        self._logos: LogoPool = logo_pool
        self._sources: list = sources
        # Per-source slate cache and ETag, parallel to _sources.
        self._source_games: list[list[tuple[int, str]]] = [[] for _ in sources]
        self._source_etags: list[str | None] = [None for _ in sources]
        # Ids to rotate through: (source_index, game_id), from _build_rotation.
        self._rotation: list[tuple[int, str]] = []
        self._utc_offset_s: int | None = utc_offset_s
        self._current_index: int = 0
        self._last_rotation_ms: int | None = None
        self._consecutive_failures: int = 0
        self._first_failure_ms: int = 0
        self._animation_reset: bool = True
        self._locked: bool = False
        # League lock: rotation restricted to one source (by key, so a
        # config-driven source rebuild can't misdirect it). None = all.
        self._league_lock_key: str | None = None
        self._skip_requested: bool = False
        self._skip_league_requested: bool = False
        # True only for the duration of the tick that consumed a skip request.
        # skip()/skip_league() reject presses while this (or a pending
        # request) is set.
        self._skip_in_flight: bool = False
        # Stale-clock guard: (game_id, clock_seconds) of the last committed
        # soccer live view, compared on re-poll of the same game.
        self._prev_soccer_clock: tuple[str, int] | None = None
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
        if self._skip_requested or self._skip_league_requested or self._skip_in_flight:
            pulse_toast()
            return
        set_toast(sticky=True, kind=TOAST_SPINNER)
        self._skip_requested = True
        self._wake.set()

    def skip_league(self) -> None:
        """Jump to the next league's games now (button long-press).

        Same arm/reject semantics as skip(): one in-flight advance at a time,
        rejected presses dim the visible toast. With a single configured
        league this degrades to a normal skip.
        """
        if self._skip_requested or self._skip_league_requested or self._skip_in_flight:
            pulse_toast()
            return
        set_toast(sticky=True, kind=TOAST_SPINNER)
        self._skip_league_requested = True
        self._wake.set()

    def toggle_lock(self) -> None:
        """Toggle rotation lock (button input). The current game keeps polling."""
        self._locked = not self._locked
        # Non-sticky by default: a lock toast fired mid-skip must survive the
        # skip tick's clear_toast_if_sticky() teardown.
        set_toast(kind=TOAST_LOCK if self._locked else TOAST_UNLOCK)
        logger.debug(f"[POLL] rotation lock: {self._locked}")

    def toggle_league_lock(self) -> None:
        """Toggle league-only rotation (button long-press): games cycle
        within the current game's league instead of moving on to the next.
        Feedback is a text toast naming the league ("MLS ONLY")."""
        if self._league_lock_key is not None:
            self._league_lock_key = None
            set_toast("ALL LEAGUES")
        else:
            if not self._rotation:
                pulse_toast()
                return
            source = self._sources[self._rotation[self._current_index][0]]
            self._league_lock_key = source.key
            set_toast(f"{source.tag} ONLY")
        self._build_rotation()
        logger.debug(f"[POLL] league lock: {self._league_lock_key}")

    async def run(self) -> None:
        while True:
            try:
                await self._tick()
                if self._consecutive_failures > 0:
                    logger.error(
                        f"[POLL] recovered after {self._consecutive_failures} failed polls"
                    )
                self._consecutive_failures = 0
            except Exception as e:
                now = time.ticks_ms()
                if self._consecutive_failures == 0:
                    self._first_failure_ms = now
                self._consecutive_failures += 1
                logger.error(
                    f"[POLL] poll failed ({self._consecutive_failures}/{self.MAX_FAILURES}): "
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
        skip_league = self._skip_league_requested
        self._skip_requested = False
        self._skip_league_requested = False
        # A consumed skip owns the sticky spinner toast for exactly this
        # tick. The finally below tears it down on EVERY exit path — success,
        # empty slate, 404, or a mid-flight exception — so the toast's lifetime
        # is precisely the work it was announcing.
        if skip or skip_league:
            self._skip_in_flight = True
        try:
            rotation_due = (
                self._last_rotation_ms is not None
                and time.ticks_diff(now, self._last_rotation_ms)
                >= self._config.game_rotation_seconds * 1000
            )

            if self._last_rotation_ms is None:
                await self._refresh_lists(initial=True)
                self._current_index = 0
                self._last_rotation_ms = now
                self._animation_reset = True
            elif skip_league:
                await self._rotate(now, next_league=True)
                self._animation_reset = True
            elif skip or (rotation_due and not self._locked):
                await self._rotate(now)
                self._animation_reset = True
            else:
                self._animation_reset = False

            if not self._rotation:
                # Only a truly empty merged slate reaches here (a non-empty
                # slate always yields at least one rotation entry).
                state = get_write_state()
                state.mode = 'no_games'
                commit_state()
                return

            await self._poll_current()
        finally:
            if skip or skip_league:
                self._skip_in_flight = False
                clear_toast_if_sticky()

    async def _refresh_lists(self, initial: bool) -> None:
        """Refresh every source's game list and rebuild the merged rotation.

        A single source failing keeps its cached slate (a dead league feed
        must not blank the others); the tick only counts as failed — feeding
        the error screen — when EVERY source refresh fails.
        """
        last_error: Exception | None = None
        failures = 0
        for i in range(len(self._sources)):
            source = self._sources[i]
            try:
                if_none_match = None if initial else self._source_etags[i]
                status, games, etag = await self._api.get_game_list(
                    source.list_path(), if_none_match, source.tag
                )
                if status != 304:
                    self._source_games[i] = games
                    self._source_etags[i] = etag
            except Exception as e:
                failures += 1
                last_error = e
                logger.error(
                    f"[POLL] {source.tag} list refresh failed, keeping cached slate: "
                    f"{type(e).__name__}: {e}"
                )
        if self._sources and failures == len(self._sources) and last_error is not None:
            raise last_error

        self._build_rotation()
        total = 0
        for games in self._source_games:
            total += len(games)
        logger.debug(
            f"[POLL] lists refreshed: sources={len(self._sources)} "
            f"slate={total} rotation={len(self._rotation)}"
        )

    def _slate_rotation(self, only_source: int | None) -> list:
        """Live-first rotation over the merged slate, optionally restricted
        to one source index."""
        live: list[tuple[int, str]] = []
        finals: list[tuple[int, str]] = []
        pregames: list[tuple[int, str]] = []
        for i in range(len(self._sources)):
            if only_source is not None and i != only_source:
                continue
            for st, gid in self._source_games[i]:
                if st == GAME_STATE_IN:
                    live.append((i, gid))
                elif st == GAME_STATE_POST:
                    finals.append((i, gid))
                elif st == GAME_STATE_PRE:
                    pregames.append((i, gid))
        return live if live else finals + pregames

    def _build_rotation(self) -> None:
        """Rebuild the rotation from the merged slate, live-first.

        Live games (if any, across all leagues) rotate alone; otherwise
        finals rotate first, then pregames — leagues in configured order,
        backend order within a league. A league lock restricts the rotation
        to that source; a locked league whose slate empties falls back to
        the full rotation (the lock is kept — its games may return) rather
        than blanking the board. To avoid a mid-view jump when an unrelated
        game flips state, the currently-shown (source, id) keeps its
        position in the new rotation if still present, else the index
        resets to 0.
        """
        current = (
            self._rotation[self._current_index] if self._rotation else None
        )

        lock_idx: int | None = None
        if self._league_lock_key is not None:
            for i in range(len(self._sources)):
                if self._sources[i].key == self._league_lock_key:
                    lock_idx = i
                    break
            if lock_idx is None:
                # The locked league left the configured sources.
                self._league_lock_key = None

        rotation = self._slate_rotation(lock_idx)
        if not rotation and lock_idx is not None:
            logger.debug("[POLL] league-locked slate empty; falling back to all leagues")
            rotation = self._slate_rotation(None)

        self._rotation = rotation
        if current is not None:
            try:
                self._current_index = rotation.index(current)
            except ValueError:
                self._current_index = 0
        else:
            self._current_index = 0

    async def _rotate(self, now: int, next_league: bool = False) -> None:
        if next_league and self._league_lock_key is not None:
            # A league skip is an explicit "move on" — it escapes the lock.
            self._league_lock_key = None
        await self._refresh_lists(initial=False)
        n = len(self._rotation)
        if n:
            if next_league:
                # First entry of the next distinct league, scanning forward
                # cyclically; single-league slates degrade to a normal skip.
                cur_source = self._rotation[self._current_index][0]
                for step in range(1, n + 1):
                    idx = (self._current_index + step) % n
                    if self._rotation[idx][0] != cur_source:
                        self._current_index = idx
                        break
                else:
                    self._current_index = (self._current_index + 1) % n
            else:
                self._current_index = (self._current_index + 1) % n
        self._last_rotation_ms = now

    async def _poll_current(self) -> None:
        # Every tick re-fetches the current game — including static pre/post
        # screens. That standing re-poll is what lets a pregame card notice its
        # own pre->in flip mid-view (detail comes back as a live model -> the
        # board goes live without waiting for the next rotation). The screens
        # don't flicker on an unchanged re-commit: the setters only restamp
        # the animation clock when the displayed (mode, game_id) identity
        # changes, so a repeat commit preserves the scroll in progress.
        source_index, game_id = self._rotation[self._current_index]
        source = self._sources[source_index]
        detail = await self._api.get_game_state(
            source.detail_path(game_id), source.parse, source.tag
        )
        if detail is None:
            # 404 means the game left today's scoreboard between the list
            # refresh and this fetch; skip this slot and let the next rotation
            # pick up a fresh list.
            return

        # Abbreviations are present on all game states of every sport, so
        # logos are fetched the same way regardless of type. Cache keys are
        # league-namespaced (a soccer "POR" is not an MLB "POR").
        home_logo = await self._logos.get(
            source.logo_key(detail.home.abbreviation),
            source.logo_path(detail.home.abbreviation),
        )
        away_logo = await self._logos.get(
            source.logo_key(detail.away.abbreviation),
            source.logo_path(detail.away.abbreviation),
        )

        # Dispatch on the model's wire state; the sport-specific commits come
        # from the source. The else arm is the fail-loud guard for a state
        # code no parser produces today.
        ws = detail.wire_state
        if ws == GAME_STATE_IN:
            source.commit_live(self, game_id, detail, home_logo, away_logo)
        elif ws == GAME_STATE_PRE:
            set_pregame(detail, home_logo, away_logo, self._utc_offset_s)
        elif ws == GAME_STATE_POST:
            source.commit_final(detail, home_logo, away_logo)
        else:
            raise DeserializeError("@1", f"unhandled game detail {type(detail).__name__}")

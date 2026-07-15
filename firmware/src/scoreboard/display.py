"""
Display rendering and thread management for the Pico Scoreboard.

Provides render functions for non-game display modes (startup, idle,
no_games, setup, error), the logo caching system, animation primitives,
and the Core 1 display thread.

Render functions are pure readers: every string they draw was pre-built on
Core 0 when the state changed (see scoreboard/state.py), so the render loop
does no per-frame text formatting.
"""

import gc
import sys
import time
import framebuf
import micropython
from machine import Pin
from hub75 import Hub75Driver, Hub75Display, row_addressing
from hub75.native import pack_hsv_to_rgb565
from scoreboard.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, measure_text, ALIGN_LEFT, ALIGN_CENTER, ALIGN_RIGHT
from scoreboard.inning_half import TOP, BOTTOM
from scoreboard.state import (
    StateBuffer, ThreadHealth, UiColors, _team_color_to_rgb565,
    TOAST_TEXT, TOAST_LOCK, TOAST_UNLOCK, TOAST_SPINNER,
    TOAST_DISPLAY_MS, TOAST_STICKY_MAX_MS,
)
from scoreboard import screen_geometry
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient
import scoreboard.logger as logger
from scoreboard.logger import ERROR, DEBUG
from scoreboard.layout import field as field_sprite
from scoreboard.layout import base_marker as base_marker_sprite
from scoreboard.layout import dot as dot_sprite
from scoreboard.layout import inning_top as inning_top_sprite
from scoreboard.layout import inning_bottom as inning_bottom_sprite
from scoreboard.layout import first_base as first_base_loc
from scoreboard.layout import second_base as second_base_loc
from scoreboard.layout import third_base as third_base_loc
from scoreboard.layout import away_logo as away_logo_loc
from scoreboard.layout import home_logo as home_logo_loc
from scoreboard.layout import away_score as away_score_loc
from scoreboard.layout import home_score as home_score_loc
from scoreboard.layout import inning as inning_loc
from scoreboard.layout import ball_label as ball_label_loc
from scoreboard.layout import ball_values as ball_values_loc
from scoreboard.layout import strike_label as strike_label_loc
from scoreboard.layout import strike_values as strike_values_loc
from scoreboard.layout import out_label as out_label_loc
from scoreboard.layout import out_values as out_values_loc
from scoreboard.layout import pitcher_label as pitcher_label_loc
from scoreboard.layout import pitcher_name as pitcher_name_loc
from scoreboard.layout import batter_label as batter_label_loc
from scoreboard.layout import batter_name as batter_name_loc
from scoreboard.layout import play_text as play_text_loc
from scoreboard.layout import toast_spinner as toast_spinner_sprite
from scoreboard.layout import toast_lock_closed as toast_lock_closed_sprite
from scoreboard.layout import toast_lock_open as toast_lock_open_sprite

# Fixed colors
BLACK = 0
WHITE = rgb565(255, 255, 255)
DIM_GRAY = rgb565(96, 96, 96)

# _team_color_to_rgb565 / _TEAM_COLOR_MIN_CHANNEL live in scoreboard.state so
# Core 0 setters can pre-brighten team colors without importing display;
# render_game imports the helper (above) for its per-frame use.


def pulse(now_ms: int, period_ms: int = 1000) -> int:
    """Triangle-wave factor in [0, 256], cycling every `period_ms`.

    Integer math only — runs per frame on the display thread, and MicroPython
    floats are heap-allocated (the old sin() version churned garbage).
    Callers map the factor into whatever range they need — e.g.
    `V = 191 + ((pulse(now_ms) * 64) >> 8)` for a subtle 75%→100% sweep.
    """
    phase = (now_ms % period_ms) * 512 // period_ms  # 0..511
    return 512 - phase if phase > 256 else phase


# Display dimensions
DISPLAY_WIDTH = 128
DISPLAY_HEIGHT = 64

# Core 1 frame budget: 20 FPS. Scroll speeds must evenly divide 1000/FRAME_MS
# (see screen_geometry's scroll-speed note).
FRAME_MS = 50

# TEMPORARY (2026-07-11 GC/stutter investigation — remove when done): sample
# gc.mem_alloc() once per display tick and log a [MEMPROF] window summary to
# the RAM log ring every 10 s. Read the ring over USB afterwards; never over
# HTTP (that perturbs what we're measuring) and never at ERROR level (a
# flash flush from the ring would lock out this core and CAUSE stutter).
# The mem_alloc() call walks the heap ATB (~0.5 ms) — acceptable while
# profiling, not free.
MEM_PROFILE = True

# Whole-frame dim under icon toasts, with a short fade ladder on both edges.
# Each 32-bit word holds two RGB565 pixels; a brightness factor k/8 is a sum
# of masked shifts — (w>>1)&m1 [+ (w>>2)&m2] [+ (w>>3)&m3] — where each mask
# clears the bits that would bleed across the R5/G6/B5 field boundaries after
# that shift (m1 also clears the arithmetic shift's bit-31 sign extension).
# Channel sums can't overflow their fields because the total factor is < 1.
# Ladder: idx 0 = 7/8, 1 = 3/4, 2 = 5/8, 3 = 1/2 (the held level); fade-in
# walks 0→3 from toast start, fade-out walks 2→0 after expiry.
_DIM_WORDS = (DISPLAY_WIDTH * DISPLAY_HEIGHT * 2) // 4
_FADE_TERMS = ((1, 1), (1, 0), (0, 1), (0, 0))  # idx -> (t2, t3)
_TOAST_FADE_STEP_MS = 50  # one ladder step per 20 FPS frame
_TOAST_FADE_OUT_MS = _TOAST_FADE_STEP_MS * 3  # 5/8 → 3/4 → 7/8, then clean

if sys.implementation.name == "micropython":
    @micropython.viper
    def _dim_frame(buf, n_words: int, t2: int, t3: int):
        p = ptr32(buf)  # noqa: F821 — viper builtin
        # Masks built through variables: the full 32-bit literals overflow
        # MicroPython's 31-bit small int and box to objects (which viper
        # can't combine with native ints), and a one-line (a << 16) | b is
        # constant-folded by mpy-cross into that same boxed object.
        m1 = 0x7BEF
        m1 = (m1 << 16) | 0x7BEF
        m2 = 0x39E7
        m2 = (m2 << 16) | 0x39E7
        m3 = 0x18E3
        m3 = (m3 << 16) | 0x18E3
        for i in range(n_words):
            w = p[i]
            v = (w >> 1) & m1
            if t2 != 0:
                v += (w >> 2) & m2
            if t3 != 0:
                v += (w >> 3) & m3
            p[i] = v
else:
    # Desktop-preview body: micropython.viper is an identity decorator there
    # and ptr32 doesn't exist. Must stay mask-identical to the viper branch.
    def _dim_frame(buf, n_words: int, t2: int, t3: int) -> None:
        w32 = memoryview(buf).cast("I")
        for i in range(n_words):
            w = w32[i]
            v = (w >> 1) & 0x7BEF7BEF
            if t2:
                v += (w >> 2) & 0x39E739E7
            if t3:
                v += (w >> 3) & 0x18E318E3
            w32[i] = v

# Play-by-play flash: the most-recent play text preempts the pitcher/batter
# view after a new play is detected. Its display window is computed per play
# (see play_text_display_ms): exactly one scroll cycle — full start pause,
# scroll to the end, full end pause — so long plays get the time they need
# and short plays don't linger. The scroll speed itself is the shared,
# user-configurable screen_geometry.GAME_SCROLL_PX_PER_SEC.
PLAY_TEXT_SCROLL_PAUSE_MS = 1000

# TOAST_DISPLAY_MS / TOAST_STICKY_MAX_MS live in scoreboard.state (next to
# ToastState) so clear_toast_if_sticky can re-stamp without importing display.

# Rejected-press dim: a press during an in-flight skip dims the toast one
# triangle cycle (TOAST_PULSE_MS) toward TOAST_PULSE_DIP darkness, then back.
TOAST_PULSE_MS = 1000
TOAST_PULSE_DIP = 128

# Critical-count dot pulse (balls==3 / strikes==2 / outs==2). Brightness sweeps
# V_BASE..V_BASE+V_RANGE and saturation 0..S_MAX in lockstep off the same pulse
# so the dots warm from white toward a pale red at the peak. Module constants
# so the preview can sweep them.
CRITICAL_PULSE_V_BASE = 191
CRITICAL_PULSE_V_RANGE = 64
CRITICAL_PULSE_S_MAX = 80


# Single source of truth for the play-flash font: play_text_display_ms must
# measure with exactly the font render_game draws with, or the computed
# window won't match the actual scroll.
PLAY_TEXT_FONT = unscii_16


def play_text_display_ms(text: str) -> int:
    """
    Compute how long a play flash should stay on screen: one full scroll
    cycle of `calculate_scroll_offset` — start pause + scroll-to-end + end
    pause. Text that fits without scrolling shows for just the two pauses.

    Called on Core 0 (the poller) when a new play arrives; the result is
    stored in PlayState so Core 1 never measures text.
    """
    text_w = measure_text(text, PLAY_TEXT_FONT)
    max_scroll = text_w - play_text_loc.WIDTH
    scroll_ms = (max_scroll * 1000) // screen_geometry.GAME_SCROLL_PX_PER_SEC if max_scroll > 0 else 0
    # Mirrors calculate_scroll_offset's cycle: pause + scroll + pause.
    return PLAY_TEXT_SCROLL_PAUSE_MS + scroll_ms + PLAY_TEXT_SCROLL_PAUSE_MS


class Region(framebuf.FrameBuffer):
    """
    A sub-view framebuffer over a rectangular region of a parent display.

    Stride is computed from the parent's width, so writes into the Region
    clip to its bounds automatically — no manual fill_rect masking required.

    The parent must expose `.width` and `.buffer` (e.g. Hub75Display, or
    another Region — Regions can be nested). RGB565 is assumed.
    """

    def __init__(self, parent, x: int, y: int, width: int, height: int):
        parent_width = parent.width
        # RGB565 = 2 bytes per pixel
        offset = (y * parent_width + x) * 2
        view = memoryview(parent.buffer)[offset:]
        super().__init__(view, width, height, framebuf.RGB565, parent_width)
        self._view = view
        self._width = width
        self._height = height

    @property
    def buffer(self) -> memoryview:
        return self._view

    @property
    def width(self) -> int:
        return self._width

    @property
    def height(self) -> int:
        return self._height


class Regions:
    """
    Pre-allocated framebuffer regions for every text slot the scoreboard draws.

    Built once on Core 0 at display init and passed through to render
    functions. Avoids per-frame Region allocation on Core 1.

    Regions covering dynamic screens (startup, idle, no_games, setup, error)
    span across screen boundaries; rendering each screen fills the display
    with BLACK first, then draws into whichever regions that screen uses.

    Setup-mode text regions span the full display width even though a QR
    code may overlap part of that area on the right. render_setup draws text
    first and blits the QR on top, so QR visibility is preserved at the cost
    of potential truncation of very long SSIDs when a QR is present.
    """

    def __init__(self, display: Hub75Display):
        # Stored so update_for_qr() can rebuild setup regions against the same display.
        self._display = display

        # --- Game screen ---
        self.inning = Region(display, inning_loc.X, inning_loc.Y, inning_loc.WIDTH, inning_loc.HEIGHT)
        self.ball_label = Region(display, ball_label_loc.X, ball_label_loc.Y, ball_label_loc.WIDTH, ball_label_loc.HEIGHT)
        self.strike_label = Region(display, strike_label_loc.X, strike_label_loc.Y, strike_label_loc.WIDTH, strike_label_loc.HEIGHT)
        self.out_label = Region(display, out_label_loc.X, out_label_loc.Y, out_label_loc.WIDTH, out_label_loc.HEIGHT)
        self.pitcher_label = Region(display, pitcher_label_loc.X, pitcher_label_loc.Y, pitcher_label_loc.WIDTH, pitcher_label_loc.HEIGHT)
        self.batter_label = Region(display, batter_label_loc.X, batter_label_loc.Y, batter_label_loc.WIDTH, batter_label_loc.HEIGHT)
        self.pitcher_name = Region(display, pitcher_name_loc.X, pitcher_name_loc.Y, pitcher_name_loc.WIDTH, pitcher_name_loc.HEIGHT)
        self.batter_name = Region(display, batter_name_loc.X, batter_name_loc.Y, batter_name_loc.WIDTH, batter_name_loc.HEIGHT)
        self.play_text = Region(display, play_text_loc.X, play_text_loc.Y, play_text_loc.WIDTH, play_text_loc.HEIGHT)

        # --- Startup screen ---
        self.startup_title = Region(display, 0, 4, DISPLAY_WIDTH, 16)
        # Step counter sits to the right of the progress bar.
        # bar_x = (DISPLAY_WIDTH - 80) // 2 = 24, so step x = 24 + 80 + 4 = 108.
        self.startup_step = Region(display, 108, 24, DISPLAY_WIDTH - 108, 8)
        self.startup_operation = Region(display, 0, 42, DISPLAY_WIDTH, 8)
        self.startup_detail = Region(display, 0, 54, DISPLAY_WIDTH, 8)

        # --- Idle screen ---
        self.idle_title = Region(display, 0, 16, DISPLAY_WIDTH, 16)
        self.idle_subtitle = Region(display, 0, 40, DISPLAY_WIDTH, 8)

        # --- No Games screen ---
        self.no_games_title = Region(display, 0, 20, DISPLAY_WIDTH, 16)
        self.no_games_subtitle = Region(display, 0, 40, DISPLAY_WIDTH, 8)

        # --- Setup screen ---
        # Initialized at full width (no QR). Rebuilt via update_for_qr() on
        # Core 0 whenever the QR code is (re)generated so text never overlaps
        # the QR footprint. Lines whose y-range sits entirely below the QR
        # stay full width.
        self.setup_title = Region(display, 2, 0, DISPLAY_WIDTH - 2, 16)
        self.setup_line_18 = Region(display, 2, 18, DISPLAY_WIDTH - 2, 8)
        self.setup_line_28 = Region(display, 2, 28, DISPLAY_WIDTH - 2, 8)
        self.setup_line_44 = Region(display, 2, 44, DISPLAY_WIDTH - 2, 8)
        self.setup_line_54 = Region(display, 2, 54, DISPLAY_WIDTH - 2, 8)

        # --- Error screen ---
        self.error_title = Region(display, 0, 0, DISPLAY_WIDTH, 16)
        self.error_line_0 = Region(display, 0, 24, DISPLAY_WIDTH, 8)
        self.error_line_1 = Region(display, 0, 34, DISPLAY_WIDTH, 8)
        self.error_line_2 = Region(display, 0, 44, DISPLAY_WIDTH, 8)
        self.error_line_3 = Region(display, 0, 54, DISPLAY_WIDTH, 8)

        # --- Pregame / final / soccer screens ---
        # Built from the active screen_geometry variant tables (name ->
        # Region). Scalar entries (DIVIDER_X, SEPARATOR_Y) are read straight
        # from the table by the renderer; only (X, Y, W, H) rects become
        # Regions. Rebuilt at runtime when the configured variants change.
        self.rebuild_variant_regions()

    def rebuild_variant_regions(self) -> None:
        """(Re)build the variant-driven region tables (pregame / final /
        soccer) from the ACTIVE screen_geometry selections.

        MUST be called on Core 0 only (same contract as update_for_qr).
        Each table is built fresh and published with a single attribute
        store, so the display thread sees either the old dict or the new
        one — never a half-built table. A frame that latches the new
        geometry constants against an old region dict can throw once; the
        render loop's per-frame try/except absorbs it and the next frame is
        consistent.
        """
        display = self._display
        # One region dict per sport x screen key. Keys whose active tables
        # are the same object (sports still sharing a design) share one
        # built dict, so the split costs no extra Region objects until a
        # design actually diverges.
        built: dict = {}
        by_table: dict = {}
        for key in screen_geometry.variant_keys():
            table = screen_geometry.geometry_for(key)
            tid = id(table)
            if tid not in by_table:
                by_table[tid] = self._build_geometry_regions(display, table)
            built[key] = by_table[tid]
        self.variant = built

    @staticmethod
    def _build_geometry_regions(display, table: dict) -> dict:
        regions = {}
        for name in table:
            spec = table[name]
            if isinstance(spec, tuple) and len(spec) == 4:
                x, y, w, h = spec
                regions[name] = Region(display, x, y, w, h)
        return regions

    def update_for_qr(self, qr_width: int, qr_height: int) -> None:
        """
        Recompute setup-screen text regions so they never overlap the QR code.

        MUST be called on Core 0 only. Invoke after set_setup_mode()
        regenerates the QR (or clears it). Lines whose vertical range
        intersects the QR get narrowed to end 4px before the QR's left edge;
        lines fully below the QR stay at full width.

        Args:
            qr_width: QR framebuffer width in px (includes quiet zone). 0 = no QR.
            qr_height: QR framebuffer height in px (includes quiet zone).
        """
        display = self._display
        left_pad = 2
        full_width = DISPLAY_WIDTH - left_pad

        if qr_width <= 0:
            narrow_width = full_width
            qr_top = 0
            qr_bottom = 0
        else:
            # QR sits at (DISPLAY_WIDTH - qr_width - 2, 2); matches render_setup's blit.
            qr_top = 2
            qr_bottom = qr_top + qr_height
            qr_x = DISPLAY_WIDTH - qr_width - 2
            # 4px visual gap between text's right edge and QR's left edge.
            narrow_width = qr_x - 4 - left_pad
            if narrow_width < 0:
                narrow_width = 0

        def width_for(y_top: int, height: int) -> int:
            y_bottom = y_top + height
            if qr_bottom > 0 and y_top < qr_bottom and y_bottom > qr_top:
                return narrow_width
            return full_width

        self.setup_title = Region(display, left_pad, 0, width_for(0, 16), 16)
        self.setup_line_18 = Region(display, left_pad, 18, width_for(18, 8), 8)
        self.setup_line_28 = Region(display, left_pad, 28, width_for(28, 8), 8)
        self.setup_line_44 = Region(display, left_pad, 44, width_for(44, 8), 8)
        self.setup_line_54 = Region(display, left_pad, 54, width_for(54, 8), 8)


def _draw_count_dots(display: Hub75Display, slice_mod: object, filled_count: int, filled_color: int | None = None) -> None:
    n_dots = (slice_mod.WIDTH + 1) // (dot_sprite.WIDTH + 1)  # type: ignore[attr-defined]
    default_outline = dot_sprite.palette.pixel(1, 0)
    default_fill = dot_sprite.palette.pixel(2, 0)
    # When `filled_color` is provided, every dot's outline tracks it (so the
    # ring on unfilled dots also pulses) and filled dots' interiors match,
    # keeping the whole dot reading as one color.
    active = filled_color if filled_color is not None else default_outline
    try:
        dot_sprite.palette.pixel(1, 0, active)
        for i in range(n_dots):
            dot_sprite.palette.pixel(2, 0, active if i < filled_count else default_fill)
            display.blit(
                dot_sprite.data,
                slice_mod.X + i * (dot_sprite.WIDTH + 1),  # type: ignore[attr-defined]
                slice_mod.Y,  # type: ignore[attr-defined]
                dot_sprite.KEY,
                dot_sprite.palette  # type: ignore
            )
    finally:
        # The palette is shared module state — always restore it, even if a
        # blit throws, so later frames don't render with the pulsed color.
        dot_sprite.palette.pixel(1, 0, default_outline)
        dot_sprite.palette.pixel(2, 0, default_fill)


# Default base-marker palette entries (the original gold ball), captured at
# import so the per-frame restore never re-reads a mutated palette.
_BASE_MARKER_DEFAULT_1 = base_marker_sprite.palette.pixel(1, 0)  # ball body
_BASE_MARKER_DEFAULT_2 = base_marker_sprite.palette.pixel(2, 0)  # highlight
_BASE_MARKER_DEFAULT_3 = base_marker_sprite.palette.pixel(3, 0)  # edge shade

# [packed RGB888 key, ball565, highlight565, shade565]; key -1 = empty.
# Mutated in place — the batting team changes at most once per half-inning,
# so steady-state frames are allocation-free.
_base_pal = [-1, 0, 0, 0]


def _base_marker_colors(packed: int) -> list:
    """Base-marker palette derived from the batting team's primary color,
    memoized on the packed RGB888. Relationships match the original gold
    sprite: highlight = 7/8 blend toward white, edge shade = 7/8 of the
    ball color. Integer math only (see pulse() on float churn)."""
    c = _base_pal
    if c[0] != packed:
        r = (packed >> 16) & 0xFF
        g = (packed >> 8) & 0xFF
        b = packed & 0xFF
        # Same brightening policy as _team_color_to_rgb565 (state.py), but we
        # need the brightened RGB888 channels here to derive the shades.
        m = r if r >= g and r >= b else (g if g >= b else b)
        if m < 128:
            if m == 0:
                r = g = b = 128
            else:
                r = r * 128 // m
                g = g * 128 // m
                b = b * 128 // m
        c[0] = packed
        c[1] = rgb565(r, g, b)
        c[2] = rgb565(r + ((255 - r) * 7 >> 3), g + ((255 - g) * 7 >> 3), b + ((255 - b) * 7 >> 3))
        c[3] = rgb565(r * 7 >> 3, g * 7 >> 3, b * 7 >> 3)
    return c


def _draw_base_markers(display: Hub75Display, bases, packed: int) -> None:
    """Blit occupied-base markers. `packed` is the batting team's RGB888
    primary, or -1 (MIDDLE/END half) to keep the default gold palette."""
    pal = base_marker_sprite.palette
    if packed >= 0:
        c = _base_marker_colors(packed)
        pal.pixel(1, 0, c[1])
        pal.pixel(2, 0, c[2])
        pal.pixel(3, 0, c[3])
    try:
        if bases.first:
            display.blit(base_marker_sprite.data, first_base_loc.X, first_base_loc.Y, base_marker_sprite.KEY, pal)  # type: ignore
        if bases.second:
            display.blit(base_marker_sprite.data, second_base_loc.X, second_base_loc.Y, base_marker_sprite.KEY, pal)  # type: ignore
        if bases.third:
            display.blit(base_marker_sprite.data, third_base_loc.X, third_base_loc.Y, base_marker_sprite.KEY, pal)  # type: ignore
    finally:
        if packed >= 0:
            pal.pixel(1, 0, _BASE_MARKER_DEFAULT_1)
            pal.pixel(2, 0, _BASE_MARKER_DEFAULT_2)
            pal.pixel(3, 0, _BASE_MARKER_DEFAULT_3)


# =============================================================================
# Logo buffer pool
# =============================================================================

class LogoPool:
    """
    Pre-allocated pool of logo framebuffers with LRU eviction.

    A fixed number of RGB565 buffers are allocated once at construction;
    fetching a new logo copies the backend's raw bytes into a free slot (or
    evicts the least-recently-used one). Repeated allocations would fragment
    the MicroPython heap, hence the pool.

    Concurrency contract: get() must only be called from ONE sequential
    caller (the poller task). The LRU bookkeeping is mutated across an
    `await`, so concurrent callers could evict a slot that another call is
    still filling. A cache re-check after the fetch guards the same-key
    double-fetch case, but interleaved different-key callers are not
    supported.
    """

    def __init__(self, api_client: ScoreboardApiClient, size: int = 8,
                 width: int = 24, height: int = 24) -> None:
        self._api = api_client
        self._width = width
        self._height = height
        self._size = size
        buffer_bytes = width * height * 2  # RGB565
        self._buffers = [bytearray(buffer_bytes) for _ in range(size)]
        self._cache = {}      # cache_key -> (slot_index, FrameBuffer)
        self._lru = []        # LRU order: oldest first
        self._free_slots = set(range(size))
        logger.debug(f"[DISPLAY] logo pool initialized: {size} buffers ({size * buffer_bytes // 1024} KB)")

    async def get(self, cache_key: str, path: str) -> framebuf.FrameBuffer | None:
        """
        Get a logo framebuffer from the cache, fetching from the API on miss.

        Args:
            cache_key: Stable key for this logo in the LRU cache.
            path: Backend URL path that returns the raw logo bytes.
        """
        key = cache_key.lower()

        cached = self._cache.get(key)
        if cached is not None:
            self._lru.remove(key)
            self._lru.append(key)
            return cached[1]

        # Need to fetch - get a buffer slot
        if self._free_slots:
            slot_index = self._free_slots.pop()
        else:
            evict_key = self._lru.pop(0)
            slot_index = self._cache[evict_key][0]
            del self._cache[evict_key]
            logger.debug(f"[LOGO] evicted: key={evict_key} slot={slot_index}/{self._size}")

        try:
            status, body = await self._api.get_team_logo_raw(
                path=path,
                width=self._width,
                height=self._height,
                background_color="000000",
                accept="image/x-rgb565"
            )

            # Re-check after the await: if a same-key fetch completed while
            # we were suspended, return its result and free our slot instead
            # of leaking it by overwriting the cache entry.
            cached = self._cache.get(key)
            if cached is not None:
                self._free_slots.add(slot_index)
                return cached[1]

            if status != 200:
                logger.error(f"[LOGO] fetch failed: key={key} status={status}")
                self._free_slots.add(slot_index)
                return None

            buf = self._buffers[slot_index]
            buf[:len(body)] = body
            fb = framebuf.FrameBuffer(buf, self._width, self._height, framebuf.RGB565)

            self._cache[key] = (slot_index, fb)
            self._lru.append(key)
            logger.debug(f"[LOGO] cached: key={key} slot={slot_index}/{self._size}")
            return fb

        except Exception as e:
            logger.error(f"[LOGO] fetch error: key={key} error_type={type(e).__name__} {e}")
            self._free_slots.add(slot_index)
            return None


def init_display(config: Config) -> tuple[Hub75Driver, Hub75Display, FontWriter, Regions]:
    """
    Initialize and return HUB75 display hardware.

    Returns:
        Tuple of (driver, display, writer, regions)
    """
    data_freq = config.data_frequency_hz
    brightness = config.brightness / 100.0
    gamma = config.gamma
    blanking_time = config.blanking_time_ns
    target_refresh_rate = config.target_refresh_rate

    driver = Hub75Driver(
        row_addressing=row_addressing.Binary(
            base_pin=Pin(11, Pin.OUT),
            bit_count=5
        ),
        shift_register_depth=128,
        output_enable_pin=Pin(28, Pin.OUT),
        base_clock_pin=Pin(26, Pin.OUT),
        base_data_pin=Pin(16, Pin.OUT),
        data_frequency=data_freq,
        brightness=brightness,
        gamma=gamma,
        blanking_time=blanking_time,
        target_refresh_rate=target_refresh_rate
    )
    display = Hub75Display(driver)
    writer = FontWriter(display, default_font=unscii_8)
    regions = Regions(display)
    return driver, display, writer, regions


def draw_progress_bar(display: Hub75Display, x: int, y: int, width: int, height: int, progress: int, colors: UiColors) -> None:
    """Draw a horizontal progress bar."""
    # Border
    display.rect(x, y, width, height, colors.secondary)
    # Fill (leave 1px border)
    fill_width = int((width - 2) * progress / 100)
    if fill_width > 0:
        display.fill_rect(x + 1, y + 1, fill_width, height - 2, colors.accent)


# =============================================================================
# Render functions for each display mode
# =============================================================================

class _startup_dots_loc:
    """WiFi attempt dots, centered in the gap between the progress bar
    (ends y=31) and the operation line (y=42). Sized for exactly 3 dots —
    coupled to max_retries=3 in main.start_station_mode."""
    WIDTH = 3 * (dot_sprite.WIDTH + 1) - 1
    X = (DISPLAY_WIDTH - WIDTH) // 2
    Y = 34


def render_startup(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors) -> None:
    """Render startup/boot progress screen."""
    display.fill(BLACK)

    startup = state.startup

    writer.draw(regions.startup_title, "BOOTING", unscii_16, ALIGN_CENTER, 0, colors.accent)

    # Progress bar (80px wide, centered) at Y=24. Completing the final step
    # fills the bar to 100%.
    bar_width = 80
    bar_x = (DISPLAY_WIDTH - bar_width) // 2
    progress = startup.step * 100 // startup.total_steps
    draw_progress_bar(display, bar_x, 24, bar_width, 8, progress, colors)

    if startup.attempts_total > 0:
        _draw_count_dots(display, _startup_dots_loc, startup.attempt)

    writer.draw(regions.startup_step, startup.step_text, spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
    writer.draw(regions.startup_operation, startup.operation, spleen_5x8, ALIGN_CENTER, 0, colors.primary)
    if startup.detail:
        writer.draw(regions.startup_detail, startup.detail, spleen_5x8, ALIGN_CENTER, 0, colors.secondary)


def render_updating(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors) -> None:
    """Render OTA update screen: download progress, then restart countdown.

    Reuses the startup regions — the geometry is identical and the two modes
    can never coexist ('updating' is only entered long after finish_startup).
    """
    display.fill(BLACK)

    updating = state.updating

    writer.draw(regions.startup_title, "UPDATING", unscii_16, ALIGN_CENTER, 0, colors.accent)

    bar_width = 80
    bar_x = (DISPLAY_WIDTH - bar_width) // 2
    draw_progress_bar(display, bar_x, 24, bar_width, 8, updating.progress, colors)

    if updating.percent_text:
        writer.draw(regions.startup_step, updating.percent_text, spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
    writer.draw(regions.startup_operation, updating.phase, spleen_5x8, ALIGN_CENTER, 0, colors.primary)
    if updating.detail:
        writer.draw(regions.startup_detail, updating.detail, spleen_5x8, ALIGN_CENTER, 0, colors.secondary)


def render_idle(display: Hub75Display, writer: FontWriter, regions: Regions, colors: UiColors) -> None:
    """Render idle/waiting screen."""
    display.fill(BLACK)
    writer.draw(regions.idle_title, "PICO", unscii_16, ALIGN_CENTER, 0, colors.primary)
    writer.draw(regions.idle_subtitle, "SCOREBOARD", unscii_8, ALIGN_CENTER, 0, colors.accent)


def render_no_games(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """Render no games scheduled screen."""
    display.fill(BLACK)
    writer.draw(regions.no_games_title, "NO GAMES", unscii_16, ALIGN_CENTER, 0, colors.primary)
    writer.draw(regions.no_games_subtitle, "scheduled", spleen_5x8, ALIGN_CENTER, 0, colors.secondary)
    _render_toast(writer, regions, state, now_ms)
    _render_toast_overlay(display, state, now_ms)


def render_setup(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """
    Render setup mode screen with WiFi QR code and contextual information.

    Text lines were pre-built by set_setup_mode; this function only picks
    colors (by reason) and draws. Text is drawn first into full-width
    regions, then the QR (if available) is blitted on top.
    """
    display.fill(BLACK)

    setup = state.setup
    elapsed_ms = time.ticks_diff(now_ms, state.animation_start_ms)

    is_failure = setup.reason == 'bad_auth' or setup.reason == 'connection_failed'
    title_color = colors.clock_warning if is_failure else colors.accent
    line_54_color = colors.accent

    writer.draw(regions.setup_title, setup.title, unscii_16, ALIGN_LEFT, 0, title_color)
    writer.draw(regions.setup_line_18, setup.line_18, spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.primary)
    writer.draw(regions.setup_line_28, setup.line_28, spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.secondary)
    writer.draw(regions.setup_line_44, setup.line_44, spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
    writer.draw(regions.setup_line_54, setup.line_54, spleen_5x8, ALIGN_LEFT, 0, line_54_color)

    # QR on top so it stays readable even if text drew underneath it.
    qr_fb = setup.qr_fb
    qr_width = setup.qr_width
    qr_palette = setup.qr_palette
    if qr_fb is not None and qr_palette is not None and qr_width > 0:
        qr_x = DISPLAY_WIDTH - qr_width - 2
        display.blit(qr_fb, qr_x, 2, -1, qr_palette)  # type: ignore


def render_error(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors) -> None:
    """Render error screen with multi-line details (pre-truncated by set_error)."""
    display.fill(BLACK)

    error = state.error

    writer.draw(regions.error_title, error.title or 'ERROR', unscii_16, ALIGN_CENTER, 0, colors.clock_warning)

    line_regions = (regions.error_line_0, regions.error_line_1, regions.error_line_2, regions.error_line_3)
    for i in range(len(error.lines)):
        writer.draw(line_regions[i], error.lines[i], spleen_5x8, ALIGN_CENTER, 0, colors.primary)


def _toast_active(state: StateBuffer, now_ms: int) -> bool:
    toast = state.toast
    if toast.updated_ms == 0:
        return False
    if toast.kind == TOAST_TEXT and not toast.text:
        return False
    window = TOAST_STICKY_MAX_MS if toast.sticky else TOAST_DISPLAY_MS
    return time.ticks_diff(now_ms, toast.updated_ms) < window


def _toast_overlay_fading(state: StateBuffer, now_ms: int) -> bool:
    """True while an expired icon toast's dim is still fading back out.
    The render loop must keep rendering static modes through this tail."""
    toast = state.toast
    if toast.kind == TOAST_TEXT or toast.updated_ms == 0:
        return False
    window = TOAST_STICKY_MAX_MS if toast.sticky else TOAST_DISPLAY_MS
    elapsed = time.ticks_diff(now_ms, toast.updated_ms)
    return window <= elapsed < window + _TOAST_FADE_OUT_MS


def _toast_dim_v(state: StateBuffer, now_ms: int) -> int:
    """Brightness 0..255 for the toast, honoring a rejected-press dim cycle
    (toast.pulse_ms set): one triangle dip toward TOAST_PULSE_DIP darkness."""
    toast = state.toast
    if toast.pulse_ms != 0:
        elapsed = time.ticks_diff(now_ms, toast.pulse_ms)
        if 0 <= elapsed < TOAST_PULSE_MS:
            tri = pulse(elapsed, TOAST_PULSE_MS)  # 0..256..0 over the cycle
            return 255 - ((tri * TOAST_PULSE_DIP) >> 8)
    return 255


def _render_toast(writer: FontWriter, regions: Regions, state: StateBuffer, now_ms: int) -> bool:
    """Draw an active TEXT toast into the bottom strip. Returns True if drawn
    (the caller then skips its own bottom-strip content). Icon-kind toasts
    render via _render_toast_overlay instead and don't consume the strip."""
    if state.toast.kind != TOAST_TEXT or not _toast_active(state, now_ms):
        return False
    v = _toast_dim_v(state, now_ms)
    color = WHITE if v == 255 else rgb565(v, v, v)
    regions.play_text.fill(BLACK)
    writer.draw(regions.play_text, state.toast.text, unscii_16, ALIGN_CENTER, 0, color)
    return True


# --- Icon toast overlay (centered) -------------------------------------------
# Compiled sprites (tools/gen_toast_icons.py -> compile_layout.py) blitted
# with KEY transparency directly over screen content; drawn LAST in each
# game-facing render so nothing paints over it.

_CX = DISPLAY_WIDTH // 2   # 64
_CY = DISPLAY_HEIGHT // 2  # 32

# Skip spinner: a comet of 12 dots on a radius-12 ring, one revolution per
# _SPINNER_PERIOD_MS. The head position is computed in 1/256ths of a dot
# step, so brightness shifts every 50 ms frame — the fluidity is the point
# (it demos the 20 FPS pipeline). The dots live in one sprite, each dot its
# own palette index; per frame the 12 entries are rewritten with the comet
# gradient (or KEY for the gap dots, which the blit then skips).
_SPINNER_PERIOD_MS = 1000
_SPINNER_TRAIL = 10  # dots of fading tail behind the head (2-dot gap)

_SPINNER_X = _CX - toast_spinner_sprite.WIDTH // 2    # 51
_SPINNER_Y = _CY - toast_spinner_sprite.HEIGHT // 2   # 20
_LOCK_X = _CX - toast_lock_closed_sprite.WIDTH // 2   # 57
_LOCK_Y = 19  # sprite row 12 (lock body top) lands at y=31

# gen_toast_icons.py bakes dot k's color so its RGB565 value == k + 1, while
# compile_layout.py assigns palette indices in row-major first-seen order.
# Invert the baked palette once at import (Core 0, before the render thread
# overwrites the entries): _SPINNER_PAL[angular index] -> palette index.
# Contract drift (recolored dots, wrong count) raises here, not mid-render.
_pal = [0] * 12
for _p in range(1, 13):
    _pal[toast_spinner_sprite.palette.pixel(_p, 0) - 1] = _p
_SPINNER_PAL = tuple(_pal)
del _pal, _p


def _draw_spinner(display: Hub75Display, elapsed_ms: int, dim_v: int) -> None:
    pal = toast_spinner_sprite.palette
    key = toast_spinner_sprite.KEY
    # Head position in 1/256 dot units, advancing continuously.
    head = (elapsed_ms % _SPINNER_PERIOD_MS) * (12 * 256) // _SPINNER_PERIOD_MS
    span = _SPINNER_TRAIL * 256
    for i in range(12):
        d = (head - i * 256) % (12 * 256)  # angular lag behind the head
        if d >= span:
            pal.pixel(_SPINNER_PAL[i], 0, key)  # maps to KEY -> transparent
            continue
        v = 255 - (d * 255) // span
        if dim_v != 255:
            v = (v * dim_v) >> 8
        pal.pixel(_SPINNER_PAL[i], 0, rgb565(v, v, v))
    # No palette restore (unlike _draw_count_dots): this sprite has a single
    # owner and every entry is unconditionally rewritten each frame.
    display.blit(toast_spinner_sprite.data, _SPINNER_X, _SPINNER_Y, key, pal)


def _draw_lock(display: Hub75Display, color: int, is_open: bool) -> None:
    """A padlock centered on the panel: 14x10 body, 2px-thick shackle.

    Closed: the shackle meets the body on both sides. Open: the shackle is
    lifted up — its right leg stays anchored in the body, its left leg
    leaves a gap. Both sprites share a canvas so one blit position serves.
    """
    spr = toast_lock_open_sprite if is_open else toast_lock_closed_sprite
    spr.palette.pixel(1, 0, color)  # sole owner; rewritten before every blit
    display.blit(spr.data, _LOCK_X, _LOCK_Y, spr.KEY, spr.palette)


def _render_toast_overlay(display: Hub75Display, state: StateBuffer, now_ms: int) -> None:
    """Draw an active icon toast (lock / unlock / spinner) centered over the
    screen. Called LAST in each game-facing render function. Text toasts are
    handled by _render_toast in the bottom strip.

    The composed frame under the icon fades down the dim ladder to half
    brightness (so the icon reads against busy/white backgrounds) and back
    up after the toast expires — dim only, no icon — so the overlay eases
    in and out instead of snapping."""
    toast = state.toast
    if toast.kind == TOAST_TEXT or toast.updated_ms == 0:
        return
    elapsed = time.ticks_diff(now_ms, toast.updated_ms)
    if elapsed < 0:
        elapsed = 0
    window = TOAST_STICKY_MAX_MS if toast.sticky else TOAST_DISPLAY_MS
    if elapsed >= window:
        # Fade-out tail: walk the ladder back up; the icon is already gone.
        idx = 2 - (elapsed - window) // _TOAST_FADE_STEP_MS
        if idx >= 0:
            t = _FADE_TERMS[idx]
            _dim_frame(display.buffer, _DIM_WORDS, t[0], t[1])
        return
    idx = elapsed // _TOAST_FADE_STEP_MS
    if idx > 3:
        idx = 3
    t = _FADE_TERMS[idx]
    _dim_frame(display.buffer, _DIM_WORDS, t[0], t[1])
    dim_v = _toast_dim_v(state, now_ms)
    if toast.kind == TOAST_SPINNER:
        _draw_spinner(display, elapsed, dim_v)
    else:
        color = WHITE if dim_v == 255 else rgb565(dim_v, dim_v, dim_v)
        _draw_lock(display, color, toast.kind == TOAST_UNLOCK)


def render_game(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    display.fill(BLACK)

    mlv = state.mlb_live
    if not mlv.game_id:
        render_idle(display, writer, regions, colors)
        _render_toast(writer, regions, state, now_ms)
        _render_toast_overlay(display, state, now_ms)
        return

    # --- Dividers (shared style with the pregame/final screens) ---
    if screen_geometry.SHOW_DIVIDERS:
        display.vline(screen_geometry.LIVE_DIVIDER_X, 0, DISPLAY_HEIGHT, DIM_GRAY)
        display.hline(
            screen_geometry.LIVE_DIVIDER_X + 1,
            screen_geometry.LIVE_SEPARATOR_Y,
            DISPLAY_WIDTH - screen_geometry.LIVE_DIVIDER_X - 1,
            DIM_GRAY,
        )

    # --- Sprites ---

    display.blit(field_sprite.data, field_sprite.X, field_sprite.Y, field_sprite.KEY, field_sprite.palette)  # type: ignore

    # Base markers take the batting team's color (top: away bats, bottom:
    # home bats); transition halves keep the default gold.
    half = mlv.half
    _draw_base_markers(display, mlv.bases, mlv.batting_packed)

    if state.away_logo is not None:
        display.blit(state.away_logo, away_logo_loc.X, away_logo_loc.Y)
    if state.home_logo is not None:
        display.blit(state.home_logo, home_logo_loc.X, home_logo_loc.Y)

    if half is TOP:
        display.blit(inning_top_sprite.data, inning_top_sprite.X, inning_top_sprite.Y, inning_top_sprite.KEY, inning_top_sprite.palette)  # type: ignore
    elif half is BOTTOM:
        display.blit(inning_bottom_sprite.data, inning_bottom_sprite.X, inning_bottom_sprite.Y, inning_bottom_sprite.KEY, inning_bottom_sprite.palette)  # type: ignore

    # --- Count dots ---

    balls_critical = mlv.balls == 3
    strikes_critical = mlv.strikes == 2
    outs_critical = mlv.outs == 2

    if balls_critical or strikes_critical or outs_critical:
        p = pulse(view_elapsed_ms)
        v = CRITICAL_PULSE_V_BASE + ((p * CRITICAL_PULSE_V_RANGE) >> 8)
        s = (p * CRITICAL_PULSE_S_MAX) >> 8
        pulsed = pack_hsv_to_rgb565(0, s, v)
    else:
        pulsed = None

    _draw_count_dots(display, ball_values_loc, mlv.balls, pulsed if balls_critical else None)
    _draw_count_dots(display, strike_values_loc, mlv.strikes, pulsed if strikes_critical else None)
    _draw_count_dots(display, out_values_loc, mlv.outs, pulsed if outs_critical else None)

    # --- Text ---
    # Scores stay on the zero-alloc integer() path.
    writer.integer(mlv.away_score, away_score_loc.X, away_score_loc.Y, away_score_loc.WIDTH, ALIGN_CENTER, WHITE, font=unscii_16)
    writer.integer(mlv.home_score, home_score_loc.X, home_score_loc.Y, home_score_loc.WIDTH, ALIGN_CENTER, WHITE, font=unscii_16)

    writer.draw(regions.inning, mlv.inning_text, unscii_8, ALIGN_CENTER, 0, WHITE)

    writer.draw(regions.ball_label, "B", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)
    writer.draw(regions.strike_label, "S", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)
    writer.draw(regions.out_label, "O", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)

    # Colors were half-resolved to rgb565 at commit; -1 = between halves.
    pitch_color = mlv.pitch_color if mlv.pitch_color >= 0 else DIM_GRAY
    bat_color = mlv.bat_color if mlv.bat_color >= 0 else DIM_GRAY

    # Bottom strip priority: toast (button feedback) > play flash > pitcher/batter.
    if not _render_toast(writer, regions, state, now_ms):
        play = state.play
        # Visibility window on the wall rail (a stall consumes it); the scroll
        # offset below rides the frame rail (a stall stretches it).
        play_window_ms = time.ticks_diff(now_ms, play.updated_ms)
        show_play = bool(play.text) and play.updated_ms != 0 and play_window_ms < play.display_ms

        if show_play:
            # No glyph fallback: fit_play_text + the wire-cap-sized pool
            # make the strip an invariant (glyph-looping a long line halved
            # the frame rate and the visible scroll speed with it).
            writer.draw_strip(
                regions.play_text, play.strip,
                ALIGN_LEFT, play_elapsed_ms, WHITE,
                pause_ms=PLAY_TEXT_SCROLL_PAUSE_MS,
                pixels_per_second=screen_geometry.GAME_SCROLL_PX_PER_SEC,
            )
        else:
            if mlv.has_at_bat:
                elapsed_ms = view_elapsed_ms
                _text_or_strip(writer, regions.pitcher_name, mlv.pitcher_text,
                               mlv.pitcher_strip, ALIGN_CENTER, elapsed_ms, pitch_color,
                               PLAY_TEXT_SCROLL_PAUSE_MS,
                               screen_geometry.GAME_SCROLL_PX_PER_SEC)
                _text_or_strip(writer, regions.batter_name, mlv.batter_text,
                               mlv.batter_strip, ALIGN_CENTER, elapsed_ms, bat_color,
                               PLAY_TEXT_SCROLL_PAUSE_MS,
                               screen_geometry.GAME_SCROLL_PX_PER_SEC)

            writer.draw(regions.pitcher_label, "PIT", unscii_8, ALIGN_LEFT, 0, pitch_color)
            writer.draw(regions.batter_label, "BAT", unscii_8, ALIGN_LEFT, 0, bat_color)

    _render_toast_overlay(display, state, now_ms)


# _cycle_phase writes into this preallocated slot list instead of returning
# a fresh tuple — it runs per frame in the pregame renderers, and the old
# tuple return was the one allocation its "allocation-free" docstring missed.
_CYCLE_OUT = [0, 0, 0]


def _cycle_phase(ends: list, elapsed_ms: int) -> tuple:
    """Locate the active phase in a cumulative-dwell list.

    Fills and returns _CYCLE_OUT as [index, phase_start_ms, position_ms] where position is `elapsed`
    wrapped into one full cycle and phase_start is the cumulative dwell before
    the active phase. Allocation-free scan over the (<=3-entry) list; callers
    pass `position - phase_start` to writer.draw as the per-phase scroll clock.
    """
    total = ends[-1]
    pos = elapsed_ms % total
    start = 0
    for i in range(len(ends)):
        if pos < ends[i]:
            _CYCLE_OUT[0] = i; _CYCLE_OUT[1] = start; _CYCLE_OUT[2] = pos
        return _CYCLE_OUT
        start = ends[i]
    return len(ends) - 1, start, pos


def _text_or_strip(writer, region, text, strip, align, elapsed, color, pause, pxs):
    """Draw scrolling-capable text: strip fast path (one blit), per-glyph
    fallback when the text out-sized its pool. Identical placement math
    either way. Module-level (not a closure) to keep render frames
    allocation-free."""
    if strip is not None:
        writer.draw_strip(region, strip, align, elapsed, color,
                          pause_ms=pause, pixels_per_second=pxs)
    else:
        writer.draw(region, text, spleen_5x8, align, elapsed, color,
                    pause_ms=pause, pixels_per_second=pxs)


def render_pregame(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int) -> None:
    """Render the pregame screen for the active screen_geometry variant.

    Logos identify the teams (no abbreviations). Records sit beside the logos;
    the right column carries venue / first-pitch / weather (cycling in A/C,
    all-visible in B) over the probable pitchers in team colors.
    """
    display.fill(BLACK)

    pv = state.pregame
    geo = screen_geometry.geometry_for(pv.variant_key)
    variant = screen_geometry.active_variant(pv.variant_key)
    R = regions.variant[pv.variant_key]
    elapsed = view_elapsed_ms  # frame rail: motion holds, never jumps

    # --- Logos ---
    if state.away_logo is not None and "LOGO_AWAY" in geo:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None and "LOGO_HOME" in geo:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

    # --- Dividers ---
    if screen_geometry.SHOW_DIVIDERS:
        if "DIVIDER_X" in geo:
            display.vline(geo["DIVIDER_X"], 0, DISPLAY_HEIGHT, DIM_GRAY)
        if "SEPARATOR_Y" in geo:
            sep_x = geo["DIVIDER_X"] + 1 if "DIVIDER_X" in geo else 0
            display.hline(sep_x, geo["SEPARATOR_Y"], DISPLAY_WIDTH - sep_x, DIM_GRAY)

    # --- Records ---
    if "REC_AWAY_WINS" in R:            # stacked wins-over-losses (A, C)
        if pv.away_wins:
            writer.draw(R["REC_AWAY_WINS"], pv.away_wins, spleen_5x8, ALIGN_CENTER, 0, WHITE)
            writer.draw(R["REC_AWAY_LOSSES"], pv.away_losses, spleen_5x8, ALIGN_CENTER, 0, DIM_GRAY)
        if pv.home_wins:
            writer.draw(R["REC_HOME_WINS"], pv.home_wins, spleen_5x8, ALIGN_CENTER, 0, WHITE)
            writer.draw(R["REC_HOME_LOSSES"], pv.home_losses, spleen_5x8, ALIGN_CENTER, 0, DIM_GRAY)
    if "REC_AWAY" in R:                 # horizontal "41-28" (B)
        if pv.away_record:
            writer.draw(R["REC_AWAY"], pv.away_record, spleen_5x8, ALIGN_LEFT, 0, WHITE)
        if pv.home_record:
            writer.draw(R["REC_HOME"], pv.home_record, spleen_5x8, ALIGN_LEFT, 0, WHITE)

    pause = screen_geometry.PREGAME_SCROLL_PAUSE_MS
    pxs = screen_geometry.PREGAME_SCROLL_PX_PER_SEC

    # --- Cycling / stacked info ---
    if variant == "A":
        ends = pv.cycle_ends
        if ends:
            i, pstart, pos = _cycle_phase(ends, elapsed)
            writer.draw(R["INFO_LABEL"], pv.cycle_labels[i], spleen_5x8, ALIGN_LEFT, 0, DIM_GRAY)
            if pv.cycle_big[i]:
                writer.draw(R["INFO_VALUE"], pv.cycle_texts[i], unscii_16, ALIGN_CENTER, 0, WHITE)
            else:
                    _text_or_strip(writer, R["INFO_VALUE"], pv.cycle_texts[i], pv.cycle_strips[i],
                               ALIGN_LEFT, pos - pstart, WHITE, pause, pxs)
    elif variant == "B":
        if pv.venue_text:
            _text_or_strip(writer, R["INFO_VENUE"], pv.venue_text, pv.venue_strip,
                          ALIGN_LEFT, elapsed, WHITE, pause, pxs)
        if pv.time_text:
            writer.draw(R["INFO_TIME"], pv.time_text, spleen_5x8, ALIGN_LEFT, 0, WHITE)
        if pv.weather_text:
            _text_or_strip(writer, R["INFO_WEATHER"], pv.weather_text, pv.weather_strip,
                          ALIGN_LEFT, elapsed, WHITE, pause, pxs)
    elif variant == "C":
        if pv.time_text:
            writer.draw(R["INFO_TIME"], pv.time_text, unscii_16, ALIGN_CENTER, 0, WHITE)
        ends = pv.alt_ends
        if ends:
            i, pstart, pos = _cycle_phase(ends, elapsed)
            _text_or_strip(writer, R["INFO_CYCLE"], pv.alt_texts[i], pv.alt_strips[i],
                          ALIGN_LEFT, pos - pstart, WHITE, pause, pxs)

    # --- Pitchers ---
    if "PITCHER_AWAY" in R:             # static, per-team (A, C)
        if pv.away_pitcher:
            _text_or_strip(writer, R["PITCHER_AWAY"], pv.away_pitcher, pv.away_pitcher_strip,
                          ALIGN_LEFT, elapsed, pv.away_color, pause, pxs)
        if pv.home_pitcher:
            _text_or_strip(writer, R["PITCHER_HOME"], pv.home_pitcher, pv.home_pitcher_strip,
                          ALIGN_LEFT, elapsed, pv.home_color, pause, pxs)
    if "PITCHER_LINE" in R:             # alternating away<->home (B)
        if pv.away_pitcher and pv.home_pitcher:
            if (elapsed // screen_geometry.PREGAME_INFO_DWELL_MS) % 2 == 0:
                _text_or_strip(writer, R["PITCHER_LINE"], pv.away_pitcher, pv.away_pitcher_strip,
                              ALIGN_CENTER, elapsed, pv.away_color, pause, pxs)
            else:
                _text_or_strip(writer, R["PITCHER_LINE"], pv.home_pitcher, pv.home_pitcher_strip,
                              ALIGN_CENTER, elapsed, pv.home_color, pause, pxs)
        elif pv.away_pitcher:
            _text_or_strip(writer, R["PITCHER_LINE"], pv.away_pitcher, pv.away_pitcher_strip,
                          ALIGN_CENTER, elapsed, pv.away_color, pause, pxs)
        elif pv.home_pitcher:
            _text_or_strip(writer, R["PITCHER_LINE"], pv.home_pitcher, pv.home_pitcher_strip,
                          ALIGN_CENTER, elapsed, pv.home_color, pause, pxs)

    _render_toast(writer, regions, state, now_ms)
    _render_toast_overlay(display, state, now_ms)


def render_final(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int) -> None:
    """Render the final screen for the active screen_geometry variant.

    Winner emphasis is by color: the winning team's score and R total render in
    its (brightened) team color, the loser in DIM_GRAY — no abbreviations. The
    three line-score rows share one elapsed clock and equal char widths, so
    they scroll in lockstep; the R total column is pinned outside the scroll.
    """
    display.fill(BLACK)

    fv = state.final
    geo = screen_geometry.geometry_for(fv.variant_key)
    R = regions.variant[fv.variant_key]
    elapsed = view_elapsed_ms  # frame rail: motion holds, never jumps

    if fv.home_won:
        away_col = DIM_GRAY
        home_col = fv.home_color
    else:
        away_col = fv.away_color
        home_col = DIM_GRAY

    # --- Logos ---
    if state.away_logo is not None and "LOGO_AWAY" in geo:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None and "LOGO_HOME" in geo:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

    # --- Dividers ---
    # The vline separates the line score from the pinned R column; start it at
    # the top-band separator (when present) so it doesn't cut through a
    # top-corner logo.
    if screen_geometry.SHOW_DIVIDERS:
        if "DIVIDER_X" in geo:
            vy0 = geo["SEPARATOR_Y"] if "SEPARATOR_Y" in geo else 0
            display.vline(geo["DIVIDER_X"], vy0, DISPLAY_HEIGHT - vy0, DIM_GRAY)
        if "SEPARATOR_Y" in geo:
            display.hline(0, geo["SEPARATOR_Y"], DISPLAY_WIDTH, DIM_GRAY)

    # --- Big scores (A, B) ---
    if "SCORE_AWAY" in geo:
        sa = geo["SCORE_AWAY"]
        writer.integer(fv.away_score, sa[0], sa[1], sa[2], ALIGN_CENTER, away_col, font=unscii_16)
        sh = geo["SCORE_HOME"]
        writer.integer(fv.home_score, sh[0], sh[1], sh[2], ALIGN_CENTER, home_col, font=unscii_16)

    # --- FINAL / F/n label ---
    if "FINAL_LABEL" in R:
        writer.draw(R["FINAL_LABEL"], fv.final_text, unscii_8, ALIGN_CENTER, 0, colors.accent)

    # --- Line-score rows (lockstep scroll) ---
    pause = screen_geometry.FINAL_LS_PAUSE_MS
    pxs = screen_geometry.FINAL_LS_PX_PER_SEC
    if "LS_HEADER" in R:
        # Strip fast path: one blit per row (per-glyph looping these three
        # rows measured ~41 ms/frame — the 9 FPS stutter). Fallback only if a
        # row out-sized its pool (>21 innings).
        for region, text, strip, col in (
            (R["LS_HEADER"], fv.ls_header, fv.ls_header_strip, DIM_GRAY),
            (R["LS_AWAY"], fv.ls_away, fv.ls_away_strip, away_col),
            (R["LS_HOME"], fv.ls_home, fv.ls_home_strip, home_col),
        ):
            if strip is not None:
                writer.draw_strip(region, strip, ALIGN_LEFT, elapsed, col,
                                  pause_ms=pause, pixels_per_second=pxs)
            else:
                writer.draw(region, text, spleen_5x8, ALIGN_LEFT, elapsed, col,
                            pause_ms=pause, pixels_per_second=pxs)

    # --- Pinned totals (header "R" for MLB runs, "T" for NBA points) ---
    if "R_HEADER" in R:
        writer.draw(R["R_HEADER"], fv.total_label, spleen_5x8, ALIGN_CENTER, 0, DIM_GRAY)
    if "R_AWAY" in geo:
        ra = geo["R_AWAY"]
        r_font = unscii_16 if ra[3] >= 16 else spleen_5x8
        writer.integer(fv.away_score, ra[0], ra[1], ra[2], ALIGN_CENTER, away_col, font=r_font)
        rh = geo["R_HOME"]
        writer.integer(fv.home_score, rh[0], rh[1], rh[2], ALIGN_CENTER, home_col, font=r_font)

    _render_toast(writer, regions, state, now_ms)
    _render_toast_overlay(display, state, now_ms)


# Fixed glyph advance of the clock font (all shipped fonts are fixed-width);
# lets the composite clock ("45+6'") be centered with pure integer math.
_CLOCK_FONT = unscii_16
_CLOCK_CHAR_W = unscii_16.GLYPHS[ord("0") - 32][1]


def _draw_soccer_clock(display: Hub75Display, writer: FontWriter, rect: tuple,
                       sv, colors: UiColors, now_ms: int) -> None:
    """Draw the extrapolated match clock, allocation-free.

    The displayed minute is derived per frame from the poll-time anchor:
    `elapsed = anchor_s + ticks_diff(now_ms, anchor_ms) // 1000`. Integer
    math only (writer.integer + static "'"/"+" literals), so the clock ticks
    between polls with zero Core 0 involvement and no per-second commits —
    a Core 0 stall (TLS reconnect, GC) can't freeze or jump it, and each
    poll re-anchors any drift away.

    Rail choice: the match clock is REAL time, not motion — a stalled frame
    must consume match time, so it rides the wall rail (`now_ms` and
    `clock_anchor_ms` are both ticks-domain; never the frame rail, which
    would drift the clock from reality by every stall).

    Convention: floor minutes, matching ESPN's displayClock exactly (fixture
    evidence: "45'+6'" with halftime immediately after = 6 full stoppage
    minutes played). 23:30 elapsed reads "23'"; past the period's base the
    clock holds the base and counts added minutes ("45+2'"), drawn in the
    warning color.
    """
    x, y, w, _h = rect
    normal = colors.clock_normal

    if sv.halftime:
        writer.aligned_text("HT", x, y, w, ALIGN_CENTER, colors.accent, font=_CLOCK_FONT)
        return

    elapsed_s = sv.clock_anchor_s
    if sv.clock_running:
        elapsed_s += time.ticks_diff(now_ms, sv.clock_anchor_ms) // 1000
    m = elapsed_s // 60
    base = sv.base_min

    if m <= base:
        # At exactly the base minute (45:00-45:59) ESPN still shows "45'".
        chars = (1 if m < 10 else (2 if m < 100 else 3)) + 1
        cx = x + (w - chars * _CLOCK_CHAR_W) // 2
        cx = writer.integer(m, cx, y, 0, ALIGN_LEFT, normal, font=_CLOCK_FONT)
        writer.text("'", cx, y, normal, font=_CLOCK_FONT)
    else:
        # Stoppage time: hold the base minute and count the added minutes.
        extra = m - base
        if extra > 99:
            extra = 99
        warn = colors.clock_warning
        chars = (2 if base < 100 else 3) + (1 if extra < 10 else 2) + 2
        cx = x + (w - chars * _CLOCK_CHAR_W) // 2
        cx = writer.integer(base, cx, y, 0, ALIGN_LEFT, warn, font=_CLOCK_FONT)
        cx = writer.text("+", cx, y, warn, font=_CLOCK_FONT)
        cx = writer.integer(extra, cx, y, 0, ALIGN_LEFT, warn, font=_CLOCK_FONT)
        writer.text("'", cx, y, warn, font=_CLOCK_FONT)


def render_soccer_live(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    """Render the live soccer screen for the active screen_geometry variant.

    Shares the MLB live screen's visual frame (identity column, dividers,
    bottom strip); the data column carries the extrapolated match clock and
    the period. Bottom strip priority: toast > commentary flash (the latest
    play-by-play line, one scroll cycle, same machinery as the MLB play
    flash) > the persistent last goal / red card in the scoring team's
    color. Scroll motion rides the frame rail; the match clock rides the
    wall rail (see _draw_soccer_clock).
    """
    display.fill(BLACK)

    sv = state.soccer_live
    geo = screen_geometry.geometry_for('soccer_live')
    R = regions.variant['soccer_live']
    elapsed = view_elapsed_ms  # frame rail: motion holds, never jumps

    # --- Dividers ---
    if screen_geometry.SHOW_DIVIDERS:
        if "DIVIDER_X" in geo:
            display.vline(geo["DIVIDER_X"], 0, DISPLAY_HEIGHT, DIM_GRAY)
        if "SEPARATOR_Y" in geo:
            sep_x = geo["DIVIDER_X"] + 1 if "DIVIDER_X" in geo else 0
            display.hline(sep_x, geo["SEPARATOR_Y"], DISPLAY_WIDTH - sep_x, DIM_GRAY)

    # --- Logos + scores ---
    if state.away_logo is not None:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

    sa = geo["SCORE_AWAY"]
    writer.integer(sv.away_score, sa[0], sa[1], sa[2], ALIGN_CENTER, WHITE, font=unscii_16)
    sh = geo["SCORE_HOME"]
    writer.integer(sv.home_score, sh[0], sh[1], sh[2], ALIGN_CENTER, WHITE, font=unscii_16)

    # --- Period + clock ---
    if "PHASE" in R and sv.phase_text:
        writer.draw(R["PHASE"], sv.phase_text, unscii_8, ALIGN_CENTER, 0, WHITE)
    if "PHASE_LONG" in R and sv.phase_long:
        writer.draw(R["PHASE_LONG"], sv.phase_long, unscii_8, ALIGN_CENTER, 0, DIM_GRAY)

    _draw_soccer_clock(display, writer, geo["CLOCK"], sv, colors, now_ms)

    # --- Bottom strip: toast > commentary flash > last event ---
    if not _render_toast(writer, regions, state, now_ms):
        play = state.play
        play_window_ms = time.ticks_diff(now_ms, play.updated_ms)
        show_play = bool(play.text) and play.updated_ms != 0 and play_window_ms < play.display_ms
        if show_play:
            # No glyph fallback — see render_game (strip is an invariant).
            writer.draw_strip(
                regions.play_text, play.strip,
                ALIGN_LEFT, play_elapsed_ms, WHITE,
                pause_ms=PLAY_TEXT_SCROLL_PAUSE_MS,
                pixels_per_second=screen_geometry.GAME_SCROLL_PX_PER_SEC,
            )
        elif sv.event_top:
            pause = screen_geometry.SOCCER_SCROLL_PAUSE_MS
            pxs = screen_geometry.GAME_SCROLL_PX_PER_SEC
            writer.draw(R["EVENT_TOP"], sv.event_top, spleen_5x8, ALIGN_CENTER, elapsed,
                        sv.event_color, pause_ms=pause, pixels_per_second=pxs)
            if sv.event_name:
                if sv.event_name_strip is not None:
                    writer.draw_strip(R["EVENT_NAME"], sv.event_name_strip, ALIGN_CENTER,
                                      elapsed, sv.event_color,
                                      pause_ms=pause, pixels_per_second=pxs)
                else:
                    writer.draw(R["EVENT_NAME"], sv.event_name, unscii_8, ALIGN_CENTER,
                                elapsed, sv.event_color,
                                pause_ms=pause, pixels_per_second=pxs)
        elif "EVENT_EMPTY" in R:
            # Nothing in the ticker yet: a dim placeholder so the strip
            # doesn't read as a rendering hole.
            writer.draw(R["EVENT_EMPTY"], "NO GOALS", spleen_5x8, ALIGN_CENTER, 0, DIM_GRAY)

    _render_toast_overlay(display, state, now_ms)


def render_nba_live(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    """Render the live NBA screen (single design, soccer-A silhouette).

    Identity column: stacked logos + scores (3-digit-wide slots) with the
    period chip ("Q3" / "OT") where soccer's half sat. Data column: the
    poll-time clock string — never extrapolated (see NbaLiveView) — in the
    clock color, warning-colored when sub-minute, accent "HT"/"END" during
    breaks. Bottom strip: toast > play flash (shared machinery); NBA has no
    persistent ticker, so the strip is empty between flashes like the MLB
    pitcher/batter view is between plays.
    """
    display.fill(BLACK)

    nv = state.nba_live
    geo = screen_geometry.geometry_for('nba_live')
    R = regions.variant['nba_live']

    # --- Dividers ---
    if screen_geometry.SHOW_DIVIDERS:
        display.vline(geo["DIVIDER_X"], 0, DISPLAY_HEIGHT, DIM_GRAY)
        display.hline(geo["DIVIDER_X"] + 1, geo["SEPARATOR_Y"],
                      DISPLAY_WIDTH - geo["DIVIDER_X"] - 1, DIM_GRAY)

    # --- Logos + scores ---
    if state.away_logo is not None:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

    sa = geo["SCORE_AWAY"]
    writer.integer(nv.away_score, sa[0], sa[1], sa[2], ALIGN_CENTER, WHITE, font=unscii_16)
    sh = geo["SCORE_HOME"]
    writer.integer(nv.home_score, sh[0], sh[1], sh[2], ALIGN_CENTER, WHITE, font=unscii_16)

    # --- Period chip + clock ---
    if nv.phase_text:
        writer.draw(R["PHASE"], nv.phase_text, unscii_8, ALIGN_CENTER, 0, WHITE)

    if nv.clock_accent:
        clock_col = colors.accent
    elif nv.clock_low:
        clock_col = colors.clock_warning
    else:
        clock_col = colors.clock_normal
    ck = geo["CLOCK"]
    writer.aligned_text(nv.clock_text, ck[0], ck[1], ck[2], ALIGN_CENTER, clock_col, font=_CLOCK_FONT)

    # --- Bottom strip: toast > play flash > empty ---
    if not _render_toast(writer, regions, state, now_ms):
        play = state.play
        play_window_ms = time.ticks_diff(now_ms, play.updated_ms)
        if bool(play.text) and play.updated_ms != 0 and play_window_ms < play.display_ms:
            # No glyph fallback — see render_game (strip is an invariant).
            writer.draw_strip(
                regions.play_text, play.strip,
                ALIGN_LEFT, play_elapsed_ms, WHITE,
                pause_ms=PLAY_TEXT_SCROLL_PAUSE_MS,
                pixels_per_second=screen_geometry.GAME_SCROLL_PX_PER_SEC,
            )

    _render_toast_overlay(display, state, now_ms)


def render_soccer_final(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int) -> None:
    """Render the soccer full-time screen.

    The final-C silhouette with goal scorers where baseball's line score
    was. Winner emphasis by color, loser in DIM_GRAY — except a draw, which
    colors both teams (soccer draws are real results).
    """
    display.fill(BLACK)

    fv = state.soccer_final
    geo = screen_geometry.geometry_for('soccer_final')
    R = regions.variant['soccer_final']
    elapsed = view_elapsed_ms  # frame rail: motion holds, never jumps

    if fv.draw:
        away_col = fv.away_color
        home_col = fv.home_color
    elif fv.home_won:
        away_col = DIM_GRAY
        home_col = fv.home_color
    else:
        away_col = fv.away_color
        home_col = DIM_GRAY

    # --- Dividers ---
    if screen_geometry.SHOW_DIVIDERS and "DIVIDER_X" in geo:
        display.vline(geo["DIVIDER_X"], 0, DISPLAY_HEIGHT, DIM_GRAY)

    # --- Logos + scores ---
    if state.away_logo is not None:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

    sa = geo["SCORE_AWAY"]
    writer.integer(fv.away_score, sa[0], sa[1], sa[2], ALIGN_CENTER, away_col, font=unscii_16)
    sh = geo["SCORE_HOME"]
    writer.integer(fv.home_score, sh[0], sh[1], sh[2], ALIGN_CENTER, home_col, font=unscii_16)

    # --- FULL TIME + scorers ---
    writer.draw(R["FT_LABEL"], fv.ft_text, unscii_8, ALIGN_CENTER, 0, colors.accent)

    pause = screen_geometry.SOCCER_SCROLL_PAUSE_MS
    pxs = screen_geometry.GAME_SCROLL_PX_PER_SEC
    if fv.scorers_away:
        _text_or_strip(writer, R["SCORERS_AWAY"], fv.scorers_away, fv.scorers_away_strip,
                       ALIGN_CENTER, elapsed, away_col, pause, pxs)
    if fv.scorers_home:
        _text_or_strip(writer, R["SCORERS_HOME"], fv.scorers_home, fv.scorers_home_strip,
                       ALIGN_CENTER, elapsed, home_col, pause, pxs)

    _render_toast(writer, regions, state, now_ms)
    _render_toast_overlay(display, state, now_ms)


def render_frame(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    """
    Render a frame based on current display state.

    Pure function of its time inputs — nothing here queries the clock.

    Two time rails (rule: a stall STRETCHES motion but CONSUMES waiting):
    - `now_ms` (wall rail): event windows and durations — toast lifetime,
      the play-flash visibility window. A GC-stalled frame still counts
      against these, so feedback timing stays honest.
    - `view_elapsed_ms` / `play_elapsed_ms` (frame rail): advance exactly
      FRAME_MS per rendered frame, latched to their epochs
      (state.animation_start_ms / play.updated_ms) by the display thread.
      All continuous motion — scroll offsets, the count-dot pulse — rides
      these, so a stalled frame holds position for one frame instead of
      jumping. Under perfect pacing the rails are identical.
    The pregame cycle (phase dwell + in-phase scroll) rides the frame rail
    as a unit: dwell is sized to one scroll cycle, and that coupling matters
    more than dwell exactness. Low-stakes modes (startup/setup/no_games)
    keep the wall rail.
    """
    renderer = _RENDERERS.get(state.mode)
    if renderer is None:
        render_idle(display, writer, regions, colors)
    else:
        renderer(display, writer, regions, state, colors, now_ms, view_elapsed_ms, play_elapsed_ms)


# Mode -> renderer table (one uniform time-rail signature; the adapters drop
# the rails a renderer doesn't use). A mode missing here falls back to
# render_idle in render_frame; adding a sport screen = one entry.
_RENDERERS = {
    'startup': lambda d, w, r, s, c, now, view, play: render_startup(d, w, r, s, c),
    'idle': lambda d, w, r, s, c, now, view, play: render_idle(d, w, r, c),
    'no_games': lambda d, w, r, s, c, now, view, play: render_no_games(d, w, r, s, c, now),
    'setup': lambda d, w, r, s, c, now, view, play: render_setup(d, w, r, s, c, now),
    'error': lambda d, w, r, s, c, now, view, play: render_error(d, w, r, s, c),
    'updating': lambda d, w, r, s, c, now, view, play: render_updating(d, w, r, s, c),
    'mlb_live': render_game,
    'pregame': lambda d, w, r, s, c, now, view, play: render_pregame(d, w, r, s, c, now, view),
    'final': lambda d, w, r, s, c, now, view, play: render_final(d, w, r, s, c, now, view),
    'soccer_live': render_soccer_live,
    'soccer_final': lambda d, w, r, s, c, now, view, play: render_soccer_final(d, w, r, s, c, now, view),
    'nba_live': render_nba_live,
}


# =============================================================================
# Display thread (runs on Core 1)
# =============================================================================

# Modes with no time-driven animation: re-rendering is only needed when a new
# commit lands (or a toast is fading out). 'mlb_live' and 'setup' animate every
# frame (scrolling text, pulsing count dots) and always redraw.
_STATIC_MODES = ('idle', 'no_games', 'error', 'startup', 'updating')


def run_display_thread(display: Hub75Display, writer: FontWriter, regions: Regions, health: ThreadHealth) -> None:
    """
    Main entry point for Core 1 display thread.

    Runs a constant 20 FPS loop latching state from the mailbox and
    rendering to the display. Static screens are only re-rendered when a
    new commit lands, so an idle scoreboard isn't redrawing 20x/second.
    `health.frame_seq` is bumped every tick (rendered or skipped) so the
    Core 0 watchdog feeder can distinguish a hung thread from a quiet one.

    Core 1 avoids heap allocation on the steady-state path: all strings are
    pre-built on Core 0, glyph blits reuse pre-allocated specs, scrolling
    text blits from pre-rendered strips, and scores use the cached-digit
    integer() path.

    Pacing is deadline-based: each iteration targets `now + FRAME_MS`, and
    the sleep absorbs however long the frame took — so the cadence is a
    constant 20 FPS instead of "20 FPS minus render time" (the old
    sleep-after-render drifted the wall-time scroll math into uneven pixel
    steps). An overrun frame re-anchors the deadline rather than bursting to
    catch up: a display must never fast-forward.

    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    Regions are pre-allocated on Core 0 and read-only here.
    """
    from scoreboard.state import acquire_display_state

    logger.debug("[DISPLAY] thread starting: core=1 rate=20fps")

    last_rendered_seq = -1
    last_frame_had_toast = False

    # Frame-health telemetry, reported every 60 s at DEBUG level: total
    # frames, frames whose period overran FRAME_MS by >40% (a stutter the
    # eye can catch), and the worst period. This is how the 9 FPS scroll
    # regression was found — cheap enough to keep.
    _hb_prev_ms = time.ticks_ms()
    _hb_frames = 0
    _hb_slow = 0
    _hb_worst = 0
    _hb_last_report = _hb_prev_ms

    # Frame rail: advances exactly FRAME_MS per loop tick, so motion derived
    # from it holds position through a stalled frame instead of jumping (the
    # wall clock keeps running through a GC pause; this clock doesn't).
    # Core 0's epoch stamps (animation_start_ms, play.updated_ms) are in the
    # ticks domain, so they are never subtracted against this rail directly —
    # the latches below translate an epoch CHANGE into "frame-rail time zero".
    anim_ms = 0
    view_epoch_stamp = -1
    view_epoch_anim = 0
    play_epoch_stamp = -1
    play_epoch_anim = 0

    deadline = time.ticks_ms()

    # [MEMPROF] window state (see MEM_PROFILE). Per-tick mem_alloc deltas:
    # positive = bytes allocated since last tick (render + whatever Core 0
    # did meanwhile); negative = a collection ran. `over` counts EVERY tick
    # that blew the 50 ms budget (health's `slow` only counts >70 ms), so
    # visible stutters and GC events can be correlated directly.
    _mp_prev = gc.mem_alloc() if MEM_PROFILE else 0
    _mp_churn = 0
    _mp_gcs = 0
    _mp_worst_d = 0
    _mp_freed = 0
    _mp_maxdrop = 0
    _mp_over = 0
    _mp_report_ms = time.ticks_ms()

    while True:
        # Heartbeat for the watchdog feeder: per tick, not per render.
        health.frame_seq = (health.frame_seq + 1) & 0x3FFFFFF

        deadline = time.ticks_add(deadline, FRAME_MS)

        try:
            now_ms = time.ticks_ms()

            if MEM_PROFILE:
                _mp_now = gc.mem_alloc()
                _mp_d = _mp_now - _mp_prev
                _mp_prev = _mp_now
                if _mp_d >= 0:
                    _mp_churn += _mp_d
                    if _mp_d > _mp_worst_d:
                        _mp_worst_d = _mp_d
                else:
                    # mem_alloc dropped: a real collection (drop ~= all
                    # garbage since the last one) or a C-level explicit
                    # free (network buffers etc., typically small). Track
                    # sizes so the two are distinguishable in the report.
                    _mp_gcs += 1
                    _mp_freed -= _mp_d
                    if -_mp_d > _mp_maxdrop:
                        _mp_maxdrop = -_mp_d

            _hb_period = time.ticks_diff(now_ms, _hb_prev_ms)
            _hb_prev_ms = now_ms
            _hb_frames += 1
            if _hb_period > _hb_worst:
                _hb_worst = _hb_period
            if _hb_period > FRAME_MS + (FRAME_MS * 2) // 5:
                _hb_slow += 1

            # Latch the latest committed state for this frame.
            state, seq = acquire_display_state()

            # Advance the frame rail and re-latch epochs on change.
            anim_ms += FRAME_MS
            if state.animation_start_ms != view_epoch_stamp:
                view_epoch_stamp = state.animation_start_ms
                view_epoch_anim = anim_ms
            if state.play.updated_ms != play_epoch_stamp:
                play_epoch_stamp = state.play.updated_ms
                play_epoch_anim = anim_ms

            # "Active" includes the overlay's fade-out tail so static modes
            # keep rendering until the dim has fully eased back out.
            toast_active = (_toast_active(state, now_ms)
                            or _toast_overlay_fading(state, now_ms))
            skip = (seq == last_rendered_seq
                    and state.mode in _STATIC_MODES
                    and not toast_active
                    and not last_frame_had_toast)

            if not skip:
                render_frame(display, writer, regions, state, state.ui_colors, now_ms,
                             anim_ms - view_epoch_anim, anim_ms - play_epoch_anim)
                display.show()
                last_rendered_seq = seq
                last_frame_had_toast = toast_active

            if time.ticks_diff(now_ms, _hb_last_report) >= 60_000:
                if logger.level >= DEBUG:
                    logger.debug(
                        "[DISPLAY] health: frames=%d slow=%d worst=%dms"
                        % (_hb_frames, _hb_slow, _hb_worst)
                    )
                _hb_frames = _hb_slow = _hb_worst = 0
                _hb_last_report = now_ms

            if MEM_PROFILE and time.ticks_diff(now_ms, _mp_report_ms) >= 10_000:
                _mp_play = state.play
                if logger.level >= DEBUG:
                    logger.debug(
                        "[MEMPROF] churn=%dB/s gc=%d freed=%dB maxdrop=%dB worstd=%dB over=%d mode=%s play=%d strip=%d"
                        % (_mp_churn // 10, _mp_gcs, _mp_freed, _mp_maxdrop,
                           _mp_worst_d, _mp_over, state.mode, len(_mp_play.text),
                           0 if _mp_play.strip is None else 1)
                    )
                _mp_churn = _mp_gcs = _mp_worst_d = _mp_freed = _mp_maxdrop = _mp_over = 0
                _mp_report_ms = now_ms

        except Exception as e:
            # Guarded: this path can repeat every frame while erroring, so
            # don't build the message when ERROR logging is off.
            if logger.level >= ERROR:
                logger.error(f"[DISPLAY] thread error: {e}")

        # Deadline pacing: sleep whatever remains of this frame's budget.
        remaining = time.ticks_diff(deadline, time.ticks_ms())
        if remaining > 0:
            time.sleep_ms(remaining)
        else:
            # Overran the budget (e.g. a GC pause): re-anchor instead of
            # bursting frames to catch up.
            _mp_over += 1
            deadline = time.ticks_ms()

"""
Display rendering and thread management for the Pico Scoreboard.

Provides render functions for non-game display modes (startup, idle,
no_games, setup, error), the logo caching system, animation primitives,
and the Core 1 display thread.

Render functions are pure readers: every string they draw was pre-built on
Core 0 when the state changed (see scoreboard/state.py), so the render loop
does no per-frame text formatting.

Core 1 mutation contract: everything the display thread may write is
enumerated in the contract block above `class LoopState` (cross-frame state
in exactly one loop-local object, registered write-before-read scratch, the
draw targets, and ThreadHealth.frame_seq). Read that block before adding ANY
mutable value to this file's render path.
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
from scoreboard.layout import football_field as football_field_sprite
from scoreboard.layout import football_ball as football_ball_sprite
from scoreboard.layout import first_base as first_base_loc
from scoreboard.layout import second_base as second_base_loc
from scoreboard.layout import third_base as third_base_loc
from scoreboard.layout import toast_spinner as toast_spinner_sprite
from scoreboard.layout import toast_lock_closed as toast_lock_closed_sprite
from scoreboard.layout import toast_lock_open as toast_lock_open_sprite

# Fixed colors
BLACK = 0
WHITE = rgb565(255, 255, 255)
DIM_GRAY = rgb565(96, 96, 96)

# _team_color_to_rgb565 / _TEAM_COLOR_MIN_CHANNEL live in scoreboard.state so
# Core 0 setters can pre-brighten team colors without importing display;
# render_mlb_live imports the helper (above) for its per-frame use.


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

# --- League menu geometry (matches the approved mockups) ---
# Inset 1px from every panel edge (owner rule 2026-07-15: edge pixels are
# unreliable on this panel — draw nothing in row 0, row 63, col 0, col 127).
# 5 list rows of 10px (y 1..50), separator hline at 52, DONE footer 54..62.
# Checkbox 7x7 at x=2, always drawn in the primary color OUTSIDE the highlight
# bar — the bar starts at x=10 so a highlighted row can't invert the checkbox
# and make checked read as unchecked; label Regions in Regions.menu_rows;
# highlight bar and scrollbar split the right side inside the inset.
_MENU_TOP = 1           # first list row's top edge (the 1px inset)
_MENU_ROW_H = 10
_MENU_VISIBLE_ROWS = 5
_MENU_SEP_Y = 52
_MENU_DONE_Y = 54       # footer band 54..62; row 63 stays dark
_MENU_CHECKBOX_X = 2
_MENU_HILIGHT_X = 10    # after the checkbox (x 2..8) + 1px dark separator
_MENU_HILIGHT_W = 115   # highlight bar x 10..124, stops before the scrollbar
_MENU_BAR_X = 125       # 2px scrollbar track at x 125..126, y 1..50

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
# measure with exactly the font the live renderers draw with, or the computed
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
    max_scroll = text_w - screen_geometry.PLAY_TEXT[2]
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

        # --- Shared play-flash strip (all live screens) ---
        # The MLB text slots that used to live here are now built from the
        # mlb_live variant table (see rebuild_variant_regions); only the
        # sport-neutral flash strip stays fixed, sourced from a code constant.
        pt = screen_geometry.PLAY_TEXT
        self.play_text = Region(display, pt[0], pt[1], pt[2], pt[3])

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

        # --- League menu (full-screen take-over; see render_menu) ---
        # Label windows: x=12 (clear of the 7x7 checkbox at x=2) through
        # x=123 (the 2px scrollbar owns 125..126 inside the 1px edge inset;
        # 124 is breathing room) = 112 px, one per visible 10px row.
        # NOTE: "PREMIER LEAGUE" is 14 unscii_8 glyphs = exactly 112 px —
        # zero margin. Shrinking this window makes the longest real league
        # label start marqueeing.
        self.menu_rows = tuple(
            Region(display, 12, _MENU_TOP + row * 10 + 1, 112, 8)
            for row in range(_MENU_VISIBLE_ROWS)
        )

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


def _draw_count_dots(display: Hub75Display, x: int, y: int, width: int, filled_count: int, filled_color: int | None = None) -> None:
    n_dots = (width + 1) // (dot_sprite.WIDTH + 1)  # type: ignore[attr-defined]
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
                x + i * (dot_sprite.WIDTH + 1),  # type: ignore[attr-defined]
                y,
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

# [ball565, highlight565, shade565] — SCRATCH (see scratch_buffers()): fully
# rewritten by _base_marker_colors before every read. Deliberately NOT a
# cross-frame memo: the old packed-key memoization made this the one piece of
# cross-frame state living outside LoopState, and it saved only ~30 integer
# ops per frame. Recomputing keeps Core 1's cross-frame state in exactly one
# audited place.
_base_pal = [0, 0, 0]


def _base_marker_colors(packed: int) -> list:
    """Base-marker palette derived from the batting team's primary color,
    written fresh into the _base_pal scratch on EVERY call (write-before-read
    scratch contract — never carry values between calls). Relationships match
    the original gold sprite: highlight = 7/8 blend toward white, edge shade =
    7/8 of the ball color. Integer math only (see pulse() on float churn)."""
    c = _base_pal
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
    c[0] = rgb565(r, g, b)
    c[1] = rgb565(r + ((255 - r) * 7 >> 3), g + ((255 - g) * 7 >> 3), b + ((255 - b) * 7 >> 3))
    c[2] = rgb565(r * 7 >> 3, g * 7 >> 3, b * 7 >> 3)
    return c


def _draw_base_markers(display: Hub75Display, bases, packed: int) -> None:
    """Blit occupied-base markers. `packed` is the batting team's RGB888
    primary, or -1 (MIDDLE/END half) to keep the default gold palette."""
    pal = base_marker_sprite.palette
    if packed >= 0:
        c = _base_marker_colors(packed)
        pal.pixel(1, 0, c[0])
        pal.pixel(2, 0, c[1])
        pal.pixel(3, 0, c[2])
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
        _draw_count_dots(display, _startup_dots_loc.X, _startup_dots_loc.Y,
                         _startup_dots_loc.WIDTH, startup.attempt)

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


def render_mlb_live(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    display.fill(BLACK)

    mlv = state.mlb_live
    if not mlv.game_id:
        render_idle(display, writer, regions, colors)
        _render_toast(writer, regions, state, now_ms)
        _render_toast_overlay(display, state, now_ms)
        return

    geo = screen_geometry.geometry_for('mlb_live')
    R = regions.variant['mlb_live']

    # --- Dividers (shared style with the pregame/final screens) ---
    if screen_geometry.SHOW_DIVIDERS:
        display.vline(geo["DIVIDER_X"], 0, DISPLAY_HEIGHT, DIM_GRAY)
        display.hline(geo["DIVIDER_X"] + 1, geo["SEPARATOR_Y"],
                      DISPLAY_WIDTH - geo["DIVIDER_X"] - 1, DIM_GRAY)

    # --- Sprites ---

    display.blit(field_sprite.data, field_sprite.X, field_sprite.Y, field_sprite.KEY, field_sprite.palette)  # type: ignore

    # Base markers take the batting team's color (top: away bats, bottom:
    # home bats); transition halves keep the default gold.
    half = mlv.half
    _draw_base_markers(display, mlv.bases, mlv.batting_packed)

    if state.away_logo is not None:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

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

    bd = geo["BALL_DOTS"]
    _draw_count_dots(display, bd[0], bd[1], bd[2], mlv.balls, pulsed if balls_critical else None)
    sd = geo["STRIKE_DOTS"]
    _draw_count_dots(display, sd[0], sd[1], sd[2], mlv.strikes, pulsed if strikes_critical else None)
    od = geo["OUT_DOTS"]
    _draw_count_dots(display, od[0], od[1], od[2], mlv.outs, pulsed if outs_critical else None)

    # --- Text ---
    # Scores stay on the zero-alloc integer() path.
    sa = geo["SCORE_AWAY"]
    writer.integer(mlv.away_score, sa[0], sa[1], sa[2], ALIGN_CENTER, WHITE, font=unscii_16)
    sh = geo["SCORE_HOME"]
    writer.integer(mlv.home_score, sh[0], sh[1], sh[2], ALIGN_CENTER, WHITE, font=unscii_16)

    writer.draw(R["INNING"], mlv.inning_text, unscii_8, ALIGN_CENTER, 0, WHITE)

    writer.draw(R["BALL_LABEL"], "B", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)
    writer.draw(R["STRIKE_LABEL"], "S", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)
    writer.draw(R["OUT_LABEL"], "O", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)

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
                _text_or_strip(writer, R["PITCHER_NAME"], mlv.pitcher_text,
                               mlv.pitcher_strip, ALIGN_CENTER, elapsed_ms, pitch_color,
                               PLAY_TEXT_SCROLL_PAUSE_MS,
                               screen_geometry.GAME_SCROLL_PX_PER_SEC)
                _text_or_strip(writer, R["BATTER_NAME"], mlv.batter_text,
                               mlv.batter_strip, ALIGN_CENTER, elapsed_ms, bat_color,
                               PLAY_TEXT_SCROLL_PAUSE_MS,
                               screen_geometry.GAME_SCROLL_PX_PER_SEC)

            writer.draw(R["PITCHER_LABEL"], "PIT", unscii_8, ALIGN_LEFT, 0, pitch_color)
            writer.draw(R["BATTER_LABEL"], "BAT", unscii_8, ALIGN_LEFT, 0, bat_color)

    _render_toast_overlay(display, state, now_ms)


# _cycle_phase writes into this preallocated slot list instead of returning
# a fresh tuple — it runs per frame in the pregame renderers, and the old
# tuple return was the one allocation its "allocation-free" docstring missed.
# SCRATCH (see scratch_buffers()): every slot is written before every read;
# it must never carry values between calls.
_CYCLE_OUT = [0, 0, 0]


def _cycle_phase(ends: list, elapsed_ms: int) -> list:
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
            _CYCLE_OUT[0] = i
            _CYCLE_OUT[1] = start
            _CYCLE_OUT[2] = pos
            return _CYCLE_OUT
        start = ends[i]
    # Unreachable — pos = elapsed % total is always < ends[-1] — but kept
    # total so a malformed dwell table degrades to the last phase instead
    # of returning stale scratch.
    _CYCLE_OUT[0] = len(ends) - 1
    _CYCLE_OUT[1] = start
    _CYCLE_OUT[2] = pos
    return _CYCLE_OUT


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
    """Render the pregame screen (single "Big time" design, all sports).

    Logos identify the teams (no abbreviations); stacked W/L records beside
    them. The right column: the big first-pitch/kickoff time — alternating
    with the pre-built "WED JUL 16" date whenever the game isn't today — over
    one cycling venue<->weather line and the probable pitchers in team
    colors. The date/time alternation rides the same frame-rail dwell
    arithmetic as every other pregame phase (pure function of elapsed).
    """
    display.fill(BLACK)

    pv = state.pregame
    geo = screen_geometry.geometry_for(pv.variant_key)
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

    # --- Records (stacked wins-over-losses) ---
    if pv.away_wins:
        writer.draw(R["REC_AWAY_WINS"], pv.away_wins, spleen_5x8, ALIGN_CENTER, 0, WHITE)
        writer.draw(R["REC_AWAY_LOSSES"], pv.away_losses, spleen_5x8, ALIGN_CENTER, 0, DIM_GRAY)
    if pv.home_wins:
        writer.draw(R["REC_HOME_WINS"], pv.home_wins, spleen_5x8, ALIGN_CENTER, 0, WHITE)
        writer.draw(R["REC_HOME_LOSSES"], pv.home_losses, spleen_5x8, ALIGN_CENTER, 0, DIM_GRAY)

    pause = screen_geometry.PREGAME_SCROLL_PAUSE_MS
    pxs = screen_geometry.PREGAME_SCROLL_PX_PER_SEC

    # --- Big time / date slot ---
    if pv.time_text:
        if pv.date_text and (
            elapsed // screen_geometry.PREGAME_INFO_DWELL_MS
        ) % 2 == 0:
            # Not today: lead with the date (the surprising fact), then the
            # time, alternating one dwell each.
            big = pv.date_text
        else:
            big = pv.time_text
        writer.draw(R["INFO_TIME"], big, unscii_16, ALIGN_CENTER, 0, WHITE)

    # --- Cycling info line (venue <-> weather) ---
    ends = pv.alt_ends
    if ends:
        i, pstart, pos = _cycle_phase(ends, elapsed)
        _text_or_strip(writer, R["INFO_CYCLE"], pv.alt_texts[i], pv.alt_strips[i],
                      ALIGN_LEFT, pos - pstart, WHITE, pause, pxs)

    # --- Pitchers (static, per-team colors) ---
    if pv.away_pitcher:
        _text_or_strip(writer, R["PITCHER_AWAY"], pv.away_pitcher, pv.away_pitcher_strip,
                      ALIGN_LEFT, elapsed, pv.away_color, pause, pxs)
    if pv.home_pitcher:
        _text_or_strip(writer, R["PITCHER_HOME"], pv.home_pitcher, pv.home_pitcher_strip,
                      ALIGN_LEFT, elapsed, pv.home_color, pause, pxs)

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

    if sv.on_break:
        # Classic halftime reads "HT"; later breaks (ET halftime, end of
        # regulation/extra time) read "BREAK" — base_min disambiguates.
        label = "HT" if sv.base_min == 45 else "BREAK"
        writer.aligned_text(label, x, y, w, ALIGN_CENTER, colors.accent, font=_CLOCK_FONT)
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
            # No glyph fallback — see render_mlb_live (strip is an invariant).
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
            # No glyph fallback — see render_mlb_live (strip is an invariant).
            writer.draw_strip(
                regions.play_text, play.strip,
                ALIGN_LEFT, play_elapsed_ms, WHITE,
                pause_ms=PLAY_TEXT_SCROLL_PAUSE_MS,
                pixels_per_second=screen_geometry.GAME_SCROLL_PX_PER_SEC,
            )

    _render_toast_overlay(display, state, now_ms)


# =============================================================================
# Football live screen
# =============================================================================

# The field sprite's endzone blocks ship as pure-red (away) / pure-blue
# (home) placeholder colors; their palette indices are discovered at import
# by value because compile_layout assigns indices in first-seen order, which
# an art edit can reorder. GS4 palettes cap at 16 entries and out-of-range
# pixel() reads return None on the device and the preview shim alike, so the
# bounded scan is safe. A missing placeholder means the art drifted — raise
# here at import (Core 0), never mid-render.
_EZ_AWAY_PLACEHOLDER = rgb565(255, 0, 0)
_EZ_HOME_PLACEHOLDER = rgb565(0, 0, 255)


def _football_palette_index(color: int, what: str) -> int:
    for i in range(16):
        if football_field_sprite.palette.pixel(i, 0) == color:
            return i
    raise ValueError("football field palette: missing " + what + " placeholder")


_EZ_AWAY_IDX = _football_palette_index(_EZ_AWAY_PLACEHOLDER, "away endzone")
_EZ_HOME_IDX = _football_palette_index(_EZ_HOME_PLACEHOLDER, "home endzone")

_FOOTBALL_FIELD_TOP_Y = football_field_sprite.Y                                        # 52
_FOOTBALL_FIELD_BOTTOM_Y = football_field_sprite.Y + football_field_sprite.HEIGHT - 1  # 62
_FOOTBALL_BALL_Y = football_field_sprite.Y - football_ball_sprite.HEIGHT - 2           # 45
_FOOTBALL_BALL_HALF_W = football_ball_sprite.WIDTH // 2
_FOOTBALL_LOS_COLOR = rgb565(0, 0, 140)   # scrimmage navy (pre-rewrite palette)
_FOOTBALL_FD_COLOR = rgb565(255, 255, 0)  # first-down yellow


def _draw_football_arrow(display: Hub75Display, x: int, y: int, right: bool, color: int) -> None:
    """3x5 solid triangle pointing left or right, top-left corner at (x, y)."""
    for i in range(3):
        col_x = x + i if right else x + 2 - i
        display.vline(col_x, y + i, 5 - 2 * i, color)


def _draw_timeout_bars(display: Hub75Display, x: int, y: int, remaining: int, color: int) -> None:
    """Three 6x1 bars with 1px gaps: team color while held, DIM_GRAY once
    spent (bars empty left-to-right as timeouts are burned)."""
    for i in range(3):
        display.hline(x + i * 7, y, 6, color if i < remaining else DIM_GRAY)


def _draw_football_field(display: Hub75Display, fb) -> None:
    """Blit the field with endzones tinted to the team colors, then the
    precomputed scrimmage / first-down perspective lines, the ball riding
    the scrimmage line's top end, and the attack-direction arrow. All
    endpoints come from FootballLiveView (Core 0 projects; Core 1 only
    draws segments). The palette tint restores in a `finally` — the
    base-marker pattern; see the mutation contract's special case.
    """
    pal = football_field_sprite.palette
    pal.pixel(_EZ_AWAY_IDX, 0, fb.away_color)
    pal.pixel(_EZ_HOME_IDX, 0, fb.home_color)
    try:
        display.blit(football_field_sprite.data, football_field_sprite.X,
                     football_field_sprite.Y, football_field_sprite.KEY, pal)  # type: ignore
    finally:
        pal.pixel(_EZ_AWAY_IDX, 0, _EZ_AWAY_PLACEHOLDER)
        pal.pixel(_EZ_HOME_IDX, 0, _EZ_HOME_PLACEHOLDER)

    if not fb.has_ball:
        return

    # 2px-wide perspective lines; first-down yellow wins where they meet.
    for dx in range(2):
        display.line(fb.los_x + dx, _FOOTBALL_FIELD_BOTTOM_Y,
                     fb.los_top_x + dx, _FOOTBALL_FIELD_TOP_Y, _FOOTBALL_LOS_COLOR)
    if fb.fd_x >= 0:
        for dx in range(2):
            display.line(fb.fd_x + dx, _FOOTBALL_FIELD_BOTTOM_Y,
                         fb.fd_top_x + dx, _FOOTBALL_FIELD_TOP_Y, _FOOTBALL_FD_COLOR)

    display.blit(football_ball_sprite.data, fb.los_top_x - _FOOTBALL_BALL_HALF_W,
                 _FOOTBALL_BALL_Y, football_ball_sprite.KEY, football_ball_sprite.palette)  # type: ignore
    if fb.dir_right:
        _draw_football_arrow(display, fb.los_top_x + _FOOTBALL_BALL_HALF_W + 3,
                             _FOOTBALL_BALL_Y, True, _FOOTBALL_LOS_COLOR)
    else:
        _draw_football_arrow(display, fb.los_top_x - _FOOTBALL_BALL_HALF_W - 5,
                             _FOOTBALL_BALL_Y, False, _FOOTBALL_LOS_COLOR)


def render_football_live(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    """Render the live football screen (single design, broadcast corners).

    Logos in the top corners over timeout bars and scores; quarter chip +
    poll-time clock across the top center (a display string, never
    extrapolated — see FootballLiveView); down & distance mid-screen with
    the possession arrow beside it, both in the warning color inside the
    red zone; the perspective field strip along the bottom with
    team-tinted endzones, scrimmage and first-down lines, and the ball at
    the scrimmage line. Bottom-zone priority: toast > play flash (shared
    strip, overlays the field zone) > field.
    """
    display.fill(BLACK)

    fb = state.football_live
    geo = screen_geometry.geometry_for('football_live')
    R = regions.variant['football_live']

    # --- Corner stacks: logos, timeout bars, scores ---
    if state.away_logo is not None:
        display.blit(state.away_logo, geo["LOGO_AWAY"][0], geo["LOGO_AWAY"][1])
    if state.home_logo is not None:
        display.blit(state.home_logo, geo["LOGO_HOME"][0], geo["LOGO_HOME"][1])

    to_y = geo["TIMEOUT_Y"]
    if fb.away_timeouts >= 0:
        _draw_timeout_bars(display, geo["TIMEOUT_AWAY_X"], to_y, fb.away_timeouts, fb.away_color)
    if fb.home_timeouts >= 0:
        _draw_timeout_bars(display, geo["TIMEOUT_HOME_X"], to_y, fb.home_timeouts, fb.home_color)

    sa = geo["SCORE_AWAY"]
    writer.integer(fb.away_score, sa[0], sa[1], sa[2], ALIGN_CENTER, WHITE, font=unscii_16)
    sh = geo["SCORE_HOME"]
    writer.integer(fb.home_score, sh[0], sh[1], sh[2], ALIGN_CENTER, WHITE, font=unscii_16)

    # --- Period chip + clock (NBA conventions) ---
    if fb.phase_text:
        writer.draw(R["PHASE"], fb.phase_text, unscii_8, ALIGN_CENTER, 0, WHITE)

    if fb.clock_accent:
        clock_col = colors.accent
    elif fb.clock_low:
        clock_col = colors.clock_warning
    else:
        clock_col = colors.clock_normal
    ck = geo["CLOCK"]
    writer.aligned_text(fb.clock_text, ck[0], ck[1], ck[2], ALIGN_CENTER, clock_col, font=_CLOCK_FONT)

    # --- Down & distance + possession arrow ---
    if fb.situation_text:
        sit_col = colors.clock_warning if fb.red_zone else WHITE
        writer.draw(R["SITUATION"], fb.situation_text, spleen_5x8, ALIGN_CENTER, 0, sit_col)
        if fb.sit_arrow_x >= 0:
            arrow_col = sit_col if fb.red_zone else (
                fb.home_color if fb.sit_arrow_right else fb.away_color)
            _draw_football_arrow(display, fb.sit_arrow_x, geo["SITUATION"][1] + 1,
                                 fb.sit_arrow_right, arrow_col)

    # --- Bottom zone: toast > play flash > field strip ---
    if not _render_toast(writer, regions, state, now_ms):
        play = state.play
        play_window_ms = time.ticks_diff(now_ms, play.updated_ms)
        if bool(play.text) and play.updated_ms != 0 and play_window_ms < play.display_ms:
            # No glyph fallback — see render_mlb_live (strip is an invariant).
            writer.draw_strip(
                regions.play_text, play.strip,
                ALIGN_LEFT, play_elapsed_ms, WHITE,
                pause_ms=PLAY_TEXT_SCROLL_PAUSE_MS,
                pixels_per_second=screen_geometry.GAME_SCROLL_PX_PER_SEC,
            )
        else:
            _draw_football_field(display, fb)

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


def render_menu(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """Full-screen league-select menu (see MenuView in state.py).

    Pure function of (latched menu view, now_ms): every label was
    pre-rendered to a strip and every layout value (visible window, scroll
    thumb) pre-computed on Core 0 by MenuController. The highlighted row's
    marquee rides the WALL rail — elapsed derived statelessly from
    `menu.updated_ms` (toast-lifetime pattern) — so the menu adds zero
    cross-frame state to Core 1 (see the mutation contract above LoopState).
    Non-highlighted rows draw at scroll offset 0 and clip in their Region:
    the approved "truncate unless highlighted" behavior.
    """
    menu = state.menu
    color = colors.primary
    elapsed = time.ticks_diff(now_ms, menu.updated_ms)

    display.fill(BLACK)
    for i in range(len(menu.row_strips)):
        y = _MENU_TOP + i * _MENU_ROW_H
        sel = i == menu.highlight
        if sel:
            display.fill_rect(_MENU_HILIGHT_X, y, _MENU_HILIGHT_W,
                              _MENU_ROW_H, color)
        fg = BLACK if sel else color
        # Checkbox stays outside the highlight bar and always draws in the
        # primary color, so checked/unchecked reads the same on every row.
        display.rect(_MENU_CHECKBOX_X, y + 1, 7, 7, color)
        if menu.row_checked[i]:
            display.fill_rect(_MENU_CHECKBOX_X + 2, y + 3, 3, 3, color)
        strip = menu.row_strips[i]
        if strip is not None:
            writer.draw_strip(regions.menu_rows[i], strip, ALIGN_LEFT,
                              elapsed if sel else 0, fg)

    if menu.thumb_y >= 0:
        display.fill_rect(_MENU_BAR_X, _MENU_TOP, 2,
                          _MENU_SEP_Y - _MENU_TOP - 1, DIM_GRAY)
        display.fill_rect(_MENU_BAR_X, menu.thumb_y, 2, menu.thumb_h, color)

    display.hline(1, _MENU_SEP_Y, DISPLAY_WIDTH - 2, DIM_GRAY)
    done_sel = menu.highlight == -1
    if done_sel:
        display.fill_rect(1, _MENU_DONE_Y, DISPLAY_WIDTH - 2,
                          DISPLAY_HEIGHT - 1 - _MENU_DONE_Y, color)
    # 4 static glyphs/frame — negligible next to the strip blits, and
    # allocation-free like every FontWriter path.
    writer.aligned_text("DONE", 0, _MENU_DONE_Y + 1, DISPLAY_WIDTH,
                        ALIGN_CENTER, BLACK if done_sel else color,
                        color if done_sel else BLACK, unscii_8)


def render_frame(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int, view_elapsed_ms: int, play_elapsed_ms: int) -> None:
    """
    Render a frame based on current display state.

    Pure function of its time inputs — nothing here queries the clock.

    The league menu preempts the mode dispatch entirely: while
    `state.menu.active` the menu IS the frame (rotation, poll commits, and
    toasts continue underneath, invisible — toast draws live inside the
    bypassed mode renderers, so suppression is structural, not special-cased).

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
    if state.menu.active:
        render_menu(display, writer, regions, state, colors, now_ms)
        return
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
    'mlb_live': render_mlb_live,
    'pregame': lambda d, w, r, s, c, now, view, play: render_pregame(d, w, r, s, c, now, view),
    'final': lambda d, w, r, s, c, now, view, play: render_final(d, w, r, s, c, now, view),
    'soccer_live': render_soccer_live,
    'soccer_final': lambda d, w, r, s, c, now, view, play: render_soccer_final(d, w, r, s, c, now, view),
    'nba_live': render_nba_live,
    'football_live': render_football_live,
}


# =============================================================================
# Display thread (runs on Core 1)
# =============================================================================

# Modes with no time-driven animation: re-rendering is only needed when a new
# commit lands (or a toast is fading out). Modes not listed here — the game
# screens and 'setup' — animate every frame (scrolls, pulses, clocks) and
# always redraw.
_STATIC_MODES = ('idle', 'no_games', 'error', 'startup', 'updating')


# =============================================================================
# Core 1 mutation contract
# =============================================================================
#
# Everything Core 1 may write, enumerated. If a change doesn't fit one of
# these four buckets, it does not belong on the render path:
#
#   1. LoopState — ALL cross-frame state: any value whose meaning survives
#      from one frame to the next (pacing, the frame rail and its epoch
#      latches, the render-skip memo, telemetry counters). Exactly one
#      instance, local to run_display_thread, NEVER passed into render_frame
#      or anything below it. Audit rule: the name `ls` must not appear below
#      render_frame in this file — renderers structurally CANNOT touch
#      cross-frame state because the reference does not exist in their scope.
#   2. Registered scratch (scratch_buffers() / SCRATCH_PALETTE_ENTRIES) —
#      preallocated buffers the draw stack may mutate under the
#      write-before-read contract: every slot read must have been written
#      earlier in the SAME draw call. tools/preview poisons all registered
#      scratch with sentinels before every rendered frame, so a violation
#      (scratch silently promoted to cross-frame state) renders garbage and
#      fails the golden tests deterministically. New scratch MUST be added
#      to the registry, or it escapes that tripwire.
#   3. Draw targets — the display framebuffer and its Region views. They are
#      the product; every renderer fully redraws what it owns, so nothing in
#      them carries meaning into the next frame.
#   4. ThreadHealth.frame_seq — the single deliberately cross-core counter
#      (watchdog liveness). It stays on ThreadHealth rather than LoopState
#      BECAUSE it is cross-core: LoopState's safety argument is thread
#      confinement, and a Core-0-readable field would force memory-model
#      reasoning onto the whole object.
#
# Special case: base_marker_sprite.palette and football_field_sprite.palette
# are tinted in place and restored in a `finally` (_draw_base_markers,
# _draw_football_field). Their steady-state entries (gold markers, endzone
# placeholders) are immutable config that must survive across frames, so
# they are NOT scratch and NOT poisonable — the restore is what keeps them
# contract-clean.
#
# The system's other two mutation domains belong to Core 0: the
# TripleBufferedState mailbox (state.py — Core 0 writes, Core 1 latches a
# read-only snapshot once per frame) and the LogoPool slots (written only by
# the poller task; Core 1 blits whatever slots the latched state references).
#
# Violation examples — all real failure shapes, and how to do it properly:
#   - A module-level memo updated from a renderer ("cache the derived
#     palette between frames"): cross-frame state outside LoopState.
#     Recompute per frame, or derive it on Core 0 in a state setter.
#   - Scratch read before write ("it still holds last frame's value"):
#     exactly the _cycle_phase early-return bug this contract came from.
#     Write every slot you read, every call; the poisoning catches you if
#     you don't.
#   - Threading LoopState (or a field of it) into a render function: breaks
#     the reachability guarantee. Pass plain values (as render_frame does
#     with the elapsed rails), never the bag.
#   - Formatting or allocating on Core 1 ("just one f-string"): GC pauses on
#     the render thread. Strings and marquee strips are pre-built on Core 0
#     (see state.py setters and fonts.render_strip).


class LoopState:
    """The ONLY home for Core 1 cross-frame state (contract above).

    One instance per display-thread lifetime, created at the top of
    run_display_thread and shared with nothing: not with Core 0, not with
    the render stack. tools/preview instantiates this same class, so the
    golden tests exercise the exact latch arithmetic the firmware runs.

    Fields are time-base and bookkeeping ONLY. Content (strings, strips,
    colors, layout) always comes from the latched mailbox buffer each
    frame — a content field on this class is a contract violation.
    """

    def __init__(self, now_ms: int, mem_alloc: int = 0) -> None:
        # --- Pacing: each iteration targets deadline + FRAME_MS ---
        self.deadline = now_ms

        # --- Frame rail + epoch latches ---
        # The rail advances exactly FRAME_MS per loop tick, so motion derived
        # from it holds position through a stalled frame instead of jumping
        # (the wall clock keeps running through a GC pause; this one
        # doesn't). Core 0's epoch stamps (animation_start_ms,
        # play.updated_ms) are in the ticks domain and are never subtracted
        # against this rail directly — advance_and_latch translates an epoch
        # CHANGE into "frame-rail time zero".
        self.anim_ms = 0
        self.view_stamp = -1
        self.view_epoch = 0
        self.play_stamp = -1
        self.play_epoch = 0
        self.view_elapsed = 0
        self.play_elapsed = 0

        # --- Render-skip memo (static screens skip unchanged redraws) ---
        self.last_rendered_seq = -1
        self.last_frame_had_toast = False

        # --- Frame-health telemetry (reported every 60 s at DEBUG) ---
        self.hb_prev_ms = now_ms
        self.hb_frames = 0
        self.hb_slow = 0
        self.hb_worst = 0
        self.hb_last_report = now_ms

        # --- [MEMPROF] window state (see MEM_PROFILE) ---
        self.mp_prev = mem_alloc
        self.mp_churn = 0
        self.mp_gcs = 0
        self.mp_worst_d = 0
        self.mp_freed = 0
        self.mp_maxdrop = 0
        self.mp_over = 0
        self.mp_report_ms = now_ms

    def advance_and_latch(self, state) -> None:
        """Advance the frame rail one FRAME_MS and re-latch epochs on change.

        Afterwards view_elapsed / play_elapsed hold the frame-rail elapsed
        values render_frame expects. Shared with tools/preview's render
        loop — never duplicate this arithmetic elsewhere.
        """
        self.anim_ms += FRAME_MS
        if state.animation_start_ms != self.view_stamp:
            self.view_stamp = state.animation_start_ms
            self.view_epoch = self.anim_ms
        if state.play.updated_ms != self.play_stamp:
            self.play_stamp = state.play.updated_ms
            self.play_epoch = self.anim_ms
        self.view_elapsed = self.anim_ms - self.view_epoch
        self.play_elapsed = self.anim_ms - self.play_epoch


def scratch_buffers(writer: FontWriter) -> tuple:
    """Every whole-buffer scratch object on the Core 1 render path.

    Contract (bucket 2 above): each is fully written before every read
    within a single draw call. tools/preview fills these with sentinels
    before every frame; anything relying on a leftover value breaks the
    golden tests. Add new scratch here, or it escapes the tripwire.
    """
    return (_base_pal, _CYCLE_OUT) + writer.scratch_buffers()


# Palette ENTRIES rewritten in place before every blit that reads them, as
# (palette FrameBuffer, first entry, count). Entry 0 of each is the immutable
# transparency KEY — config, not scratch — hence entry-level registration
# rather than whole-buffer poisoning.
SCRATCH_PALETTE_ENTRIES = (
    (toast_spinner_sprite.palette, 1, 12),     # trail, all 12 rewritten per draw
    (toast_lock_closed_sprite.palette, 1, 1),  # tint, rewritten per draw
    (toast_lock_open_sprite.palette, 1, 1),    # tint, rewritten per draw
)


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

    ALL cross-frame state — pacing, frame rail, epoch latches, skip memo,
    telemetry — lives on ONE LoopState instance local to this function (see
    the Core 1 mutation contract above LoopState). The loop body binds only
    within-iteration temporaries; anything that must outlive an iteration
    goes on `ls`, and `ls` never crosses into the render stack.

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

    # ALL cross-frame state lives on this one object — see the Core 1
    # mutation contract above LoopState. Nothing else in this loop body may
    # outlive an iteration (plain temporaries within one tick are fine), and
    # `ls` is never passed to render_frame or anything below it.
    # [MEMPROF] note: per-tick mem_alloc deltas — positive = bytes allocated
    # since last tick (render + whatever Core 0 did meanwhile); negative = a
    # collection ran. `mp_over` counts EVERY tick that blew the 50 ms budget
    # (health's `slow` only counts >70 ms), so visible stutters and GC
    # events can be correlated directly. Frame-health telemetry is how the
    # 9 FPS scroll regression was found — cheap enough to keep.
    ls = LoopState(time.ticks_ms(), gc.mem_alloc() if MEM_PROFILE else 0)

    while True:
        # Heartbeat for the watchdog feeder: per tick, not per render.
        health.frame_seq = (health.frame_seq + 1) & 0x3FFFFFF

        ls.deadline = time.ticks_add(ls.deadline, FRAME_MS)

        try:
            now_ms = time.ticks_ms()

            if MEM_PROFILE:
                _mp_now = gc.mem_alloc()
                _mp_d = _mp_now - ls.mp_prev
                ls.mp_prev = _mp_now
                if _mp_d >= 0:
                    ls.mp_churn += _mp_d
                    if _mp_d > ls.mp_worst_d:
                        ls.mp_worst_d = _mp_d
                else:
                    # mem_alloc dropped: a real collection (drop ~= all
                    # garbage since the last one) or a C-level explicit
                    # free (network buffers etc., typically small). Track
                    # sizes so the two are distinguishable in the report.
                    ls.mp_gcs += 1
                    ls.mp_freed -= _mp_d
                    if -_mp_d > ls.mp_maxdrop:
                        ls.mp_maxdrop = -_mp_d

            _hb_period = time.ticks_diff(now_ms, ls.hb_prev_ms)
            ls.hb_prev_ms = now_ms
            ls.hb_frames += 1
            if _hb_period > ls.hb_worst:
                ls.hb_worst = _hb_period
            if _hb_period > FRAME_MS + (FRAME_MS * 2) // 5:
                ls.hb_slow += 1

            # Latch the latest committed state for this frame.
            state, seq = acquire_display_state()

            # Advance the frame rail and re-latch epochs on change.
            ls.advance_and_latch(state)

            # "Active" includes the overlay's fade-out tail so static modes
            # keep rendering until the dim has fully eased back out.
            toast_active = (_toast_active(state, now_ms)
                            or _toast_overlay_fading(state, now_ms))
            skip = (seq == ls.last_rendered_seq
                    and state.mode in _STATIC_MODES
                    and not toast_active
                    and not ls.last_frame_had_toast
                    # The menu take-over animates (marquee) regardless of
                    # the underlying mode — never skip while it's up.
                    and not state.menu.active)

            if not skip:
                render_frame(display, writer, regions, state, state.ui_colors, now_ms,
                             ls.view_elapsed, ls.play_elapsed)
                display.show()
                ls.last_rendered_seq = seq
                ls.last_frame_had_toast = toast_active

            if time.ticks_diff(now_ms, ls.hb_last_report) >= 60_000:
                if logger.level >= DEBUG:
                    logger.debug(
                        "[DISPLAY] health: frames=%d slow=%d worst=%dms"
                        % (ls.hb_frames, ls.hb_slow, ls.hb_worst)
                    )
                ls.hb_frames = ls.hb_slow = ls.hb_worst = 0
                ls.hb_last_report = now_ms

            if MEM_PROFILE and time.ticks_diff(now_ms, ls.mp_report_ms) >= 10_000:
                _mp_play = state.play
                if logger.level >= DEBUG:
                    logger.debug(
                        "[MEMPROF] churn=%dB/s gc=%d freed=%dB maxdrop=%dB worstd=%dB over=%d mode=%s play=%d strip=%d"
                        % (ls.mp_churn // 10, ls.mp_gcs, ls.mp_freed, ls.mp_maxdrop,
                           ls.mp_worst_d, ls.mp_over, state.mode, len(_mp_play.text),
                           0 if _mp_play.strip is None else 1)
                    )
                ls.mp_churn = ls.mp_gcs = ls.mp_worst_d = ls.mp_freed = ls.mp_maxdrop = ls.mp_over = 0
                ls.mp_report_ms = now_ms

        except Exception as e:
            # Guarded: this path can repeat every frame while erroring, so
            # don't build the message when ERROR logging is off.
            if logger.level >= ERROR:
                logger.error(f"[DISPLAY] thread error: {e}")

        # Deadline pacing: sleep whatever remains of this frame's budget.
        remaining = time.ticks_diff(ls.deadline, time.ticks_ms())
        if remaining > 0:
            time.sleep_ms(remaining)
        else:
            # Overran the budget (e.g. a GC pause): re-anchor instead of
            # bursting frames to catch up.
            ls.mp_over += 1
            ls.deadline = time.ticks_ms()

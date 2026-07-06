"""
Display rendering and thread management for the Pico Scoreboard.

Provides render functions for non-game display modes (startup, idle,
no_games, setup, error), the logo caching system, animation primitives,
and the Core 1 display thread.

Render functions are pure readers: every string they draw was pre-built on
Core 0 when the state changed (see scoreboard/state.py), so the render loop
does no per-frame text formatting.
"""

import time
import framebuf
from machine import Pin
from hub75 import Hub75Driver, Hub75Display, row_addressing
from hub75.native import pack_hsv_to_rgb565
from scoreboard.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, measure_text, ALIGN_LEFT, ALIGN_CENTER
from scoreboard.inning_half import TOP, BOTTOM
from scoreboard.state import StateBuffer, ThreadHealth, UiColors
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient
import scoreboard.logger as logger
from scoreboard.logger import ERROR
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

# Fixed colors
BLACK = 0
WHITE = rgb565(255, 255, 255)
DIM_GRAY = rgb565(96, 96, 96)


# Minimum brightest-channel value for team colors. Teams whose primary is
# darker than this (e.g. Yankees/Brewers navy, White Sox near-black) get
# scaled up proportionally so the text stays legible on the black panel.
# Preserves hue for chromatic colors; near-neutrals move toward bright gray.
_TEAM_COLOR_MIN_CHANNEL = 128


def _team_color_to_rgb565(packed: int) -> int:
    r = (packed >> 16) & 0xFF
    g = (packed >> 8) & 0xFF
    b = packed & 0xFF
    m = r if r >= g and r >= b else (g if g >= b else b)
    if m < _TEAM_COLOR_MIN_CHANNEL:
        if m == 0:
            r = g = b = _TEAM_COLOR_MIN_CHANNEL
        else:
            scale = _TEAM_COLOR_MIN_CHANNEL / m
            r = int(r * scale)
            g = int(g * scale)
            b = int(b * scale)
    return rgb565(r, g, b)


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

# Play-by-play flash: the most-recent play text preempts the pitcher/batter
# view after a new play is detected. Its display window is computed per play
# (see play_text_display_ms): exactly one scroll cycle — full start pause,
# scroll to the end, full end pause — so long plays get the time they need
# and short plays don't linger. Scroll tunables are kept separate from the
# default scroll feel.
PLAY_TEXT_SCROLL_PAUSE_MS = 1000
PLAY_TEXT_SCROLL_PX_PER_SEC = 30

# Button-feedback toast: how long a transient overlay (SKIPPING... / LOCKED)
# stays on screen after set_toast().
TOAST_DISPLAY_MS = 1500


_ORDINALS = (
    "", "1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th", "9th",
    "10th", "11th", "12th", "13th", "14th", "15th", "16th", "17th", "18th", "19th", "20th",
    "21st", "22nd", "23rd", "24th", "25th", "26th", "27th", "28th", "29th", "30th",
)

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
    scroll_ms = (max_scroll * 1000) // PLAY_TEXT_SCROLL_PX_PER_SEC if max_scroll > 0 else 0
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

    writer.draw(regions.startup_step, startup.step_text, spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
    writer.draw(regions.startup_operation, startup.operation, spleen_5x8, ALIGN_CENTER, 0, colors.primary)
    if startup.detail:
        writer.draw(regions.startup_detail, startup.detail, spleen_5x8, ALIGN_CENTER, 0, colors.secondary)


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
    return (toast.updated_ms != 0 and bool(toast.text)
            and time.ticks_diff(now_ms, toast.updated_ms) < TOAST_DISPLAY_MS)


def _render_toast(writer: FontWriter, regions: Regions, state: StateBuffer, now_ms: int) -> bool:
    """Draw the transient toast overlay if active. Returns True if drawn."""
    if not _toast_active(state, now_ms):
        return False
    regions.play_text.fill(BLACK)
    writer.draw(regions.play_text, state.toast.text, unscii_16, ALIGN_CENTER, 0, WHITE)
    return True


def render_game(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    display.fill(BLACK)

    live = state.game.live
    if live is None:
        render_idle(display, writer, regions, colors)
        _render_toast(writer, regions, state, now_ms)
        return

    # --- Sprites ---

    display.blit(field_sprite.data, field_sprite.X, field_sprite.Y, field_sprite.KEY, field_sprite.palette)  # type: ignore

    if live.bases.first:
        display.blit(base_marker_sprite.data, first_base_loc.X, first_base_loc.Y, base_marker_sprite.KEY, base_marker_sprite.palette)  # type: ignore
    if live.bases.second:
        display.blit(base_marker_sprite.data, second_base_loc.X, second_base_loc.Y, base_marker_sprite.KEY, base_marker_sprite.palette)  # type: ignore
    if live.bases.third:
        display.blit(base_marker_sprite.data, third_base_loc.X, third_base_loc.Y, base_marker_sprite.KEY, base_marker_sprite.palette)  # type: ignore

    if state.away_logo is not None:
        display.blit(state.away_logo, away_logo_loc.X, away_logo_loc.Y)
    if state.home_logo is not None:
        display.blit(state.home_logo, home_logo_loc.X, home_logo_loc.Y)

    half = live.inning.half
    if half is TOP:
        display.blit(inning_top_sprite.data, inning_top_sprite.X, inning_top_sprite.Y, inning_top_sprite.KEY, inning_top_sprite.palette)  # type: ignore
    elif half is BOTTOM:
        display.blit(inning_bottom_sprite.data, inning_bottom_sprite.X, inning_bottom_sprite.Y, inning_bottom_sprite.KEY, inning_bottom_sprite.palette)  # type: ignore

    # --- Count dots ---

    balls_critical = live.count.balls == 3
    strikes_critical = live.count.strikes == 2
    outs_critical = live.count.outs == 2

    if balls_critical or strikes_critical or outs_critical:
        v = 191 + ((pulse(now_ms) * 64) >> 8)
        pulsed = pack_hsv_to_rgb565(0, 0, v)
    else:
        pulsed = None

    _draw_count_dots(display, ball_values_loc, live.count.balls, pulsed if balls_critical else None)
    _draw_count_dots(display, strike_values_loc, live.count.strikes, pulsed if strikes_critical else None)
    _draw_count_dots(display, out_values_loc, live.count.outs, pulsed if outs_critical else None)

    # --- Text ---
    # Scores stay on the zero-alloc integer() path.
    writer.integer(live.away.score, away_score_loc.X, away_score_loc.Y, away_score_loc.WIDTH, ALIGN_CENTER, WHITE, font=unscii_16)
    writer.integer(live.home.score, home_score_loc.X, home_score_loc.Y, home_score_loc.WIDTH, ALIGN_CENTER, WHITE, font=unscii_16)

    inning_num = live.inning.number
    inning_text = _ORDINALS[inning_num] if inning_num < len(_ORDINALS) else str(inning_num)
    writer.draw(regions.inning, inning_text, unscii_8, ALIGN_CENTER, 0, WHITE)

    writer.draw(regions.ball_label, "B", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)
    writer.draw(regions.strike_label, "S", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)
    writer.draw(regions.out_label, "O", unscii_8, ALIGN_LEFT, 0, DIM_GRAY)

    if half is TOP:
        pitch_color = _team_color_to_rgb565(live.home.colors.primary)
        bat_color = _team_color_to_rgb565(live.away.colors.primary)
    elif half is BOTTOM:
        pitch_color = _team_color_to_rgb565(live.away.colors.primary)
        bat_color = _team_color_to_rgb565(live.home.colors.primary)
    else:
        pitch_color = DIM_GRAY
        bat_color = DIM_GRAY

    # Bottom strip priority: toast (button feedback) > play flash > pitcher/batter.
    if _render_toast(writer, regions, state, now_ms):
        return

    play = state.game.play
    play_elapsed = time.ticks_diff(now_ms, play.updated_ms)
    show_play = bool(play.text) and play.updated_ms != 0 and play_elapsed < play.display_ms

    if show_play:
        writer.draw(
            regions.play_text, play.text, PLAY_TEXT_FONT,
            ALIGN_LEFT, play_elapsed, WHITE,
            pause_ms=PLAY_TEXT_SCROLL_PAUSE_MS,
            pixels_per_second=PLAY_TEXT_SCROLL_PX_PER_SEC,
        )
    else:
        at_bat = live.at_bat
        if at_bat is not None:
            elapsed_ms = time.ticks_diff(now_ms, state.animation_start_ms)
            writer.draw(regions.pitcher_name, at_bat.pitcher, spleen_5x8, ALIGN_CENTER, elapsed_ms, pitch_color)
            writer.draw(regions.batter_name, at_bat.batter, spleen_5x8, ALIGN_CENTER, elapsed_ms, bat_color)

        writer.draw(regions.pitcher_label, "PIT", unscii_8, ALIGN_LEFT, 0, pitch_color)
        writer.draw(regions.batter_label, "BAT", unscii_8, ALIGN_LEFT, 0, bat_color)


def render_frame(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """
    Render a frame based on current display state.

    Pure function: all timing-dependent computations use the passed now_ms
    timestamp rather than querying time internally.
    """
    mode = state.mode

    if mode == 'startup':
        render_startup(display, writer, regions, state, colors)
    elif mode == 'idle':
        render_idle(display, writer, regions, colors)
    elif mode == 'no_games':
        render_no_games(display, writer, regions, state, colors, now_ms)
    elif mode == 'setup':
        render_setup(display, writer, regions, state, colors, now_ms)
    elif mode == 'error':
        render_error(display, writer, regions, state, colors)
    elif mode == 'game':
        render_game(display, writer, regions, state, colors, now_ms)
    else:
        render_idle(display, writer, regions, colors)


# =============================================================================
# Display thread (runs on Core 1)
# =============================================================================

# Modes with no time-driven animation: re-rendering is only needed when a new
# commit lands (or a toast is fading out). 'game' and 'setup' animate every
# frame (scrolling text, pulsing count dots) and always redraw.
_STATIC_MODES = ('idle', 'no_games', 'error', 'startup')


def run_display_thread(display: Hub75Display, writer: FontWriter, regions: Regions, health: ThreadHealth) -> None:
    """
    Main entry point for Core 1 display thread.

    Runs a constant 20 FPS loop latching state from the mailbox and
    rendering to the display. Static screens are only re-rendered when a
    new commit lands, so an idle scoreboard isn't redrawing 20x/second.
    `health.frame_seq` is bumped every tick (rendered or skipped) so the
    Core 0 watchdog feeder can distinguish a hung thread from a quiet one.

    Core 1 avoids heap allocation on the steady-state path: all strings are
    pre-built on Core 0, glyph blits reuse pre-allocated specs, and scores
    use the cached-digit integer() path. (Small per-character memoryview
    allocations from font glyph lookup remain — see BACKLOG.)

    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    Regions are pre-allocated on Core 0 and read-only here.
    """
    from scoreboard.state import acquire_display_state

    logger.debug("[DISPLAY] thread starting: core=1 rate=20fps")

    last_rendered_seq = -1
    last_frame_had_toast = False

    while True:
        # Heartbeat for the watchdog feeder: per tick, not per render.
        health.frame_seq = (health.frame_seq + 1) & 0x3FFFFFF

        try:
            now_ms = time.ticks_ms()

            # Latch the latest committed state for this frame.
            state, seq = acquire_display_state()

            toast_active = _toast_active(state, now_ms)
            skip = (seq == last_rendered_seq
                    and state.mode in _STATIC_MODES
                    and not toast_active
                    and not last_frame_had_toast)

            if not skip:
                render_frame(display, writer, regions, state, state.ui_colors, now_ms)
                display.show()
                last_rendered_seq = seq
                last_frame_had_toast = toast_active

        except Exception as e:
            # Guarded: this path can repeat every frame while erroring, so
            # don't build the message when ERROR logging is off.
            if logger.level >= ERROR:
                logger.error(f"[DISPLAY] thread error: {e}")

        # Constant 20 FPS tick for all animations
        time.sleep_ms(50)

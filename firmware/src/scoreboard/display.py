"""
Display rendering and thread management for the Pico Scoreboard.

Provides render functions for non-game display modes (startup, idle,
no_games, setup, error), the logo caching system, animation primitives,
and the Core 1 display thread.
"""

import math
import time
import framebuf
from machine import Pin
from hub75 import Hub75Driver, Hub75Display, row_addressing
from hub75.native import pack_hsv_to_rgb565
from scoreboard.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, ALIGN_LEFT, ALIGN_CENTER
from scoreboard.inning_half import Top, Bottom
from scoreboard.state import StateBuffer, UiColors
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient
from scoreboard.logger import DEBUG, ERROR
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


_TWO_PI = 2.0 * math.pi


def pulse(now_ms: int, hz: float = 1.0) -> float:
    """Sine-wave factor in [0.0, 1.0], cycling at `hz` cycles per second.

    Callers map the factor into whatever range they need — e.g.
    `V = 191 + int(pulse(now_ms) * 64)` for a subtle 75%→100% HSV brightness sweep.
    """
    return (math.sin(now_ms * hz * _TWO_PI / 1000.0) + 1.0) * 0.5

# Bright magenta, used as the transparency sentinel for compiled sprites.
# Matches _TRANSPARENT_RGB565 in tools/compile_layout.py. Sprites emit a
# per-module `KEY` constant that's already in the right form for their
# format (palette index for paletted sprites, this RGB565 value for RGB565
# sprites, or -1 for sprites with no transparent pixels) — prefer passing
# `sprite.KEY` to blit() rather than this constant directly.
MAGENTA_RGB565 = 0xF81F

# Display dimensions
DISPLAY_WIDTH = 128
DISPLAY_HEIGHT = 64


_ORDINALS = (
    "", "1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th", "9th",
    "10th", "11th", "12th", "13th", "14th", "15th", "16th", "17th", "18th", "19th", "20th",
)


# =============================================================================
# Animation primitives
# =============================================================================

def calculate_scroll_offset(
    text_width: int,
    display_width: int,
    elapsed_ms: int,
    pause_ms: int = 2000,
    pixels_per_second: int = 30
) -> int:
    """
    Pure function: Given dimensions and elapsed time, return pixel offset.

    The animation cycle is:
        [pause_start] -> [scrolling] -> [pause_end] -> repeat
    """
    max_scroll = text_width - display_width
    if max_scroll <= 0:
        return 0

    scroll_duration_ms = (max_scroll * 1000) // pixels_per_second
    total_cycle_ms = pause_ms + scroll_duration_ms + pause_ms

    position = elapsed_ms % total_cycle_ms

    if position < pause_ms:
        # Phase 1: Paused at start
        return 0
    elif position < pause_ms + scroll_duration_ms:
        # Phase 2: Scrolling
        scroll_position = position - pause_ms
        return (scroll_position * pixels_per_second) // 1000
    else:
        # Phase 3: Paused at end
        return max_scroll


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
        # the QR's footprint. Lines whose y-range sits entirely below the QR
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
    dot_sprite.palette.pixel(1, 0, active)
    for i in range(n_dots):
        dot_sprite.palette.pixel(2, 0, active if i < filled_count else default_fill)
        display.blit(
            dot_sprite.data,
            slice_mod.X + i * (dot_sprite.WIDTH + 1),  # type: ignore[attr-defined]
            slice_mod.Y,  # type: ignore[attr-defined]
            MAGENTA_RGB565,
            dot_sprite.palette
        )
    dot_sprite.palette.pixel(1, 0, default_outline)
    dot_sprite.palette.pixel(2, 0, default_fill)


# =============================================================================
# Logo buffer pool
# =============================================================================

# Pre-allocated logo buffer pool
_LOGO_POOL_SIZE = 8  # Max logos cached
_LOGO_WIDTH = 24
_LOGO_HEIGHT = 24
_LOGO_BUFFER_SIZE = _LOGO_WIDTH * _LOGO_HEIGHT * 2  # 1152 bytes per logo

_logo_buffers = [bytearray(_LOGO_BUFFER_SIZE) for _ in range(_LOGO_POOL_SIZE)]
_logo_cache = {}  # cache_key -> (slot_index, FrameBuffer)
_logo_lru = []    # LRU order: oldest first
_free_slots = set(range(_LOGO_POOL_SIZE))

print(f"[DISPLAY] logo pool initialized: {_LOGO_POOL_SIZE} buffers ({_LOGO_POOL_SIZE * _LOGO_BUFFER_SIZE // 1024} KB)")



async def get_logo_framebuffer(api_client: ScoreboardApiClient, cache_key: str, path: str) -> framebuf.FrameBuffer | None:
    """
    Get logo framebuffer from cache or fetch from API.

    Uses a pre-allocated buffer pool with LRU eviction to prevent
    memory fragmentation from repeated allocations.

    Args:
        api_client: The scoreboard API client.
        cache_key: Stable key for this logo in the LRU cache.
        path: Backend URL path that returns the raw logo bytes.
    """
    key = cache_key.lower()

    # Return cached if available
    if key in _logo_cache:
        _logo_lru.remove(key)
        _logo_lru.append(key)
        return _logo_cache[key][1]

    # Need to fetch - get a buffer slot
    if _free_slots:
        slot_index = _free_slots.pop()
    else:
        evict_key = _logo_lru.pop(0)
        slot_index = _logo_cache[evict_key][0]
        del _logo_cache[evict_key]
        if api_client._config.log_level >= DEBUG:
            print(f"[LOGO] evicted: key={evict_key} slot={slot_index}/{_LOGO_POOL_SIZE}")

    try:
        status, body = await api_client.get_team_logo_raw(
            path=path,
            width=_LOGO_WIDTH,
            height=_LOGO_HEIGHT,
            background_color="000000",
            accept="image/x-rgb565"
        )

        if status != 200:
            if api_client._config.log_level >= ERROR:
                print(f"[LOGO] fetch failed: key={key} status={status}")
            _free_slots.add(slot_index)
            return None

        buf = _logo_buffers[slot_index]
        buf[:len(body)] = body
        fb = framebuf.FrameBuffer(buf, _LOGO_WIDTH, _LOGO_HEIGHT, framebuf.RGB565)

        _logo_cache[key] = (slot_index, fb)
        _logo_lru.append(key)
        if api_client._config.log_level >= DEBUG:
            print(f"[LOGO] cached: key={key} slot={slot_index}/{_LOGO_POOL_SIZE}")
        return fb

    except Exception as e:
        if api_client._config.log_level >= ERROR:
            print(f"[LOGO] fetch error: key={key} error_type={type(e).__name__} {e}")
        _free_slots.add(slot_index)
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
    step = startup.step
    total = startup.total_steps
    operation = startup.operation
    detail = startup.detail

    writer.draw(regions.startup_title, "BOOTING", unscii_16, ALIGN_CENTER, 0, colors.accent)

    # Progress bar (80px wide, centered) at Y=24
    bar_width = 80
    bar_x = (DISPLAY_WIDTH - bar_width) // 2
    progress = int((step - 1) / total * 100) + (100 // total) // 2
    draw_progress_bar(display, bar_x, 24, bar_width, 8, progress, colors)

    writer.draw(regions.startup_step, f"{step}/{total}", spleen_5x8, ALIGN_LEFT, 0, colors.secondary)

    if len(operation) > 25:
        operation = operation[:24] + '.'
    writer.draw(regions.startup_operation, operation, spleen_5x8, ALIGN_CENTER, 0, colors.primary)

    if detail:
        if len(detail) > 25:
            detail = detail[:24] + '.'
        writer.draw(regions.startup_detail, detail, spleen_5x8, ALIGN_CENTER, 0, colors.secondary)


def render_idle(display: Hub75Display, writer: FontWriter, regions: Regions, colors: UiColors) -> None:
    """Render idle/waiting screen."""
    display.fill(BLACK)
    writer.draw(regions.idle_title, "PICO", unscii_16, ALIGN_CENTER, 0, colors.primary)
    writer.draw(regions.idle_subtitle, "SCOREBOARD", unscii_8, ALIGN_CENTER, 0, colors.accent)


def render_no_games(display: Hub75Display, writer: FontWriter, regions: Regions, colors: UiColors) -> None:
    """Render no games scheduled screen."""
    display.fill(BLACK)
    writer.draw(regions.no_games_title, "NO GAMES", unscii_16, ALIGN_CENTER, 0, colors.primary)
    writer.draw(regions.no_games_subtitle, "scheduled", spleen_5x8, ALIGN_CENTER, 0, colors.secondary)


def render_setup(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """
    Render setup mode screen with WiFi QR code and contextual information.

    Text is drawn first into full-width regions, then the QR (if available)
    is blitted on top. Long text that would have scrolled under the old
    narrow text-area behavior will now only scroll if it exceeds full
    display width.
    """
    display.fill(BLACK)

    setup = state.setup
    reason = setup.reason or 'no_config'
    ap_ssid = setup.ap_ssid or 'scoreboard'
    ap_ip = setup.ap_ip or '192.168.4.1'
    wifi_ssid = setup.wifi_ssid or ''
    elapsed_ms = time.ticks_diff(now_ms, state.animation_start_ms)

    if reason == 'bad_auth':
        writer.draw(regions.setup_title, "WRONG PASS", unscii_16, ALIGN_LEFT, 0, colors.clock_warning)
        writer.draw(regions.setup_line_18, f'for "{wifi_ssid}"', spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.primary)
        writer.draw(regions.setup_line_28, f'Scan/join "{ap_ssid}"', spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.secondary)
        writer.draw(regions.setup_line_44, f"Then go to {ap_ip}", spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
        writer.draw(regions.setup_line_54, "to fix password", spleen_5x8, ALIGN_LEFT, 0, colors.accent)

    elif reason == 'connection_failed':
        writer.draw(regions.setup_title, "WIFI FAIL", unscii_16, ALIGN_LEFT, 0, colors.clock_warning)
        writer.draw(regions.setup_line_18, f'"{wifi_ssid}"', spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.primary)
        writer.draw(regions.setup_line_28, f'Scan/join "{ap_ssid}"', spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.secondary)
        writer.draw(regions.setup_line_44, f"Then go to {ap_ip}", spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
        writer.draw(regions.setup_line_54, "to reconfigure", spleen_5x8, ALIGN_LEFT, 0, colors.accent)

    else:
        writer.draw(regions.setup_title, "SETUP", unscii_16, ALIGN_LEFT, 0, colors.accent)
        writer.draw(regions.setup_line_18, "Scan QR or join", spleen_5x8, ALIGN_LEFT, 0, colors.primary)
        writer.draw(regions.setup_line_28, f'"{ap_ssid}" WiFi', spleen_5x8, ALIGN_LEFT, elapsed_ms, colors.secondary)
        writer.draw(regions.setup_line_44, "Then go to", spleen_5x8, ALIGN_LEFT, 0, colors.secondary)
        writer.draw(regions.setup_line_54, ap_ip, spleen_5x8, ALIGN_LEFT, 0, colors.accent)

    # QR on top so it stays readable even if text drew underneath it.
    qr_fb = setup.qr_fb
    qr_width = setup.qr_width
    qr_palette = setup.qr_palette
    if qr_fb is not None and qr_palette is not None and qr_width > 0:
        qr_x = DISPLAY_WIDTH - qr_width - 2
        display.blit(qr_fb, qr_x, 2, -1, qr_palette)  # type: ignore


def render_error(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors) -> None:
    """Render error screen with multi-line details."""
    display.fill(BLACK)

    error = state.error
    title = error.title
    lines = error.lines

    writer.draw(regions.error_title, title or 'ERROR', unscii_16, ALIGN_CENTER, 0, colors.clock_warning)

    line_regions = (regions.error_line_0, regions.error_line_1, regions.error_line_2, regions.error_line_3)
    for i, line in enumerate(lines[:4]):
        display_line = line[:25] if len(line) > 25 else line
        writer.draw(line_regions[i], display_line, spleen_5x8, ALIGN_CENTER, 0, colors.primary)


def render_game(display: Hub75Display, writer: FontWriter, regions: Regions, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    display.fill(BLACK)

    live = state.game.live
    if live is None:
        render_idle(display, writer, regions, colors)
        return

    # --- Sprites ---

    display.blit(field_sprite.data, field_sprite.X, field_sprite.Y, MAGENTA_RGB565, field_sprite.palette)  # type: ignore

    if live.bases.first:
        display.blit(base_marker_sprite.data, first_base_loc.X, first_base_loc.Y, MAGENTA_RGB565, base_marker_sprite.palette)  # type: ignore
    if live.bases.second:
        display.blit(base_marker_sprite.data, second_base_loc.X, second_base_loc.Y, MAGENTA_RGB565, base_marker_sprite.palette)  # type: ignore
    if live.bases.third:
        display.blit(base_marker_sprite.data, third_base_loc.X, third_base_loc.Y, MAGENTA_RGB565, base_marker_sprite.palette)  # type: ignore

    if state.away_logo is not None:
        display.blit(state.away_logo, away_logo_loc.X, away_logo_loc.Y)
    if state.home_logo is not None:
        display.blit(state.home_logo, home_logo_loc.X, home_logo_loc.Y)

    half = live.inning.half
    if isinstance(half, Top):
        display.blit(inning_top_sprite.data, inning_top_sprite.X, inning_top_sprite.Y, MAGENTA_RGB565, inning_top_sprite.palette)  # type: ignore
    elif isinstance(half, Bottom):
        display.blit(inning_bottom_sprite.data, inning_bottom_sprite.X, inning_bottom_sprite.Y, MAGENTA_RGB565, inning_bottom_sprite.palette)  # type: ignore

    # --- Count dots ---

    balls_critical = live.count.balls == 3
    strikes_critical = live.count.strikes == 2
    outs_critical = live.count.outs == 2

    if balls_critical or strikes_critical or outs_critical:
        v = 191 + int(pulse(now_ms) * 64)
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

    if isinstance(half, Top):
        pitch_color = _team_color_to_rgb565(live.home.colors.primary)
        bat_color = _team_color_to_rgb565(live.away.colors.primary)
    elif isinstance(half, Bottom):
        pitch_color = _team_color_to_rgb565(live.away.colors.primary)
        bat_color = _team_color_to_rgb565(live.home.colors.primary)
    else:
        pitch_color = DIM_GRAY
        bat_color = DIM_GRAY

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
        render_no_games(display, writer, regions, colors)
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

def run_display_thread(display: Hub75Display, writer: FontWriter, regions: Regions, config: Config | None = None) -> None:
    """
    Main entry point for Core 1 display thread.

    Runs a constant 20 FPS loop reading state from the front buffer
    and rendering to the display. The fixed frame rate ensures smooth
    animations (scrolling text, clock updates).

    IMPORTANT: This function runs on Core 1 with ZERO memory allocations.
    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    UI colors are pre-computed on Core 0 and read from state.ui_colors.
    Regions are pre-allocated on Core 0 and read-only here.
    """
    from scoreboard.state import get_display_state

    if config is not None and config.log_level >= DEBUG:
        print("[DISPLAY] thread starting: core=1 rate=20fps")

    while True:
        try:
            now_ms = time.ticks_ms()

            # Read from front buffer (lock-protected capture)
            state = get_display_state()

            # Render frame using pre-computed colors (no allocation!)
            colors = state.ui_colors
            render_frame(display, writer, regions, state, colors, now_ms)
            display.show()

        except Exception as e:
            if config is not None and config.log_level >= ERROR:
                print(f"[DISPLAY] thread error: {e}")

        # Constant 20 FPS for all animations
        time.sleep_ms(50)

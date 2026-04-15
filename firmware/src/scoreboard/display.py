"""
Display rendering and thread management for the Pico Scoreboard.

Provides render functions for non-game display modes (startup, idle,
no_games, setup, error), the logo caching system, animation primitives,
and the Core 1 display thread.
"""

import time
import framebuf
from machine import Pin
from hub75 import Hub75Driver, Hub75Display, row_addressing
from scoreboard.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, ALIGN_CENTER
from scoreboard.state import StateBuffer, UiColors
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient
from scoreboard.logger import DEBUG, ERROR

# Fixed colors
BLACK = 0
WHITE = rgb565(255, 255, 255)

# Bright magenta, used as the transparency sentinel for compiled sprites.
# Matches _TRANSPARENT_RGB565 in tools/sprites/build.py. Sprites emit a
# per-module `KEY` constant that's already in the right form for their
# format (palette index for paletted sprites, this RGB565 value for RGB565
# sprites, or -1 for sprites with no transparent pixels) — prefer passing
# `sprite.KEY` to blit() rather than this constant directly.
MAGENTA_RGB565 = 0xF81F

# Display dimensions
DISPLAY_WIDTH = 128
DISPLAY_HEIGHT = 64


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


def init_display(config: Config) -> tuple[Hub75Driver, Hub75Display, FontWriter]:
    """
    Initialize and return HUB75 display hardware.

    Returns:
        Tuple of (driver, display, writer)
    """
    data_freq = config.data_frequency_hz
    brightness = config.brightness / 100.0
    gamma = config.gamma
    blanking_time = config.blanking_time_ns
    target_refresh_rate = config.target_refresh_rate

    driver = Hub75Driver(
        row_addressing=row_addressing.Direct(
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
    return driver, display, writer


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

def render_startup(display: Hub75Display, writer: FontWriter, state: StateBuffer, colors: UiColors) -> None:
    """Render startup/boot progress screen."""
    display.fill(BLACK)

    startup = state.startup
    step = startup.step
    total = startup.total_steps
    operation = startup.operation
    detail = startup.detail

    # Title "BOOTING" at top
    writer.aligned_text("BOOTING", 0, 4, DISPLAY_WIDTH, ALIGN_CENTER, colors.accent, font=unscii_16)

    # Progress bar (80px wide, centered) at Y=24
    bar_width = 80
    bar_x = (DISPLAY_WIDTH - bar_width) // 2
    progress = int((step - 1) / total * 100) + (100 // total) // 2
    draw_progress_bar(display, bar_x, 24, bar_width, 8, progress, colors)

    # Step indicator to the right of progress bar
    step_text = f"{step}/{total}"
    writer.text(step_text, bar_x + bar_width + 4, 24, colors.secondary, font=spleen_5x8)

    # Operation text (truncate to 25 chars)
    if len(operation) > 25:
        operation = operation[:24] + '.'
    writer.aligned_text(operation, 0, 42, DISPLAY_WIDTH, ALIGN_CENTER, colors.primary, font=spleen_5x8)

    # Detail text (truncate to 25 chars)
    if detail:
        if len(detail) > 25:
            detail = detail[:24] + '.'
        writer.aligned_text(detail, 0, 54, DISPLAY_WIDTH, ALIGN_CENTER, colors.secondary, font=spleen_5x8)


def render_idle(display: Hub75Display, writer: FontWriter, colors: UiColors) -> None:
    """Render idle/waiting screen."""
    display.fill(BLACK)

    # TEMPORARY: blit the mini field sprite plus base markers at the first
    # and third base slice coordinates, to verify the compiled sprite pipeline
    # (absolute layers, relative layers, slice coords, palettes) works end-to-end.
    # TODO: REMOVE after verification — replaced by real scoreboard rendering.
    from scoreboard.sprites import field as _field_test
    from scoreboard.sprites import base_marker as _base_marker_test
    from scoreboard.sprites import first_base as _first_base_test
    from scoreboard.sprites import third_base as _third_base_test
    display.blit(_field_test.data, _field_test.X, _field_test.Y, _field_test.KEY, _field_test.palette)  # type: ignore
    display.blit(_base_marker_test.data, _first_base_test.X, _first_base_test.Y, _base_marker_test.KEY, _base_marker_test.palette)  # type: ignore
    display.blit(_base_marker_test.data, _third_base_test.X, _third_base_test.Y, _base_marker_test.KEY, _base_marker_test.palette)  # type: ignore

    writer.aligned_text("PICO", 0, 16, DISPLAY_WIDTH, ALIGN_CENTER, colors.primary, font=unscii_16)
    writer.aligned_text("SCOREBOARD", 0, 40, DISPLAY_WIDTH, ALIGN_CENTER, colors.accent)


def render_no_games(display: Hub75Display, writer: FontWriter, colors: UiColors) -> None:
    """Render no games scheduled screen."""
    display.fill(BLACK)
    writer.aligned_text("NO GAMES", 0, 20, DISPLAY_WIDTH, ALIGN_CENTER, colors.primary, font=unscii_16)
    writer.aligned_text("scheduled", 0, 40, DISPLAY_WIDTH, ALIGN_CENTER, colors.secondary, font=spleen_5x8)


def render_setup(display: Hub75Display, writer: FontWriter, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """Render setup mode screen with WiFi QR code and contextual information."""
    display.fill(BLACK)

    setup = state.setup
    reason = setup.reason or 'no_config'
    ap_ssid = setup.ap_ssid or 'scoreboard'
    ap_ip = setup.ap_ip or '192.168.4.1'
    wifi_ssid = setup.wifi_ssid or ''
    animation_start_ms = state.animation_start_ms

    # Get QR code from state (generated on Core 0)
    qr_fb = setup.qr_fb
    qr_width = setup.qr_width
    qr_height = setup.qr_height
    qr_palette = setup.qr_palette

    # Render QR on right side if available
    text_area_width = DISPLAY_WIDTH
    qr_y = 0
    if qr_fb is not None and qr_palette is not None and qr_width > 0:
        qr_x = DISPLAY_WIDTH - qr_width - 2
        qr_y = 2
        display.blit(qr_fb, qr_x, qr_y, -1, qr_palette) # type: ignore
        text_area_width = qr_x - 4

    # Calculate where QR code ends vertically
    qr_bottom = qr_y + qr_height if qr_height > 0 else 0

    def render_scrolling_text(text: str, y: int, color: int, width: int | None = None) -> None:
        if width is None:
            width = DISPLAY_WIDTH if y >= qr_bottom else text_area_width
        pixel_width = writer.measure(text, spleen_5x8)
        if pixel_width > width and width > 0:
            elapsed = time.ticks_diff(now_ms, animation_start_ms)
            offset = calculate_scroll_offset(pixel_width, width, elapsed)
            writer.text(text, 2 - offset, y, color, font=spleen_5x8)
        else:
            writer.text(text, 2, y, color, font=spleen_5x8)

    if reason == 'bad_auth':
        writer.text("WRONG PASS", 2, 0, colors.clock_warning, font=unscii_16)
        render_scrolling_text(f'for "{wifi_ssid}"', 18, colors.primary)
        render_scrolling_text(f'Scan/join "{ap_ssid}"', 28, colors.secondary)
        writer.text(f"Then go to {ap_ip}", 2, 44, colors.secondary, font=spleen_5x8)
        writer.text("to fix password", 2, 54, colors.accent, font=spleen_5x8)

    elif reason == 'connection_failed':
        writer.text("WIFI FAIL", 2, 0, colors.clock_warning, font=unscii_16)
        render_scrolling_text(f'"{wifi_ssid}"', 18, colors.primary)
        render_scrolling_text(f'Scan/join "{ap_ssid}"', 28, colors.secondary)
        writer.text(f"Then go to {ap_ip}", 2, 44, colors.secondary, font=spleen_5x8)
        writer.text("to reconfigure", 2, 54, colors.accent, font=spleen_5x8)

    else:
        writer.text("SETUP", 2, 0, colors.accent, font=unscii_16)
        writer.text("Scan QR or join", 2, 18, colors.primary, font=spleen_5x8)
        render_scrolling_text(f'"{ap_ssid}" WiFi', 28, colors.secondary)
        writer.text("Then go to", 2, 44, colors.secondary, font=spleen_5x8)
        writer.text(ap_ip, 2, 54, colors.accent, font=spleen_5x8)


def render_error(display: Hub75Display, writer: FontWriter, state: StateBuffer, colors: UiColors) -> None:
    """Render error screen with multi-line details."""
    display.fill(BLACK)

    error = state.error
    title = error.title
    lines = error.lines

    # Title in warning color at top
    writer.aligned_text(title or 'ERROR', 0, 0, DISPLAY_WIDTH, ALIGN_CENTER, colors.clock_warning, font=unscii_16)

    # Detail lines (up to 4, using spleen_5x8)
    y_start = 24
    line_height = 10
    for i, line in enumerate(lines[:4]):
        display_line = line[:25] if len(line) > 25 else line
        writer.aligned_text(display_line, 0, y_start + (i * line_height), DISPLAY_WIDTH, ALIGN_CENTER, colors.primary, font=spleen_5x8)


def render_frame(display: Hub75Display, writer: FontWriter, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """
    Render a frame based on current display state.

    Pure function: all timing-dependent computations use the passed now_ms
    timestamp rather than querying time internally.
    """
    mode = state.mode

    if mode == 'startup':
        render_startup(display, writer, state, colors)
    elif mode == 'idle':
        render_idle(display, writer, colors)
    elif mode == 'no_games':
        render_no_games(display, writer, colors)
    elif mode == 'setup':
        render_setup(display, writer, state, colors, now_ms)
    elif mode == 'error':
        render_error(display, writer, state, colors)
    else:
        render_idle(display, writer, colors)


# =============================================================================
# Display thread (runs on Core 1)
# =============================================================================

def run_display_thread(display: Hub75Display, writer: FontWriter, config: Config | None = None) -> None:
    """
    Main entry point for Core 1 display thread.

    Runs a constant 20 FPS loop reading state from the front buffer
    and rendering to the display. The fixed frame rate ensures smooth
    animations (scrolling text, clock updates).

    IMPORTANT: This function runs on Core 1 with ZERO memory allocations.
    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    UI colors are pre-computed on Core 0 and read from state.ui_colors.
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
            render_frame(display, writer, state, colors, now_ms)
            display.show()

        except Exception as e:
            if config is not None and config.log_level >= ERROR:
                print(f"[DISPLAY] thread error: {e}")

        # Constant 20 FPS for all animations
        time.sleep_ms(50)

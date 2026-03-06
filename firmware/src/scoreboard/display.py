"""
Display rendering and thread management for the Pico Scoreboard.

Provides render functions for different display modes (startup, idle, game, error),
the logo caching system, animation primitives, and the Core 1 display thread.
"""

import time
import framebuf
from machine import Pin
from hub75 import Hub75Driver, Hub75Display, row_addressing, gamma as gamma_mod
from scoreboard.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, ALIGN_LEFT, ALIGN_CENTER, ALIGN_RIGHT
from scoreboard.models import Color, PregameGame, LiveGame, FinalGame, STATE_PREGAME, STATE_LIVE, STATE_FINAL
from scoreboard.state import StateBuffer, UiColors
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient
from scoreboard.sprites import field as field_sprite
from scoreboard.sprites import ball as ball_sprite

# Fixed colors
BLACK = 0
WHITE = rgb565(255, 255, 255)

# Display dimensions
DISPLAY_WIDTH = 128
DISPLAY_HEIGHT = 64

# Layout constants (derived from display dimensions)
LOGO_SIZE = 24
LOGO_PADDING = 1
AWAY_LOGO_X = LOGO_PADDING                              # 1
HOME_LOGO_X = DISPLAY_WIDTH - LOGO_SIZE - LOGO_PADDING  # 103

# Timeout bars (between logos and scores)
TIMEOUT_Y = 25         # 1px below logo bottom (Y=24)
TIMEOUT_BAR_W = 6      # Width of each bar
TIMEOUT_BAR_H = 1      # Height of each bar
TIMEOUT_GAP = 1        # Gap between bars
TIMEOUT_COUNT = 3      # Number of timeout bars
# Total width: 6+1+6+1+6 = 20px, centered under 24px logo
TIMEOUT_TOTAL_W = TIMEOUT_COUNT * TIMEOUT_BAR_W + (TIMEOUT_COUNT - 1) * TIMEOUT_GAP  # 20
TIMEOUT_OFFSET_X = (LOGO_SIZE - TIMEOUT_TOTAL_W) // 2  # 2px inset from logo edge

# Score positioning (below timeout bars)
SCORE_Y = 27  # Below timeout bars (which end at Y=25)

# Center area (between logos)
CENTER_X = AWAY_LOGO_X + LOGO_SIZE                      # 25
CENTER_WIDTH = HOME_LOGO_X - CENTER_X                   # 78

# Quarter/clock horizontal split: 1/3 for quarter, 2/3 for clock
QUARTER_WIDTH = CENTER_WIDTH // 3                       # 26
QUARTER_X = CENTER_X                                    # 25
CLOCK_WIDTH = CENTER_WIDTH - QUARTER_WIDTH              # 52
CLOCK_X = CENTER_X + QUARTER_WIDTH                      # 51

# Quarter/clock line positioning
# Clock uses unscii_16 (16px), quarter uses unscii_8 (8px)
# Quarter is vertically centered on clock: offset by (16-8)/2 = 4px
CLOCK_Y = 4
QUARTER_Y = CLOCK_Y + 4  # Vertically centered on clock

# Down/distance (below quarter/clock line)
SITUATION_Y = 22

# Last play summary (bottom of display, centered in free space below scores)
# Scores end at Y=43 (SCORE_Y + 16), display height is 64, free space is 21px
# Using spleen_5x8 (8px tall): Y = 43 + (21 - 8) / 2 ≈ 49
LAST_PLAY_Y = 49

# Football field sprite position (121x11, left-biased center, 1px bottom padding)
FIELD_X = 3                # (128 - 121) // 2 = 3
FIELD_Y = 52               # 64 - 11 - 1 = 52

# Field perspective vanishing point (configurable)
FIELD_VP_X = 63            # Roughly center of 128px display
FIELD_VP_Y = -1            # Just above top edge of screen

# Field line colors
FIELD_YELLOW = rgb565(255, 255, 0)   # First down marker
FIELD_NAVY = rgb565(0, 0, 140)       # Line of scrimmage

# Score flash animation constants
FLASH_DURATION_MS = 3000   # How long to flash after scoring (3 seconds)
FLASH_INTERVAL_MS = 200    # Toggle rate (5 Hz = 200ms per state)

# Clock flash animation constants
CLOCK_FLASH_INTERVAL_MS = 800   # Sub-10s clock toggle rate (1/4 score flash speed)
CLOCK_ZERO_FLASH_DURATION_MS = 5000  # How long to flash after clock hits zero


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
# Rendering helpers
# =============================================================================

def should_flash(scored_ms: int, now_ms: int) -> bool:
    """
    Determine if score should show alternate color based on flash timing.

    Returns True if we're in a flash window AND on an "alternate" cycle.
    Uses time.ticks_diff for proper wraparound handling.
    """
    if scored_ms == 0:
        return False

    elapsed = time.ticks_diff(now_ms, scored_ms)
    if elapsed < 0 or elapsed > FLASH_DURATION_MS:
        return False

    # Alternate every FLASH_INTERVAL_MS
    return (elapsed // FLASH_INTERVAL_MS) % 2 == 1


def dim_team_color(color: Color) -> int:
    """Dim a Color object to 50% brightness and return RGB565."""
    return rgb565(color.r >> 1, color.g >> 1, color.b >> 1)


def draw_timeout_bars(display: Hub75Display, logo_x: int, timeouts_remaining: int, team_color: int, used_color: int) -> None:
    """Draw 3 timeout indicator bars under a team's logo."""
    base_x = logo_x + TIMEOUT_OFFSET_X
    for i in range(TIMEOUT_COUNT):
        x = base_x + i * (TIMEOUT_BAR_W + TIMEOUT_GAP)
        color = team_color if i < timeouts_remaining else used_color
        display.fill_rect(x, TIMEOUT_Y, TIMEOUT_BAR_W, TIMEOUT_BAR_H, color)


def draw_possession_arrow(display: Hub75Display, x: int, y: int, pointing_right: bool, color: int) -> None:
    """Draw a small 3x5 filled triangle as a possession indicator."""
    if pointing_right:
        display.fill_rect(x, y, 1, 5, color)
        display.fill_rect(x + 1, y + 1, 1, 3, color)
        display.pixel(x + 2, y + 2, color)
    else:
        display.fill_rect(x + 2, y, 1, 5, color)
        display.fill_rect(x + 1, y + 1, 1, 3, color)
        display.pixel(x, y + 2, color)


def draw_football_field(display: Hub75Display, field) -> None:
    """Draw football field with endzone colors, perspective lines, and ball."""
    if field.ball_x is None:
        return

    # 1. Update endzone palette colors
    field_sprite.palette.pixel(3, 0, field.away_color)
    field_sprite.palette.pixel(4, 0, field.home_color)

    # 2. Blit field sprite
    display.blit(field_sprite.data, FIELD_X, FIELD_Y, -1, field_sprite.palette)

    # 3. Perspective lines (LOS and first down)
    field_bottom_y = FIELD_Y + field_sprite.HEIGHT - 1  # 62
    field_top_y = FIELD_Y                                # 52
    t = (field_bottom_y - field_top_y) / (field_bottom_y - FIELD_VP_Y)

    # Line of scrimmage (navy, 2px wide)
    los_x = max(14, min(field.ball_x, 113))
    for dx in range(2):
        bx = los_x + dx
        tx = round(bx + t * (FIELD_VP_X - bx))
        display.line(bx, field_bottom_y, tx, field_top_y, FIELD_NAVY)

    # First down marker (yellow, 2px wide)
    if field.first_down_x is not None:
        fd_x = max(14, min(field.first_down_x, 113))
        for dx in range(2):
            bx = fd_x + dx
            tx = round(bx + t * (FIELD_VP_X - bx))
            display.line(bx, field_bottom_y, tx, field_top_y, FIELD_YELLOW)

    # 4. Ball sprite above the field at LOS position (aligned to top perspective)
    los_top_x = round(los_x + t * (FIELD_VP_X - los_x))
    ball_x = los_top_x - ball_sprite.WIDTH // 2
    ball_y = FIELD_Y - ball_sprite.HEIGHT - 2
    display.blit(ball_sprite.data, ball_x, ball_y, 0)

    # 5. Direction arrow next to ball
    direction = field.direction
    if direction == 1:
        # Arrow pointing right
        ax = ball_x + ball_sprite.WIDTH + 1
        ay = ball_y + ball_sprite.HEIGHT // 2
        display.fill_rect(ax, ay - 1, 1, 3, FIELD_NAVY)
        display.pixel(ax + 1, ay, FIELD_NAVY)
    elif direction == -1:
        # Arrow pointing left
        ax = ball_x - 2
        ay = ball_y + ball_sprite.HEIGHT // 2
        display.fill_rect(ax + 1, ay - 1, 1, 3, FIELD_NAVY)
        display.pixel(ax, ay, FIELD_NAVY)


# Pre-allocated logo buffer pool
_LOGO_POOL_SIZE = 8  # Max logos cached
_LOGO_WIDTH = 24
_LOGO_HEIGHT = 24
_LOGO_BUFFER_SIZE = _LOGO_WIDTH * _LOGO_HEIGHT * 2  # 1152 bytes per logo

_logo_buffers = [bytearray(_LOGO_BUFFER_SIZE) for _ in range(_LOGO_POOL_SIZE)]
_logo_cache = {}  # team_abbr -> (slot_index, FrameBuffer)
_logo_lru = []    # LRU order: oldest first
_free_slots = set(range(_LOGO_POOL_SIZE))

print(f"Pre-allocated {_LOGO_POOL_SIZE} logo buffers ({_LOGO_POOL_SIZE * _LOGO_BUFFER_SIZE // 1024} KB)")



def get_logo_framebuffer(api_client: ScoreboardApiClient, team_abbreviation: str) -> framebuf.FrameBuffer | None:
    """
    Get logo framebuffer from cache or fetch from API.

    Uses a pre-allocated buffer pool with LRU eviction to prevent
    memory fragmentation from repeated allocations.
    """
    key = team_abbreviation.lower()

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
        print(f"Evicted logo {evict_key} from slot {slot_index}")

    try:
        status, body = api_client.get_team_logo_raw(
            team_id=key,
            width=_LOGO_WIDTH,
            height=_LOGO_HEIGHT,
            background_color="000000",
            accept="image/x-rgb565"
        )

        if status != 200:
            print(f"Logo fetch failed for {key}: HTTP {status}")
            _free_slots.add(slot_index)
            return None

        buf = _logo_buffers[slot_index]
        buf[:len(body)] = body
        fb = framebuf.FrameBuffer(buf, _LOGO_WIDTH, _LOGO_HEIGHT, framebuf.RGB565)

        _logo_cache[key] = (slot_index, fb)
        _logo_lru.append(key)
        print(f"Cached logo for {key} (slot {slot_index}/{_LOGO_POOL_SIZE})")
        return fb

    except Exception as e:
        print(f"Logo fetch error for {key}: {e}")
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


def safe_team_color(color: Color, fallback_color: int) -> int:
    """
    Convert team color to RGB565, ensuring visibility on black background.
    If color is too dark, use the fallback color instead.
    """
    if color.r < 30 and color.g < 30 and color.b < 30:
        return fallback_color
    return rgb565(color.r, color.g, color.b)


def render_scrolling_or_centered(writer: FontWriter, text: str, y: int, width: int, color: int, font: object, animation_start_ms: int, now_ms: int) -> None:
    """
    Pure function: Render text centered if it fits, or scroll if too wide.

    Computes scroll offset from timestamps without any internal state.
    """
    text_width = writer.measure(text, font)
    if text_width <= width:
        writer.aligned_text(text, 0, y, width, ALIGN_CENTER, color, font=font)
    else:
        elapsed = time.ticks_diff(now_ms, animation_start_ms)
        offset = calculate_scroll_offset(text_width, width, elapsed)
        writer.text(text, -offset, y, color, font=font)


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
        display.blit(qr_fb, qr_x, qr_y, -1, qr_palette)
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


def render_pregame(display: Hub75Display, writer: FontWriter, game: PregameGame, state: StateBuffer, colors: UiColors, home_logo: framebuf.FrameBuffer | None, away_logo: framebuf.FrameBuffer | None, now_ms: int) -> None:
    """Render pregame screen with team matchup on 128x64 display."""
    display.fill(BLACK)

    home_color = safe_team_color(game.home.color, colors.secondary)
    away_color = safe_team_color(game.away.color, colors.secondary)

    if away_logo:
        display.blit(away_logo, AWAY_LOGO_X, 0)
    if home_logo:
        display.blit(home_logo, HOME_LOGO_X, 0)

    # "@" in center between logos
    writer.aligned_text("@", 0, QUARTER_Y, DISPLAY_WIDTH, ALIGN_CENTER, colors.primary, font=unscii_16)

    # Use pre-formatted date/time from state (no allocation)
    display_data = state.display
    date_display = display_data.pregame_date
    time_str = display_data.pregame_time

    writer.aligned_text(date_display, 0, 26, DISPLAY_WIDTH, ALIGN_CENTER, colors.secondary)
    writer.aligned_text(time_str, 0, 36, DISPLAY_WIDTH, ALIGN_CENTER, colors.accent)

    # Venue (single line, scrolls if too long)
    if game.venue:
        animation_start_ms = state.animation_start_ms
        render_scrolling_or_centered(
            writer, game.venue, 46, DISPLAY_WIDTH,
            colors.secondary, unscii_8, animation_start_ms, now_ms
        )


def render_live(display: Hub75Display, writer: FontWriter, game: LiveGame, state: StateBuffer, colors: UiColors, home_logo: framebuf.FrameBuffer | None, away_logo: framebuf.FrameBuffer | None, now_ms: int) -> None:
    """Render live game with logos and scores on 128x64 display."""
    display.fill(BLACK)

    home_color = safe_team_color(game.home.color, colors.secondary)
    away_color = safe_team_color(game.away.color, colors.secondary)

    if away_logo:
        display.blit(away_logo, AWAY_LOGO_X, 0)
    if home_logo:
        display.blit(home_logo, HOME_LOGO_X, 0)

    # Determine score colors with flash effect
    if should_flash(state.away_scored_ms, now_ms):
        away_score_color = colors.accent
    else:
        away_score_color = away_color

    if should_flash(state.home_scored_ms, now_ms):
        home_score_color = colors.accent
    else:
        home_score_color = home_color

    # Timeout bars under logos
    draw_timeout_bars(display, AWAY_LOGO_X, game.away.timeouts, away_color, dim_team_color(game.away.color))
    draw_timeout_bars(display, HOME_LOGO_X, game.home.timeouts, home_color, dim_team_color(game.home.color))

    # Scores centered under logos - ZERO ALLOCATIONS
    writer.integer(game.away.score, AWAY_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, away_score_color, font=unscii_16)
    writer.integer(game.home.score, HOME_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, home_score_color, font=unscii_16)

    # Quarter and situation from pre-formatted state (no allocation)
    display_data = state.display
    quarter_display = display_data.quarter
    situation_str = display_data.situation

    # Quarter (left 1/3) and Clock (right 2/3) on same line
    writer.aligned_text(quarter_display, QUARTER_X, QUARTER_Y, QUARTER_WIDTH, ALIGN_CENTER, colors.primary)

    # Compute display clock from immutable state (no mutation!)
    clock_seconds = state.clock_seconds or 0
    clock_last_tick_ms = state.clock_last_tick_ms
    clock_is_counting = hasattr(game, 'clock_running') and game.clock_running and clock_last_tick_ms
    if clock_is_counting:
        elapsed_ms = time.ticks_diff(now_ms, clock_last_tick_ms)
        display_seconds = max(0, clock_seconds - elapsed_ms // 1000)
    else:
        display_seconds = clock_seconds

    # Clock color: normal >= 60s, warning < 60s, flashing < 10s, fast flash at zero
    if display_seconds == 0 and clock_is_counting and clock_seconds > 0:
        zero_ms = clock_last_tick_ms + clock_seconds * 1000
        since_zero = time.ticks_diff(now_ms, zero_ms)
        if 0 <= since_zero < CLOCK_ZERO_FLASH_DURATION_MS:
            if (now_ms // FLASH_INTERVAL_MS) % 2 == 1:
                clock_color = colors.accent
            else:
                clock_color = colors.clock_warning
        else:
            clock_color = colors.clock_warning
    elif display_seconds < 10:
        if (now_ms // CLOCK_FLASH_INTERVAL_MS) % 2 == 1:
            clock_color = colors.accent
        else:
            clock_color = colors.clock_warning
    elif display_seconds < 60:
        clock_color = colors.clock_warning
    else:
        clock_color = colors.clock_normal
    writer.clock(display_seconds, CLOCK_X, CLOCK_Y, CLOCK_WIDTH, ALIGN_CENTER, clock_color)

    # Down & distance below quarter/clock line, with possession arrow
    if situation_str:
        writer.aligned_text(situation_str, 0, SITUATION_Y, DISPLAY_WIDTH, ALIGN_CENTER, colors.primary)

        possession = display_data.possession
        if possession:
            text_width = writer.measure(situation_str)
            text_x = (DISPLAY_WIDTH - text_width) // 2
            arrow_y = SITUATION_Y + 1
            if possession == 'away':
                draw_possession_arrow(display, text_x - 5, arrow_y, False, away_color)
            elif possession == 'home':
                draw_possession_arrow(display, text_x + text_width + 2, arrow_y, True, home_color)

    # Football field visualization at bottom of display
    draw_football_field(display, state.field)


def render_final(display: Hub75Display, writer: FontWriter, game: FinalGame, state: StateBuffer, colors: UiColors, home_logo: framebuf.FrameBuffer | None, away_logo: framebuf.FrameBuffer | None, now_ms: int) -> None:
    """Render final score with logos on 128x64 display."""
    display.fill(BLACK)

    home_color = safe_team_color(game.home.color, colors.secondary)
    away_color = safe_team_color(game.away.color, colors.secondary)

    if away_logo:
        display.blit(away_logo, AWAY_LOGO_X, 0)
    if home_logo:
        display.blit(home_logo, HOME_LOGO_X, 0)

    # Determine score colors with flash effect (continues from live game)
    if should_flash(state.away_scored_ms, now_ms):
        away_score_color = colors.accent
    else:
        away_score_color = away_color

    if should_flash(state.home_scored_ms, now_ms):
        home_score_color = colors.accent
    else:
        home_score_color = home_color

    # Timeout bars under logos
    draw_timeout_bars(display, AWAY_LOGO_X, game.away.timeouts, away_color, dim_team_color(game.away.color))
    draw_timeout_bars(display, HOME_LOGO_X, game.home.timeouts, home_color, dim_team_color(game.home.color))

    # Scores centered under logos - ZERO ALLOCATIONS
    writer.integer(game.away.score, AWAY_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, away_score_color, font=unscii_16)
    writer.integer(game.home.score, HOME_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, home_score_color, font=unscii_16)

    # Final status centered
    status_text = "FINAL"
    if game.status == "final/OT":
        status_text = "F/OT"
    writer.aligned_text(status_text, 0, CLOCK_Y, DISPLAY_WIDTH, ALIGN_CENTER, colors.primary, font=unscii_16)


def redraw_clock_only(display: Hub75Display, writer: FontWriter, seconds: int, colors: UiColors) -> None:
    """Redraw only the clock region (partial update) with zero allocations."""
    clock_height = 16

    display.fill_rect(CLOCK_X, CLOCK_Y, CLOCK_WIDTH, clock_height, BLACK)

    if seconds < 60:
        clock_color = colors.clock_warning
    else:
        clock_color = colors.clock_normal

    writer.clock(seconds, CLOCK_X, CLOCK_Y, CLOCK_WIDTH, ALIGN_CENTER, clock_color)


def render_frame(display: Hub75Display, writer: FontWriter, state: StateBuffer, colors: UiColors, now_ms: int) -> None:
    """
    Render a frame based on current display state.

    Pure function: all timing-dependent computations use the passed now_ms
    timestamp rather than querying time internally.
    """
    mode = state.mode

    home_logo = state.home_logo
    away_logo = state.away_logo

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
    elif mode == 'game':
        game = state.game
        if game is None:
            render_idle(display, writer, colors)
        elif isinstance(game, PregameGame):
            render_pregame(display, writer, game, state, colors,
                          home_logo, away_logo, now_ms)
        elif isinstance(game, LiveGame):
            render_live(display, writer, game, state, colors,
                       home_logo, away_logo, now_ms)
        elif isinstance(game, FinalGame):
            render_final(display, writer, game, state, colors,
                        home_logo, away_logo, now_ms)
        else:
            render_idle(display, writer, colors)
    else:
        render_idle(display, writer, colors)


# =============================================================================
# Display thread (runs on Core 1)
# =============================================================================

def run_display_thread(display: Hub75Display, writer: FontWriter) -> None:
    """
    Main entry point for Core 1 display thread.

    Runs a constant 20 FPS loop reading state from the front buffer
    and rendering to the display. The fixed frame rate ensures smooth
    animations (scrolling text, score flashing, clock updates).

    IMPORTANT: This function runs on Core 1 with ZERO memory allocations.
    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    UI colors are pre-computed on Core 0 and read from state.ui_colors.
    """
    from scoreboard.state import get_display_state

    print("Display thread starting on Core 1 (20 FPS)...")

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
            print(f"Display thread error: {e}")

        # Constant 20 FPS for all animations
        time.sleep_ms(50)

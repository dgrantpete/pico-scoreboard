"""
Display rendering functions for the Pico Scoreboard.

Provides render functions for different display modes (startup, idle, game, error)
and the logo caching system. The actual display loop runs in display_thread.py
on Core 1.
"""

import time
import framebuf
from machine import Pin
from hub75 import Hub75Driver, Hub75Display
from lib.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565, ALIGN_LEFT, ALIGN_CENTER, ALIGN_RIGHT
from lib.scoreboard.models import STATE_PREGAME, STATE_LIVE, STATE_FINAL
from lib.animations import calculate_scroll_offset

# Fixed color
BLACK = 0

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

# Score flash animation constants
FLASH_DURATION_MS = 3000   # How long to flash after scoring (3 seconds)
FLASH_INTERVAL_MS = 200    # Toggle rate (5 Hz = 200ms per state)

# Clock flash animation constants
CLOCK_FLASH_INTERVAL_MS = 800   # Sub-10s clock toggle rate (1/4 score flash speed)
CLOCK_ZERO_FLASH_DURATION_MS = 5000  # How long to flash after clock hits zero


def should_flash(scored_ms: int, now_ms: int) -> bool:
    """
    Determine if score should show alternate color based on flash timing.

    Returns True if we're in a flash window AND on an "alternate" cycle.
    Uses time.ticks_diff for proper wraparound handling.

    Args:
        scored_ms: Timestamp when score changed (0 if no recent score)
        now_ms: Current timestamp (passed from render loop)
    """
    if scored_ms == 0:
        return False

    elapsed = time.ticks_diff(now_ms, scored_ms)
    if elapsed < 0 or elapsed > FLASH_DURATION_MS:
        return False

    # Alternate every FLASH_INTERVAL_MS
    # Returns True for odd intervals (shows accent color)
    return (elapsed // FLASH_INTERVAL_MS) % 2 == 1


def dim_team_color(color):
    """Dim a Color object to 50% brightness and return RGB565."""
    return rgb565(color.r >> 1, color.g >> 1, color.b >> 1)


def draw_timeout_bars(display, logo_x, timeouts_remaining, team_color, used_color):
    """
    Draw 3 timeout indicator bars under a team's logo.

    Args:
        display: Hub75Display instance
        logo_x: X position of the team's logo (AWAY_LOGO_X or HOME_LOGO_X)
        timeouts_remaining: Number of remaining timeouts (0-3)
        team_color: RGB565 color for remaining timeouts
        used_color: RGB565 color for used timeouts
    """
    base_x = logo_x + TIMEOUT_OFFSET_X
    for i in range(TIMEOUT_COUNT):
        x = base_x + i * (TIMEOUT_BAR_W + TIMEOUT_GAP)
        color = team_color if i < timeouts_remaining else used_color
        display.fill_rect(x, TIMEOUT_Y, TIMEOUT_BAR_W, TIMEOUT_BAR_H, color)


def draw_possession_arrow(display, x, y, pointing_right, color):
    """
    Draw a small 3x5 filled triangle as a possession indicator.

    Args:
        display: Hub75Display instance
        x: X position (left edge of bounding box)
        y: Y position (top edge of bounding box)
        pointing_right: True for ► (home), False for ◄ (away)
        color: RGB565 color
    """
    if pointing_right:
        # ►: col 0 = full height, col 1 = middle 3, col 2 = center
        display.fill_rect(x, y, 1, 5, color)
        display.fill_rect(x + 1, y + 1, 1, 3, color)
        display.pixel(x + 2, y + 2, color)
    else:
        # ◄: col 2 = full height, col 1 = middle 3, col 0 = center
        display.fill_rect(x + 2, y, 1, 5, color)
        display.fill_rect(x + 1, y + 1, 1, 3, color)
        display.pixel(x, y + 2, color)


# Pre-allocated logo buffer pool
# Allocating upfront prevents memory fragmentation during runtime
_LOGO_POOL_SIZE = 8  # Max logos cached
_LOGO_WIDTH = 24
_LOGO_HEIGHT = 24
_LOGO_BUFFER_SIZE = _LOGO_WIDTH * _LOGO_HEIGHT * 2  # 1152 bytes per logo

# Pre-allocate all buffers at module load time (before fragmentation occurs)
_logo_buffers = [bytearray(_LOGO_BUFFER_SIZE) for _ in range(_LOGO_POOL_SIZE)]
_logo_cache = {}  # team_abbr -> (slot_index, FrameBuffer)
_logo_lru = []    # LRU order: oldest first
_free_slots = set(range(_LOGO_POOL_SIZE))  # Track available buffer slots

print(f"Pre-allocated {_LOGO_POOL_SIZE} logo buffers ({_LOGO_POOL_SIZE * _LOGO_BUFFER_SIZE // 1024} KB)")



def get_logo_framebuffer(api_client, team_abbreviation):
    """
    Get logo framebuffer from cache or fetch from API.

    Uses a pre-allocated buffer pool with LRU eviction to prevent
    memory fragmentation from repeated allocations.

    Args:
        api_client: ScoreboardApiClient instance
        team_abbreviation: Team abbreviation (e.g., "DAL")

    Returns:
        framebuf.FrameBuffer or None on error
    """
    key = team_abbreviation.lower()

    # Return cached if available
    if key in _logo_cache:
        # Move to end of LRU (most recently used)
        _logo_lru.remove(key)
        _logo_lru.append(key)
        return _logo_cache[key][1]  # Return FrameBuffer

    # Need to fetch - get a buffer slot
    if _free_slots:
        # Use a free slot
        slot_index = _free_slots.pop()
    else:
        # Evict oldest entry to reuse its slot
        evict_key = _logo_lru.pop(0)
        slot_index = _logo_cache[evict_key][0]
        del _logo_cache[evict_key]
        print(f"Evicted logo {evict_key} from slot {slot_index}")

    # Fetch from API
    try:
        status, body = api_client.get_team_logo_raw(
            team_id=key,
            width=_LOGO_WIDTH,
            height=_LOGO_HEIGHT,
            accept="image/x-rgb565"
        )

        if status != 200:
            print(f"Logo fetch failed for {key}: HTTP {status}")
            _free_slots.add(slot_index)  # Return slot to free pool
            return None

        # Copy RGB565 data directly into pre-allocated buffer
        buf = _logo_buffers[slot_index]
        buf[:len(body)] = body
        fb = framebuf.FrameBuffer(buf, _LOGO_WIDTH, _LOGO_HEIGHT, framebuf.RGB565)

        _logo_cache[key] = (slot_index, fb)
        _logo_lru.append(key)
        print(f"Cached logo for {key} (slot {slot_index}/{_LOGO_POOL_SIZE})")
        return fb

    except Exception as e:
        print(f"Logo fetch error for {key}: {e}")
        _free_slots.add(slot_index)  # Return slot to free pool
        return None


def get_ui_colors(config):
    """
    Get UI colors from config, converted to RGB565.

    Returns dict with color names mapped to RGB565 values.
    """
    def to_rgb565(color_dict):
        return rgb565(color_dict["r"], color_dict["g"], color_dict["b"])

    return {
        'primary': to_rgb565(config.get_color('primary')),
        'secondary': to_rgb565(config.get_color('secondary')),
        'accent': to_rgb565(config.get_color('accent')),
        'clock_normal': to_rgb565(config.get_color('clock_normal')),
        'clock_warning': to_rgb565(config.get_color('clock_warning')),
    }


def init_display(config=None):
    """
    Initialize and return HUB75 display hardware.

    Args:
        config: Optional Config instance for display settings.
                Uses driver defaults if not provided.

    Returns:
        Tuple of (driver, display, writer)
    """
    if config is not None:
        data_freq = config.data_frequency_hz
        brightness = config.brightness / 100.0
        gamma = config.gamma
        blanking_time = config.blanking_time_ns
        target_refresh_rate = config.target_refresh_rate
    else:
        from hub75.driver import DEFAULT_DATA_FREQUENCY
        data_freq = DEFAULT_DATA_FREQUENCY
        brightness = 1.0
        gamma = 2.2
        blanking_time = 0
        target_refresh_rate = 120.0

    driver = Hub75Driver(
        address_bit_count=5,
        shift_register_depth=128,
        base_address_pin=Pin(8, Pin.OUT),
        output_enable_pin=Pin(7, Pin.OUT),
        base_clock_pin=Pin(5, Pin.OUT),
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


def color_to_rgb565(color):
    """Convert a Color object to RGB565."""
    return rgb565(color.r, color.g, color.b)


def safe_team_color(color, fallback_color):
    """
    Convert team color to RGB565, ensuring visibility on black background.
    If color is too dark, use the fallback color instead.
    """
    if color.r < 30 and color.g < 30 and color.b < 30:
        return fallback_color
    return rgb565(color.r, color.g, color.b)


def render_scrolling_or_centered(writer, text, y, width, color, font, animation_start_ms, now_ms):
    """
    Pure function: Render text centered if it fits, or scroll if too wide.

    Computes scroll offset from timestamps without any internal state.

    Args:
        writer: FontWriter instance
        text: Text to render
        y: Y position
        width: Available width in pixels
        color: Text color (RGB565)
        font: Font to use
        animation_start_ms: Timestamp when animation started
        now_ms: Current timestamp
    """
    text_width = writer.measure(text, font)
    if text_width <= width:
        writer.aligned_text(text, 0, y, width, ALIGN_CENTER, color, font=font)
    else:
        elapsed = time.ticks_diff(now_ms, animation_start_ms)
        offset = calculate_scroll_offset(text_width, width, elapsed)
        writer.text(text, -offset, y, color, font=font)


def wrap_text(text, max_chars):
    """
    Wrap text to fit within max_chars per line.

    Tries to break at word boundaries when possible.
    Returns list of lines.
    """
    if len(text) <= max_chars:
        return [text]

    lines = []
    remaining = text

    while remaining:
        if len(remaining) <= max_chars:
            lines.append(remaining)
            break

        # Find last space within limit
        break_point = remaining[:max_chars].rfind(' ')
        if break_point <= 0:
            # No space found, hard break at limit
            break_point = max_chars - 1
            lines.append(remaining[:break_point] + '-')
            remaining = remaining[break_point:]
        else:
            lines.append(remaining[:break_point])
            remaining = remaining[break_point + 1:]  # Skip the space

    return lines


def draw_progress_bar(display, x, y, width, height, progress, colors):
    """
    Draw a horizontal progress bar.

    Args:
        display: Hub75Display instance
        x, y: Top-left position
        width, height: Dimensions
        progress: 0-100 percentage
        colors: Color dict with 'secondary' (border) and 'accent' (fill)
    """
    # Border
    display.rect(x, y, width, height, colors['secondary'])
    # Fill (leave 1px border)
    fill_width = int((width - 2) * progress / 100)
    if fill_width > 0:
        display.fill_rect(x + 1, y + 1, fill_width, height - 2, colors['accent'])


def render_startup(display, writer, state, colors):
    """Render startup/boot progress screen."""
    display.fill(BLACK)

    startup = state.get('startup', {})
    step = startup.get('step', 1)
    total = startup.get('total_steps', 5)
    operation = startup.get('operation', 'Starting...')
    detail = startup.get('detail', '')

    # Title "BOOTING" at top
    writer.aligned_text("BOOTING", 0, 4, DISPLAY_WIDTH, ALIGN_CENTER, colors['accent'], font=unscii_16)

    # Progress bar (80px wide, centered) at Y=24
    bar_width = 80
    bar_x = (DISPLAY_WIDTH - bar_width) // 2
    progress = int((step - 1) / total * 100) + (100 // total) // 2  # Center of current step
    draw_progress_bar(display, bar_x, 24, bar_width, 8, progress, colors)

    # Step indicator to the right of progress bar
    step_text = f"{step}/{total}"
    writer.text(step_text, bar_x + bar_width + 4, 24, colors['secondary'], font=spleen_5x8)

    # Operation text (truncate to 25 chars)
    if len(operation) > 25:
        operation = operation[:24] + '.'
    writer.aligned_text(operation, 0, 42, DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'], font=spleen_5x8)

    # Detail text (truncate to 25 chars)
    if detail:
        if len(detail) > 25:
            detail = detail[:24] + '.'
        writer.aligned_text(detail, 0, 54, DISPLAY_WIDTH, ALIGN_CENTER, colors['secondary'], font=spleen_5x8)


def render_idle(display, writer, colors):
    """Render idle/waiting screen."""
    display.fill(BLACK)
    writer.aligned_text("PICO", 0, 16, DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'], font=unscii_16)
    writer.aligned_text("SCOREBOARD", 0, 40, DISPLAY_WIDTH, ALIGN_CENTER, colors['accent'])


def render_no_games(display, writer, colors):
    """Render no games scheduled screen."""
    display.fill(BLACK)
    writer.aligned_text("NO GAMES", 0, 20, DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'], font=unscii_16)
    writer.aligned_text("scheduled", 0, 40, DISPLAY_WIDTH, ALIGN_CENTER, colors['secondary'], font=spleen_5x8)


def render_setup(display, writer, state, colors, now_ms):
    """
    Render setup mode screen with WiFi QR code and contextual information.

    All setup screens show a WiFi QR code so users can (re)join the AP network.
    QR is a convenience - text instructions are always provided as the primary path.
    Shows different messaging based on the reason for entering setup mode:
    - no_config: First-time setup instructions
    - connection_failed: WiFi connection failure with SSID
    - bad_auth: Wrong password error with SSID

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        state: Display state dict
        colors: UI colors dict
        now_ms: Current timestamp for animations
    """
    display.fill(BLACK)

    setup = state.get('setup', {})
    reason = setup.get('reason', 'no_config')
    ap_ssid = setup.get('ap_ssid', 'scoreboard')
    ap_ip = setup.get('ap_ip', '192.168.4.1')
    wifi_ssid = setup.get('wifi_ssid', '')
    animation_start_ms = state.get('animation_start_ms', 0)

    # Get QR code from state (generated on Core 0)
    qr_fb = setup.get('qr_fb')
    qr_width = setup.get('qr_width', 0)
    qr_height = setup.get('qr_height', 0)
    qr_palette = setup.get('qr_palette')

    # Render QR on right side if available
    text_area_width = DISPLAY_WIDTH
    if qr_fb is not None and qr_palette is not None and qr_width > 0:
        qr_x = DISPLAY_WIDTH - qr_width - 2  # 2px padding from right
        qr_y = 2  # 2px from top
        display.blit(qr_fb, qr_x, qr_y, -1, qr_palette)
        text_area_width = qr_x - 4  # Leave gap before QR

    # Calculate where QR code ends vertically
    qr_bottom = qr_y + qr_height if qr_height > 0 else 0

    # Helper for scrolling text - pure function using timestamps
    def render_scrolling_text(text, y, color, width=None):
        # Use full display width for lines below QR code, otherwise text_area_width
        if width is None:
            width = DISPLAY_WIDTH if y >= qr_bottom else text_area_width
        # Use actual measured width instead of approximating
        pixel_width = writer.measure(text, spleen_5x8)
        if pixel_width > width and width > 0:
            elapsed = time.ticks_diff(now_ms, animation_start_ms)
            offset = calculate_scroll_offset(pixel_width, width, elapsed)
            writer.text(text, 2 - offset, y, color, font=spleen_5x8)
        else:
            writer.text(text, 2, y, color, font=spleen_5x8)

    if reason == 'bad_auth':
        # Wrong password - show error, explain how to fix
        writer.text("WRONG PASS", 2, 0, colors['clock_warning'], font=unscii_16)
        # y=18 is next to QR, y=28+ is below QR
        render_scrolling_text(f'for "{wifi_ssid}"', 18, colors['primary'])
        render_scrolling_text(f'Scan/join "{ap_ssid}"', 28, colors['secondary'])
        writer.text(f"Then go to {ap_ip}", 2, 44, colors['secondary'], font=spleen_5x8)
        writer.text("to fix password", 2, 54, colors['accent'], font=spleen_5x8)

    elif reason == 'connection_failed':
        # Connection failed - show error, explain how to reconfigure
        writer.text("WIFI FAIL", 2, 0, colors['clock_warning'], font=unscii_16)
        # y=18 is next to QR, y=28+ is below QR
        render_scrolling_text(f'"{wifi_ssid}"', 18, colors['primary'])
        render_scrolling_text(f'Scan/join "{ap_ssid}"', 28, colors['secondary'])
        writer.text(f"Then go to {ap_ip}", 2, 44, colors['secondary'], font=spleen_5x8)
        writer.text("to reconfigure", 2, 54, colors['accent'], font=spleen_5x8)

    else:
        # no_config - first-time setup with clear step-by-step instructions
        writer.text("SETUP", 2, 0, colors['accent'], font=unscii_16)
        writer.text("Scan QR or join", 2, 18, colors['primary'], font=spleen_5x8)
        # y=28 is below QR, use full width
        render_scrolling_text(f'"{ap_ssid}" WiFi', 28, colors['secondary'])
        writer.text("Then go to", 2, 44, colors['secondary'], font=spleen_5x8)
        writer.text(ap_ip, 2, 54, colors['accent'], font=spleen_5x8)


def render_error(display, writer, state, colors):
    """
    Render error screen with multi-line details.

    Uses state['error']['title'] for the header and
    state['error']['lines'] for up to 4 detail lines.
    Falls back to state['error_message'] for legacy compatibility.
    """
    display.fill(BLACK)

    error = state.get('error', {})
    title = error.get('title', '')
    lines = error.get('lines', [])

    # Fall back to legacy error_message if new format not used
    if not title and not lines:
        legacy_message = state.get('error_message', '')
        title = 'ERROR'
        if legacy_message:
            lines = [legacy_message[:25]]

    # Title in warning color at top
    writer.aligned_text(title or 'ERROR', 0, 0, DISPLAY_WIDTH, ALIGN_CENTER, colors['clock_warning'], font=unscii_16)

    # Detail lines (up to 4, using spleen_5x8)
    y_start = 24
    line_height = 10
    for i, line in enumerate(lines[:4]):
        # Truncate line to fit (25 chars for spleen_5x8)
        display_line = line[:25] if len(line) > 25 else line
        writer.aligned_text(display_line, 0, y_start + (i * line_height), DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'], font=spleen_5x8)


def render_pregame(display, writer, game, state, colors, home_logo, away_logo, now_ms):
    """
    Render pregame screen with team matchup on 128x64 display.

    Uses pre-formatted date/time from state['display'] to avoid allocations.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        game: PregameGame object
        state: Display state dict (contains pre-formatted display strings)
        colors: UI colors dict
        home_logo: Home team logo FrameBuffer (from state)
        away_logo: Away team logo FrameBuffer (from state)
        now_ms: Current timestamp for animations
    """
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if away_logo:
        display.blit(away_logo, AWAY_LOGO_X, 0)
    if home_logo:
        display.blit(home_logo, HOME_LOGO_X, 0)

    # "@" in center between logos
    writer.aligned_text("@", 0, QUARTER_Y, DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'], font=unscii_16)

    # Use pre-formatted date/time from state (no allocation)
    display_data = state['display']
    date_display = display_data['pregame_date']
    time_str = display_data['pregame_time']

    # Date and time centered (vertically separated)
    writer.aligned_text(date_display, 0, 26, DISPLAY_WIDTH, ALIGN_CENTER, colors['secondary'])
    writer.aligned_text(time_str, 0, 36, DISPLAY_WIDTH, ALIGN_CENTER, colors['accent'])

    # Venue (single line, scrolls if too long)
    if game.venue:
        animation_start_ms = state.get('animation_start_ms', 0)
        render_scrolling_or_centered(
            writer, game.venue, 46, DISPLAY_WIDTH,
            colors['secondary'], unscii_8, animation_start_ms, now_ms
        )


def render_live(display, writer, game, state, colors, home_logo, away_logo, now_ms):
    """
    Render live game with logos and scores on 128x64 display.

    Pure function: computes clock display from state timestamps, no mutations.
    Uses zero-allocation integer rendering for scores and pre-formatted
    strings from state['display'] for quarter/situation.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        game: LiveGame object
        state: Display state dict (contains pre-formatted display strings)
        colors: UI colors dict
        home_logo: Home team logo FrameBuffer (from state)
        away_logo: Away team logo FrameBuffer (from state)
        now_ms: Current timestamp for animations and clock computation
    """
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if away_logo:
        display.blit(away_logo, AWAY_LOGO_X, 0)
    if home_logo:
        display.blit(home_logo, HOME_LOGO_X, 0)

    # Determine score colors with flash effect
    if should_flash(state.get('away_scored_ms', 0), now_ms):
        away_score_color = colors['accent']
    else:
        away_score_color = away_color

    if should_flash(state.get('home_scored_ms', 0), now_ms):
        home_score_color = colors['accent']
    else:
        home_score_color = home_color

    # Timeout bars under logos (used timeouts dimmed to 50%)
    draw_timeout_bars(display, AWAY_LOGO_X, game.away.timeouts, away_color, dim_team_color(game.away.color))
    draw_timeout_bars(display, HOME_LOGO_X, game.home.timeouts, home_color, dim_team_color(game.home.color))

    # Scores centered under logos - ZERO ALLOCATIONS
    writer.integer(game.away.score, AWAY_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, away_score_color, font=unscii_16)
    writer.integer(game.home.score, HOME_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, home_score_color, font=unscii_16)

    # Quarter and situation from pre-formatted state (no allocation)
    display_data = state['display']
    quarter_display = display_data['quarter']
    situation_str = display_data['situation']

    # Quarter (left 1/3) and Clock (right 2/3) on same line, both centered in their areas
    # Quarter is vertically centered on the taller clock font
    writer.aligned_text(quarter_display, QUARTER_X, QUARTER_Y, QUARTER_WIDTH, ALIGN_CENTER, colors['primary'])

    # Compute display clock from immutable state (no mutation!)
    clock_seconds = state.get('clock_seconds') or 0
    clock_last_tick_ms = state.get('clock_last_tick_ms', 0)
    clock_is_counting = hasattr(game, 'clock_running') and game.clock_running and clock_last_tick_ms
    if clock_is_counting:
        # Subtract elapsed time since last API sync
        elapsed_ms = time.ticks_diff(now_ms, clock_last_tick_ms)
        display_seconds = max(0, clock_seconds - elapsed_ms // 1000)
    else:
        # Clock is stopped - show exact API value
        display_seconds = clock_seconds

    # Clock color: normal >= 60s, warning < 60s, flashing < 10s, fast flash at zero
    if display_seconds == 0 and clock_is_counting and clock_seconds > 0:
        # Clock counted down to zero — fast flash for 5 seconds
        # Compute when the clock crossed zero (stateless, from existing timestamps)
        zero_ms = clock_last_tick_ms + clock_seconds * 1000
        since_zero = time.ticks_diff(now_ms, zero_ms)
        if 0 <= since_zero < CLOCK_ZERO_FLASH_DURATION_MS:
            if (now_ms // FLASH_INTERVAL_MS) % 2 == 1:
                clock_color = colors['accent']
            else:
                clock_color = colors['clock_warning']
        else:
            clock_color = colors['clock_warning']
    elif display_seconds < 10:
        # Sub-10s (includes 0 when not counting down): slow flash (800ms)
        if (now_ms // CLOCK_FLASH_INTERVAL_MS) % 2 == 1:
            clock_color = colors['accent']
        else:
            clock_color = colors['clock_warning']
    elif display_seconds < 60:
        clock_color = colors['clock_warning']
    else:
        clock_color = colors['clock_normal']
    writer.clock(display_seconds, CLOCK_X, CLOCK_Y, CLOCK_WIDTH, ALIGN_CENTER, clock_color)

    # Down & distance below quarter/clock line, with possession arrow
    if situation_str:
        writer.aligned_text(situation_str, 0, SITUATION_Y, DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'])

        # Possession triangle arrow next to situation text
        possession = display_data.get('possession', '')
        if possession:
            text_width = writer.measure(situation_str)
            text_x = (DISPLAY_WIDTH - text_width) // 2
            arrow_y = SITUATION_Y + 1  # Vertically centered (5px arrow in 8px text)
            if possession == 'away':
                draw_possession_arrow(display, text_x - 5, arrow_y, False, away_color)
            elif possession == 'home':
                draw_possession_arrow(display, text_x + text_width + 2, arrow_y, True, home_color)

    # Last play summary at bottom of display
    last_play_text = display_data.get('last_play_text', '')
    if last_play_text:
        animation_start_ms = state.get('animation_start_ms', 0)
        render_scrolling_or_centered(
            writer, last_play_text, LAST_PLAY_Y, DISPLAY_WIDTH,
            colors['secondary'], unscii_8, animation_start_ms, now_ms
        )


def render_final(display, writer, game, state, colors, home_logo, away_logo, now_ms):
    """
    Render final score with logos on 128x64 display.

    Uses zero-allocation integer rendering for scores.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        game: FinalGame object
        state: Display state dict
        colors: UI colors dict
        home_logo: Home team logo FrameBuffer (from state)
        away_logo: Away team logo FrameBuffer (from state)
        now_ms: Current timestamp for flash animation
    """
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if away_logo:
        display.blit(away_logo, AWAY_LOGO_X, 0)
    if home_logo:
        display.blit(home_logo, HOME_LOGO_X, 0)

    # Determine score colors with flash effect (continues from live game)
    if should_flash(state.get('away_scored_ms', 0), now_ms):
        away_score_color = colors['accent']
    else:
        away_score_color = away_color

    if should_flash(state.get('home_scored_ms', 0), now_ms):
        home_score_color = colors['accent']
    else:
        home_score_color = home_color

    # Timeout bars under logos (used timeouts dimmed to 50%)
    draw_timeout_bars(display, AWAY_LOGO_X, game.away.timeouts, away_color, dim_team_color(game.away.color))
    draw_timeout_bars(display, HOME_LOGO_X, game.home.timeouts, home_color, dim_team_color(game.home.color))

    # Scores centered under logos - ZERO ALLOCATIONS
    writer.integer(game.away.score, AWAY_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, away_score_color, font=unscii_16)
    writer.integer(game.home.score, HOME_LOGO_X, SCORE_Y, LOGO_SIZE, ALIGN_CENTER, home_score_color, font=unscii_16)

    # Final status centered
    status_text = "FINAL"
    if game.status == "final/OT":
        status_text = "F/OT"
    writer.aligned_text(status_text, 0, CLOCK_Y, DISPLAY_WIDTH, ALIGN_CENTER, colors['primary'], font=unscii_16)


def redraw_clock_only(display, writer, seconds, colors):
    """
    Redraw only the clock region (partial update) with zero allocations.

    Clears the clock strip and redraws the clock using the zero-allocation
    writer.clock() method. Does not touch other parts of the display.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance (must have init_clock() called first)
        seconds: Clock time in total seconds (e.g., 225 for "3:45")
        colors: Color dict with clock_normal and clock_warning
    """
    # Clock is centered in its area (right 2/3 of center) at CLOCK_Y, using unscii_16 (16px tall)
    clock_height = 16

    # Clear the clock area (preserves quarter on left)
    display.fill_rect(CLOCK_X, CLOCK_Y, CLOCK_WIDTH, clock_height, BLACK)

    # Determine clock color based on time remaining (integer comparison)
    if seconds < 60:  # Under 1 minute
        clock_color = colors['clock_warning']
    else:
        clock_color = colors['clock_normal']

    # Use zero-allocation clock method (font already set via init_clock)
    writer.clock(seconds, CLOCK_X, CLOCK_Y, CLOCK_WIDTH, ALIGN_CENTER, clock_color)


def render_frame(display, writer, state, colors, now_ms):
    """
    Render a frame based on current display state.

    Pure function: all timing-dependent computations use the passed now_ms
    timestamp rather than querying time internally.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        state: Display state dict (contains mode, game, logos, etc.)
        colors: UI colors dict
        now_ms: Current timestamp for all timing-dependent rendering
    """
    mode = state.get('mode', 'idle')

    # Get logos from state (fetched by networking thread)
    home_logo = state.get('home_logo')
    away_logo = state.get('away_logo')

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
        game = state.get('game')
        if game is None:
            render_idle(display, writer, colors)
        elif game.state == STATE_PREGAME:
            render_pregame(display, writer, game, state, colors,
                          home_logo, away_logo, now_ms)
        elif game.state == STATE_LIVE:
            render_live(display, writer, game, state, colors,
                       home_logo, away_logo, now_ms)
        elif game.state == STATE_FINAL:
            render_final(display, writer, game, state, colors,
                        home_logo, away_logo, now_ms)
        else:
            render_idle(display, writer, colors)
    else:
        render_idle(display, writer, colors)



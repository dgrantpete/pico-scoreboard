"""
Display rendering functions for the Pico Scoreboard.

Provides render functions for different display modes (startup, idle, game, error)
and the logo caching system. The actual display loop runs in display_thread.py
on Core 1.
"""

import time
from machine import Pin
from hub75 import Hub75Driver, Hub75Display
from lib.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565
from lib.scoreboard.models import STATE_PREGAME, STATE_LIVE, STATE_FINAL
from lib.hub75.image import rgb888_to_rgb565_into_buffer
from lib.animations import ScrollingText

# Fixed color
BLACK = 0

# Display dimensions
DISPLAY_WIDTH = 128
DISPLAY_HEIGHT = 64

# Pre-allocated logo buffer pool
# Allocating upfront prevents memory fragmentation during runtime
_LOGO_POOL_SIZE = 32  # Max logos cached
_LOGO_WIDTH = 24
_LOGO_HEIGHT = 24
_LOGO_BUFFER_SIZE = _LOGO_WIDTH * _LOGO_HEIGHT * 2  # 1152 bytes per logo

# Pre-allocate all buffers at module load time (before fragmentation occurs)
_logo_buffers = [bytearray(_LOGO_BUFFER_SIZE) for _ in range(_LOGO_POOL_SIZE)]
_logo_cache = {}  # team_abbr -> (slot_index, FrameBuffer)
_logo_lru = []    # LRU order: oldest first
_free_slots = set(range(_LOGO_POOL_SIZE))  # Track available buffer slots

print(f"Pre-allocated {_LOGO_POOL_SIZE} logo buffers ({_LOGO_POOL_SIZE * _LOGO_BUFFER_SIZE // 1024} KB)")

# Scrolling text instances (keyed by identifier)
_scrollers = {}


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
            accept="image/x-rgb888"
        )

        if status != 200:
            print(f"Logo fetch failed for {key}: HTTP {status}")
            _free_slots.add(slot_index)  # Return slot to free pool
            return None

        # Convert RGB888 to RGB565 with gamma correction into pre-allocated buffer
        fb = rgb888_to_rgb565_into_buffer(body, _logo_buffers[slot_index], _LOGO_WIDTH, _LOGO_HEIGHT)

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


def get_or_create_scroller(key: str, text: str, text_width: int, display_width: int) -> ScrollingText:
    """
    Get existing scroller or create new one if text changed.

    Args:
        key: Unique identifier for this scroller instance
        text: The text to scroll
        text_width: Pre-measured width of text in pixels
        display_width: Width of display area in pixels

    Returns:
        ScrollingText instance (existing or newly created)
    """
    existing = _scrollers.get(key)
    if existing and existing.text == text:
        return existing

    scroller = ScrollingText(text, text_width, display_width)
    _scrollers[key] = scroller
    return scroller


def init_display(config=None):
    """
    Initialize and return HUB75 display hardware.

    Args:
        config: Optional Config instance for frequency settings.
                Uses driver defaults if not provided.

    Returns:
        Tuple of (driver, display, writer)
    """
    # Determine frequencies from config or use driver defaults
    if config is not None:
        data_freq = config.data_frequency_hz
        addr_freq = config.address_frequency_hz
    else:
        from hub75 import DEFAULT_DATA_FREQUENCY, DEFAULT_ADDRESS_FREQUENCY_DIVIDER
        data_freq = DEFAULT_DATA_FREQUENCY
        addr_freq = data_freq // DEFAULT_ADDRESS_FREQUENCY_DIVIDER

    driver = Hub75Driver(
        address_bit_count=5,
        shift_register_depth=128,
        base_address_pin=Pin(8, Pin.OUT),
        output_enable_pin=Pin(7, Pin.OUT),
        base_clock_pin=Pin(5, Pin.OUT),
        base_data_pin=Pin(16, Pin.OUT),
        data_frequency=data_freq,
        address_frequency=addr_freq
    )
    display = Hub75Display(driver)
    writer = FontWriter(display.frame_buffer, default_font=unscii_8)
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
    writer.center_text("BOOTING", 4, colors['accent'], width=DISPLAY_WIDTH, font=unscii_16)

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
    writer.center_text(operation, 42, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)

    # Detail text (truncate to 25 chars)
    if detail:
        if len(detail) > 25:
            detail = detail[:24] + '.'
        writer.center_text(detail, 54, colors['secondary'], width=DISPLAY_WIDTH, font=spleen_5x8)


def render_idle(display, writer, colors):
    """Render idle/waiting screen."""
    display.fill(BLACK)
    writer.center_text("PICO", 16, colors['primary'], width=DISPLAY_WIDTH, font=unscii_16)
    writer.center_text("SCOREBOARD", 40, colors['accent'], width=DISPLAY_WIDTH)


def render_no_games(display, writer, colors):
    """Render no games scheduled screen."""
    display.fill(BLACK)
    writer.center_text("NO GAMES", 20, colors['primary'], width=DISPLAY_WIDTH, font=unscii_16)
    writer.center_text("scheduled", 40, colors['secondary'], width=DISPLAY_WIDTH, font=spleen_5x8)


def render_setup(display, writer, state, colors):
    """
    Render setup mode screen with contextual information.

    Shows different content based on the reason for entering setup mode:
    - no_config: First-time setup instructions (with QR code)
    - connection_failed: WiFi connection failure with SSID
    - bad_auth: Wrong password error with SSID
    """
    display.fill(BLACK)

    setup = state.get('setup', {})
    reason = setup.get('reason', 'no_config')
    ap_ssid = setup.get('ap_ssid', 'scoreboard')
    ap_ip = setup.get('ap_ip', '192.168.4.1')
    wifi_ssid = setup.get('wifi_ssid', '')

    # Truncate SSIDs if too long (max ~20 chars for spleen_5x8)
    ap_ssid_display = ap_ssid[:18] if len(ap_ssid) > 18 else ap_ssid
    wifi_ssid_display = wifi_ssid[:18] if len(wifi_ssid) > 18 else wifi_ssid

    if reason == 'bad_auth':
        # Wrong password
        writer.center_text("BAD PASSWORD", 0, colors['clock_warning'], width=DISPLAY_WIDTH, font=unscii_16)
        writer.center_text("Auth failed for:", 20, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)
        writer.center_text(f'"{wifi_ssid_display}"', 30, colors['secondary'], width=DISPLAY_WIDTH, font=spleen_5x8)
        # Common footer
        writer.center_text("Open browser to:", 44, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)
        writer.center_text(ap_ip, 54, colors['accent'], width=DISPLAY_WIDTH, font=spleen_5x8)
    elif reason == 'connection_failed':
        # Could not connect (timeout, not found, etc.)
        writer.center_text("WIFI FAIL", 0, colors['clock_warning'], width=DISPLAY_WIDTH, font=unscii_16)
        writer.center_text("Could not connect to:", 20, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)
        writer.center_text(f'"{wifi_ssid_display}"', 30, colors['secondary'], width=DISPLAY_WIDTH, font=spleen_5x8)
        # Common footer
        writer.center_text("Open browser to:", 44, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)
        writer.center_text(ap_ip, 54, colors['accent'], width=DISPLAY_WIDTH, font=spleen_5x8)
    else:
        # ============================================================
        # TEMPORARY CODE - QR code for first-time setup
        # ============================================================
        # no_config - first-time setup with QR code
        try:
            from lib.scoreboard.qr_data import get_qr_framebuffer, QR_WIDTH, QR_HEIGHT
            qr_fb = get_qr_framebuffer()
            # Position QR code on the right side, vertically centered in top area
            qr_x = DISPLAY_WIDTH - QR_WIDTH - 2  # 2px padding from right
            qr_y = 2  # 2px from top
            display.blit(qr_fb, qr_x, qr_y)

            # Text on left side (narrower area)
            text_width = qr_x - 4  # Leave gap before QR
            writer.text("SETUP", 2, 0, colors['accent'], font=unscii_16)
            writer.text("Scan QR", 2, 18, colors['primary'], font=spleen_5x8)
            writer.text("to connect", 2, 28, colors['primary'], font=spleen_5x8)

            # Footer (full width, below QR code area)
            writer.text("Or browse to:", 2, 44, colors['secondary'], font=spleen_5x8)
            writer.text(ap_ip, 2, 54, colors['accent'], font=spleen_5x8)
        except ImportError:
            # Fallback if QR module not available
            writer.center_text("SETUP", 0, colors['accent'], width=DISPLAY_WIDTH, font=unscii_16)
            writer.center_text("Connect to WiFi:", 20, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)
            writer.center_text(f'"{ap_ssid_display}"', 30, colors['accent'], width=DISPLAY_WIDTH, font=spleen_5x8)
            writer.center_text("Open browser to:", 44, colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)
            writer.center_text(ap_ip, 54, colors['accent'], width=DISPLAY_WIDTH, font=spleen_5x8)
        # ============================================================
        # END TEMPORARY CODE
        # ============================================================


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
    writer.center_text(title or 'ERROR', 0, colors['clock_warning'], width=DISPLAY_WIDTH, font=unscii_16)

    # Detail lines (up to 4, using spleen_5x8)
    y_start = 24
    line_height = 10
    for i, line in enumerate(lines[:4]):
        # Truncate line to fit (25 chars for spleen_5x8)
        display_line = line[:25] if len(line) > 25 else line
        writer.center_text(display_line, y_start + (i * line_height), colors['primary'], width=DISPLAY_WIDTH, font=spleen_5x8)


def render_pregame(display, writer, game, state, colors, home_logo=None, away_logo=None):
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
    """
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if away_logo:
        display.blit(away_logo, 1, 0)
    if home_logo:
        display.blit(home_logo, 103, 0)  # 128 - 24 - 1

    # "@" in center
    writer.center_text("@", 4, colors['primary'], width=DISPLAY_WIDTH, font=unscii_16)

    # Use pre-formatted date/time from state (no allocation)
    display_data = state['display']
    date_display = display_data['pregame_date']
    time_str = display_data['pregame_time']

    # Team abbreviations aligned to logos, date centered (all at Y=26)
    writer.text(game.away.abbreviation, 1, 26, away_color)  # Left-aligned
    home_abbr_x = 103 + 24 - len(game.home.abbreviation) * 8
    writer.text(game.home.abbreviation, home_abbr_x, 26, home_color)  # Right-aligned
    writer.center_text(date_display, 22, colors['secondary'], width=DISPLAY_WIDTH)

    # Time
    writer.center_text(time_str, 32, colors['accent'], width=DISPLAY_WIDTH)

    # Venue (single line, scrolls if too long)
    if game.venue:
        venue_width = writer.measure(game.venue, unscii_8)
        if venue_width <= DISPLAY_WIDTH:
            # Fits - center it
            writer.center_text(game.venue, 46, colors['secondary'], width=DISPLAY_WIDTH)
        else:
            # Too long - use scroller
            scroller = get_or_create_scroller('venue', game.venue, venue_width, DISPLAY_WIDTH)
            offset = scroller.get_offset()
            writer.text(game.venue, -offset, 46, colors['secondary'], font=unscii_8)


def render_live(display, writer, game, state, colors, home_logo=None, away_logo=None):
    """
    Render live game with logos and scores on 128x64 display.

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
    """
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if away_logo:
        display.blit(away_logo, 1, 0)
    if home_logo:
        display.blit(home_logo, 103, 0)  # 128 - 24 - 1

    # Away score (to the right of away logo) - ZERO ALLOCATIONS
    writer.integer(game.away.score, 29, 8, away_color, font=unscii_16)

    # Home score (to the left of home logo, right-aligned) - ZERO ALLOCATIONS
    writer.integer(game.home.score, 0, 8, home_color, font=unscii_16,
                   right_align=True, right_x=99)

    # Team abbreviations aligned to logos
    writer.text(game.away.abbreviation, 1, 26, away_color)  # Left-aligned
    home_abbr_x = 103 + 24 - len(game.home.abbreviation) * 8
    writer.text(game.home.abbreviation, home_abbr_x, 26, home_color)  # Right-aligned

    # Clock - color changes as time runs low (centered at Y=22, between abbreviations)
    clock_seconds = state.get('clock_seconds') or 0
    if clock_seconds < 40:
        clock_color = colors['clock_warning']
    else:
        clock_color = colors['clock_normal']
    writer.clock(clock_seconds, 0, 22, clock_color, centered=True, width=DISPLAY_WIDTH)

    # Quarter and situation from pre-formatted state (no allocation)
    display_data = state['display']
    quarter_display = display_data['quarter']
    situation_str = display_data['situation']

    if situation_str:
        combined = f"{quarter_display}  {situation_str}"
        writer.center_text(combined, 54, colors['primary'], width=DISPLAY_WIDTH)
    else:
        writer.center_text(quarter_display, 54, colors['primary'], width=DISPLAY_WIDTH)

    # Last play text with scrolling if too long
    last_play_text = display_data.get('last_play_text', '')
    if last_play_text:
        text_width = writer.measure(last_play_text, unscii_8)
        if text_width <= DISPLAY_WIDTH:
            writer.center_text(last_play_text, 44, colors['secondary'],
                              width=DISPLAY_WIDTH, font=unscii_8)
        else:
            scroller = get_or_create_scroller('last_play', last_play_text,
                                              text_width, DISPLAY_WIDTH)
            offset = scroller.get_offset()
            writer.text(last_play_text, -offset, 44, colors['secondary'], font=unscii_8)


def render_final(display, writer, game, state, colors, home_logo=None, away_logo=None):
    """
    Render final score with logos on 128x64 display.

    Uses zero-allocation integer rendering for scores.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        game: FinalGame object
        state: Display state dict (unused but kept for consistent signature)
        colors: UI colors dict
        home_logo: Home team logo FrameBuffer (from state)
        away_logo: Away team logo FrameBuffer (from state)
    """
    _ = state  # Unused - final screen doesn't need pre-formatted strings
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if away_logo:
        display.blit(away_logo, 1, 0)
    if home_logo:
        display.blit(home_logo, 103, 0)  # 128 - 24 - 1

    # Away score (to the right of away logo) - ZERO ALLOCATIONS
    writer.integer(game.away.score, 29, 8, away_color, font=unscii_16)

    # Home score (to the left of home logo, right-aligned) - ZERO ALLOCATIONS
    writer.integer(game.home.score, 0, 8, home_color, font=unscii_16,
                   right_align=True, right_x=99)

    # Team abbreviations aligned to logos
    writer.text(game.away.abbreviation, 1, 26, away_color)  # Left-aligned
    home_abbr_x = 103 + 24 - len(game.home.abbreviation) * 8
    writer.text(game.home.abbreviation, home_abbr_x, 26, home_color)  # Right-aligned

    # Final status
    status_text = "FINAL"
    if game.status == "final/OT":
        status_text = "F/OT"
    writer.center_text(status_text, 40, colors['primary'], width=DISPLAY_WIDTH, font=unscii_16)


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
    # Clock is at Y=22 (centered), using unscii_16 (16px tall)
    # Only clear the center portion to preserve team abbreviations on the edges
    clock_y = 22
    clock_height = 16
    clock_clear_x = 32  # Start of center region (preserves left abbreviation)
    clock_clear_width = 64  # Center 64px (32 to 96)

    # Clear only the center portion where clock is drawn
    display.fill_rect(clock_clear_x, clock_y, clock_clear_width, clock_height, BLACK)

    # Determine clock color based on time remaining (integer comparison)
    if seconds < 40:  # Under 40 seconds
        clock_color = colors['clock_warning']
    else:
        clock_color = colors['clock_normal']

    # Use zero-allocation clock method (font already set via init_clock)
    writer.clock(seconds, 0, clock_y, clock_color, centered=True, width=DISPLAY_WIDTH)


def render_frame(display, writer, state, colors):
    """
    Render a frame based on current display state.

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        state: Display state dict (contains mode, game, logos, etc.)
        colors: UI colors dict
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
        render_setup(display, writer, state, colors)
    elif mode == 'error':
        render_error(display, writer, state, colors)
    elif mode == 'game':
        game = state.get('game')
        if game is None:
            render_idle(display, writer, colors)
        elif game.state == STATE_PREGAME:
            render_pregame(display, writer, game, state, colors,
                          home_logo=home_logo, away_logo=away_logo)
        elif game.state == STATE_LIVE:
            render_live(display, writer, game, state, colors,
                       home_logo=home_logo, away_logo=away_logo)
        elif game.state == STATE_FINAL:
            render_final(display, writer, game, state, colors,
                        home_logo=home_logo, away_logo=away_logo)
        else:
            render_idle(display, writer, colors)
    else:
        render_idle(display, writer, colors)



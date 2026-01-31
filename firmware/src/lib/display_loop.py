"""
Async display rendering loop for the Pico Scoreboard.

Runs as an asyncio task, reading from global display state and rendering
to the HUB75 LED matrix at 1-second intervals.
"""

import time
import uasyncio as asyncio
from machine import Pin
from hub75 import Hub75Driver, Hub75Display
from lib.fonts import FontWriter, unscii_8, unscii_16, spleen_5x8, rgb565
from lib.scoreboard.state import display_state, parse_clock, format_clock
from lib.scoreboard.models import STATE_PREGAME, STATE_LIVE, STATE_FINAL
from lib.hub75.image import rgb888_to_rgb565_into_buffer
from lib.animations import ScrollingText

# Fixed color
BLACK = 0

# Day name abbreviations (Monday=0 to Sunday=6)
DAY_NAMES = ("MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN")

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


def init_display():
    """
    Initialize and return HUB75 display hardware.

    Returns:
        Tuple of (driver, display, writer)
    """
    driver = Hub75Driver(
        address_bit_count=5,
        shift_register_depth=128,
        base_address_pin=Pin(8, Pin.OUT),
        output_enable_pin=Pin(7, Pin.OUT),
        base_clock_pin=Pin(5, Pin.OUT),
        base_data_pin=Pin(16, Pin.OUT)
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


def parse_iso_datetime(iso_str):
    """
    Parse ISO datetime string and return (day_abbr, date_str, time_str) tuple.

    Input: "2024-01-15T19:30:00Z" or "2024-01-15T19:30Z"
    Output: ("WED", "01/15", "7:30 PM")

    Falls back gracefully if format is not ISO (e.g., old "7:30 PM ET" format).
    Note: MicroPython lacks strptime, so we use string slicing.
    """
    # Check if it looks like ISO format (contains 'T')
    if 'T' not in iso_str:
        # Old format - strip timezone suffix if present (e.g., " ET", " PT")
        time_str = iso_str
        for tz in [" ET", " PT", " CT", " MT", " EST", " PST", " CST", " MST"]:
            if time_str.endswith(tz):
                time_str = time_str[:-len(tz)]
                break
        return ("", "", time_str)

    try:
        # Split date and time parts
        date_part = iso_str[0:10]   # "2024-01-15"
        time_part = iso_str[11:16]  # "19:30"

        # Extract year, month, day as integers
        year = int(date_part[0:4])
        month = int(date_part[5:7])
        day = int(date_part[8:10])

        # Format date as MM/DD
        date_str = f"{month:02d}/{day:02d}"

        # Calculate day of week using time module
        time_tuple = (year, month, day, 0, 0, 0, 0, 0)
        timestamp = time.mktime(time_tuple)
        weekday = time.localtime(timestamp)[6]  # 0=Monday, 6=Sunday
        day_abbr = DAY_NAMES[weekday]

        # Format time as 12-hour with AM/PM
        hour = int(time_part[0:2])
        minute = time_part[3:5]
        am_pm = "AM" if hour < 12 else "PM"
        if hour == 0:
            hour = 12
        elif hour > 12:
            hour -= 12
        time_str = f"{hour}:{minute} {am_pm}"

        return (day_abbr, date_str, time_str)
    except (ValueError, IndexError):
        # Parsing failed - return original string with no date
        return ("", "", iso_str)


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


def render_setup(display, writer, colors):
    """Render setup mode screen."""
    display.fill(BLACK)
    writer.center_text("SETUP", 8, colors['accent'], width=DISPLAY_WIDTH, font=unscii_16)
    writer.center_text("Connect to", 32, colors['primary'], width=DISPLAY_WIDTH)
    writer.center_text("WiFi AP", 44, colors['primary'], width=DISPLAY_WIDTH)


def render_error(display, writer, message, colors):
    """Render error screen."""
    display.fill(BLACK)
    writer.center_text("ERROR", 8, colors['clock_warning'], width=DISPLAY_WIDTH, font=unscii_16)
    if message:
        # Truncate long messages
        if len(message) > 10:
            message = message[:10]
        writer.center_text(message, 36, colors['primary'], width=DISPLAY_WIDTH)


def render_pregame(display, writer, game, colors, api_client):
    """Render pregame screen with team matchup on 128x64 display."""
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if api_client:
        away_logo = get_logo_framebuffer(api_client, game.away.abbreviation)
        if away_logo:
            display.blit(away_logo, 1, 0)

        home_logo = get_logo_framebuffer(api_client, game.home.abbreviation)
        if home_logo:
            display.blit(home_logo, 103, 0)  # 128 - 24 - 1

    # "@" in center
    writer.center_text("@", 4, colors['primary'], width=DISPLAY_WIDTH, font=unscii_16)

    # Parse ISO datetime for day, date and time
    day_abbr, date_str, time_str = parse_iso_datetime(game.start_time)

    # Team abbreviations aligned to logos, date centered (all at Y=26)
    writer.text(game.away.abbreviation, 1, 26, away_color)  # Left-aligned
    home_abbr_x = 103 + 24 - len(game.home.abbreviation) * 8
    writer.text(game.home.abbreviation, home_abbr_x, 26, home_color)  # Right-aligned
    date_display = f"{day_abbr} {date_str}" if day_abbr else date_str
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


def render_live(display, writer, game, colors, api_client):
    """Render live game with logos and scores on 128x64 display."""
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if api_client:
        away_logo = get_logo_framebuffer(api_client, game.away.abbreviation)
        if away_logo:
            display.blit(away_logo, 1, 0)

        home_logo = get_logo_framebuffer(api_client, game.home.abbreviation)
        if home_logo:
            display.blit(home_logo, 103, 0)  # 128 - 24 - 1

    # Away score (to the right of away logo)
    away_score_str = str(game.away.score)
    writer.text(away_score_str, 29, 8, away_color, font=unscii_16)

    # Home score (to the left of home logo, right-aligned)
    home_score_str = str(game.home.score)
    home_score_width = len(home_score_str) * 8
    home_score_x = 99 - home_score_width
    writer.text(home_score_str, home_score_x, 8, home_color, font=unscii_16)

    # Team abbreviations aligned to logos
    writer.text(game.away.abbreviation, 1, 26, away_color)  # Left-aligned
    home_abbr_x = 103 + 24 - len(game.home.abbreviation) * 8
    writer.text(game.home.abbreviation, home_abbr_x, 26, home_color)  # Right-aligned

    # Clock - color changes as time runs low
    clock = game.clock
    if clock.startswith("0:3") or clock.startswith("0:2") or clock.startswith("0:1") or clock.startswith("0:0"):
        clock_color = colors['clock_warning']
    else:
        clock_color = colors['clock_normal']
    writer.center_text(clock, 38, clock_color, width=DISPLAY_WIDTH, font=unscii_16)

    # Quarter and situation
    quarter_display = format_quarter(game.quarter)
    situation_str = format_situation(game.situation) if game.situation else None

    if situation_str:
        combined = f"{quarter_display}  {situation_str}"
        writer.center_text(combined, 54, colors['primary'], width=DISPLAY_WIDTH)
    else:
        writer.center_text(quarter_display, 54, colors['primary'], width=DISPLAY_WIDTH)


def render_final(display, writer, game, colors, api_client):
    """Render final score with logos on 128x64 display."""
    display.fill(BLACK)

    # Team colors (with dark color protection)
    home_color = safe_team_color(game.home.color, colors['secondary'])
    away_color = safe_team_color(game.away.color, colors['secondary'])

    # Team logos (24x24 each) with 1px padding
    if api_client:
        away_logo = get_logo_framebuffer(api_client, game.away.abbreviation)
        if away_logo:
            display.blit(away_logo, 1, 0)

        home_logo = get_logo_framebuffer(api_client, game.home.abbreviation)
        if home_logo:
            display.blit(home_logo, 103, 0)  # 128 - 24 - 1

    # Away score (to the right of away logo)
    away_score_str = str(game.away.score)
    writer.text(away_score_str, 29, 8, away_color, font=unscii_16)

    # Home score (to the left of home logo, right-aligned)
    home_score_str = str(game.home.score)
    home_score_width = len(home_score_str) * 8
    home_score_x = 99 - home_score_width
    writer.text(home_score_str, home_score_x, 8, home_color, font=unscii_16)

    # Team abbreviations aligned to logos
    writer.text(game.away.abbreviation, 1, 26, away_color)  # Left-aligned
    home_abbr_x = 103 + 24 - len(game.home.abbreviation) * 8
    writer.text(game.home.abbreviation, home_abbr_x, 26, home_color)  # Right-aligned

    # Final status
    status_text = "FINAL"
    if game.status == "final/OT":
        status_text = "F/OT"
    writer.center_text(status_text, 40, colors['primary'], width=DISPLAY_WIDTH, font=unscii_16)


def redraw_clock_only(display, writer, clock_str, colors):
    """
    Redraw only the clock region (partial update).

    Clears the clock strip and redraws the clock text without
    touching other parts of the display (logos, scores, etc.).

    Args:
        display: Hub75Display instance
        writer: FontWriter instance
        clock_str: Clock string to display (e.g., "3:45")
        colors: Color dict with clock_normal and clock_warning
    """
    # Clock is at Y=38, using unscii_16 (16px tall)
    clock_y = 38
    clock_height = 16

    # Clear the entire clock row (full width handles varying clock widths)
    display.fill_rect(0, clock_y, DISPLAY_WIDTH, clock_height, BLACK)

    # Determine clock color based on time remaining
    if clock_str.startswith("0:3") or clock_str.startswith("0:2") or \
       clock_str.startswith("0:1") or clock_str.startswith("0:0"):
        clock_color = colors['clock_warning']
    else:
        clock_color = colors['clock_normal']

    writer.center_text(clock_str, clock_y, clock_color, width=DISPLAY_WIDTH, font=unscii_16)


def format_quarter(quarter):
    """Format quarter for display."""
    quarter_map = {
        "first": "Q1",
        "second": "Q2",
        "third": "Q3",
        "fourth": "Q4",
        "OT": "OT",
        "OT2": "2OT",
    }
    return quarter_map.get(quarter, quarter.upper()[:3])


def format_situation(situation):
    """Format down and distance for display."""
    if situation is None:
        return None
    down_map = {
        "first": "1st",
        "second": "2nd",
        "third": "3rd",
        "fourth": "4th",
    }
    down_str = down_map.get(situation.down, situation.down)
    return f"{down_str} & {situation.distance}"


def render_frame(display, writer, state, colors, api_client):
    """Render a frame based on current display state."""
    mode = state.get('mode', 'idle')

    if mode == 'startup':
        render_startup(display, writer, state, colors)
    elif mode == 'idle':
        render_idle(display, writer, colors)
    elif mode == 'setup':
        render_setup(display, writer, colors)
    elif mode == 'error':
        render_error(display, writer, state.get('error_message'), colors)
    elif mode == 'game':
        game = state.get('game')
        if game is None:
            render_idle(display, writer, colors)
        elif game.state == STATE_PREGAME:
            render_pregame(display, writer, game, colors, api_client)
        elif game.state == STATE_LIVE:
            render_live(display, writer, game, colors, api_client)
        elif game.state == STATE_FINAL:
            render_final(display, writer, game, colors, api_client)
        else:
            render_idle(display, writer, colors)
    else:
        render_idle(display, writer, colors)


async def display_loop(config, api_client, display=None, writer=None):
    """
    Main display rendering loop.

    Runs continuously, reading from display_state and updating the display.
    Uses asyncio.sleep() to yield control between frames.

    When a live game clock is running, updates at 200ms intervals and
    decrements the clock locally. Otherwise updates at 1 second intervals.

    Args:
        config: Config instance for reading UI colors
        api_client: ScoreboardApiClient instance for fetching team logos
        display: Optional pre-initialized Hub75Display (for startup display)
        writer: Optional pre-initialized FontWriter (for startup display)
    """
    if display is None or writer is None:
        print("Initializing display...")
        driver, display, writer = init_display()
        print("Display initialized")

    while True:
        try:
            # Check if we're in a live game with clock running
            clock_running = False
            game = display_state.get('game')
            if (display_state['mode'] == 'game' and
                game is not None and
                game.state == STATE_LIVE and
                hasattr(game, 'clock_running') and
                game.clock_running):
                clock_running = True

            # Check if scrolling animation is active (pregame with long venue)
            scrolling_active = False
            if (display_state['mode'] == 'game' and
                game is not None and
                game.state == STATE_PREGAME and
                'venue' in _scrollers and
                _scrollers['venue'].needs_scrolling):
                scrolling_active = True

            # Tick the clock if running
            if clock_running and display_state['clock_seconds'] is not None:
                now_ms = time.ticks_ms()
                elapsed_ms = time.ticks_diff(now_ms, display_state['clock_last_tick_ms'])

                # Decrement by whole seconds elapsed
                if elapsed_ms >= 1000:
                    seconds_elapsed = elapsed_ms // 1000
                    display_state['clock_seconds'] = max(0, display_state['clock_seconds'] - seconds_elapsed)
                    display_state['clock_last_tick_ms'] = now_ms
                    display_state['clock_dirty'] = True

            # Handle rendering
            if display_state['dirty'] or scrolling_active:
                # Full redraw (score change, quarter change, API update, scrolling, etc.)
                colors = get_ui_colors(config)
                render_frame(display, writer, display_state, colors, api_client)
                display.show()
                display_state['dirty'] = False
                display_state['clock_dirty'] = False
            elif display_state.get('clock_dirty') and clock_running:
                # Partial redraw - clock only
                colors = get_ui_colors(config)
                clock_str = format_clock(display_state['clock_seconds'])
                redraw_clock_only(display, writer, clock_str, colors)
                display.show()
                display_state['clock_dirty'] = False

        except Exception as e:
            print(f"Display loop error: {e}")
            # Don't crash - keep trying

        # Dynamic sleep based on animation state
        if scrolling_active:
            await asyncio.sleep(0.05)  # 50ms = 20 FPS for smooth scrolling
        elif clock_running:
            await asyncio.sleep(0.2)   # 200ms for clock updates
        else:
            await asyncio.sleep(1)     # 1 second when idle

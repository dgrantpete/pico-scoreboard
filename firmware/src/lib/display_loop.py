"""
Async display rendering loop for the Pico Scoreboard.

Runs as an asyncio task, reading from global display state and rendering
to the HUB75 LED matrix at 1-second intervals.
"""

import uasyncio as asyncio
from machine import Pin
from hub75 import Hub75Driver, Hub75Display
from lib.fonts import FontWriter, unscii_8, unscii_16, rgb565
from lib.scoreboard.state import display_state
from lib.scoreboard.models import STATE_PREGAME, STATE_LIVE, STATE_FINAL

# Colors (RGB565)
WHITE = rgb565(255, 255, 255)
RED = rgb565(255, 10, 10)
BLUE = rgb565(50, 150, 255)
YELLOW = rgb565(255, 255, 0)
GREEN = rgb565(0, 255, 0)
GRAY = rgb565(128, 128, 128)
BLACK = 0

# Display dimensions
DISPLAY_WIDTH = 64
DISPLAY_HEIGHT = 64


def init_display():
    """
    Initialize and return HUB75 display hardware.

    Returns:
        Tuple of (driver, display, writer)
    """
    driver = Hub75Driver(
        address_bit_count=5,
        shift_register_depth=64,
        base_address_pin=Pin(9, Pin.OUT),
        output_enable_pin=Pin(8, Pin.OUT),
        base_clock_pin=Pin(6, Pin.OUT),
        base_data_pin=Pin(0, Pin.OUT)
    )
    display = Hub75Display(driver)
    writer = FontWriter(display.frame_buffer, default_font=unscii_8)
    return driver, display, writer


def color_to_rgb565(color):
    """Convert a Color object to RGB565."""
    return rgb565(color.r, color.g, color.b)


def render_idle(display, writer):
    """Render idle/waiting screen."""
    display.fill(BLACK)
    writer.center_text("PICO", 16, WHITE, width=DISPLAY_WIDTH, font=unscii_16)
    writer.center_text("SCOREBOARD", 40, YELLOW, width=DISPLAY_WIDTH)


def render_setup(display, writer):
    """Render setup mode screen."""
    display.fill(BLACK)
    writer.center_text("SETUP", 8, YELLOW, width=DISPLAY_WIDTH, font=unscii_16)
    writer.center_text("Connect to", 32, WHITE, width=DISPLAY_WIDTH)
    writer.center_text("WiFi AP", 44, WHITE, width=DISPLAY_WIDTH)


def render_error(display, writer, message):
    """Render error screen."""
    display.fill(BLACK)
    writer.center_text("ERROR", 8, RED, width=DISPLAY_WIDTH, font=unscii_16)
    if message:
        # Truncate long messages
        if len(message) > 10:
            message = message[:10]
        writer.center_text(message, 36, WHITE, width=DISPLAY_WIDTH)


def render_pregame(display, writer, game):
    """Render pregame screen with team matchup."""
    display.fill(BLACK)

    # Team colors
    home_color = color_to_rgb565(game.home.color)
    away_color = color_to_rgb565(game.away.color)

    # Team abbreviations at top
    writer.text(game.away.abbreviation, 2, 0, away_color)
    writer.text("@", 28, 0, WHITE)
    writer.text(game.home.abbreviation, 38, 0, home_color)

    # Divider
    display.hline(0, 12, DISPLAY_WIDTH, GRAY)

    # Start time (simplified - show first 5 chars like "12:30")
    time_str = game.start_time[:5] if len(game.start_time) >= 5 else game.start_time
    writer.center_text(time_str, 20, YELLOW, width=DISPLAY_WIDTH, font=unscii_16)

    # Venue if available
    if game.venue:
        venue = game.venue[:10] if len(game.venue) > 10 else game.venue
        writer.center_text(venue, 44, GRAY, width=DISPLAY_WIDTH)


def render_live(display, writer, game):
    """Render live game with scores."""
    display.fill(BLACK)

    # Team colors
    home_color = color_to_rgb565(game.home.color)
    away_color = color_to_rgb565(game.away.color)

    # Away team (left side)
    writer.text(game.away.abbreviation, 2, 0, away_color)
    away_score_str = str(game.away.score)
    away_x = 10 if game.away.score < 10 else 6
    writer.text(away_score_str, away_x, 12, away_color, font=unscii_16)

    # Home team (right side)
    writer.text(game.home.abbreviation, 34, 0, home_color)
    home_score_str = str(game.home.score)
    home_x = 42 if game.home.score < 10 else 38
    writer.text(home_score_str, home_x, 12, home_color, font=unscii_16)

    # Divider line
    display.hline(0, 32, DISPLAY_WIDTH, WHITE)

    # Clock - color changes as time runs low
    clock = game.clock
    if clock.startswith("0:3") or clock.startswith("0:2") or clock.startswith("0:1") or clock.startswith("0:0"):
        clock_color = RED
    else:
        clock_color = GREEN
    writer.center_text(clock, 36, clock_color, width=DISPLAY_WIDTH, font=unscii_16)

    # Quarter
    quarter_display = format_quarter(game.quarter)
    writer.center_text(quarter_display, 54, WHITE, width=DISPLAY_WIDTH)


def render_final(display, writer, game):
    """Render final score."""
    display.fill(BLACK)

    # Team colors
    home_color = color_to_rgb565(game.home.color)
    away_color = color_to_rgb565(game.away.color)

    # Away team (left side)
    writer.text(game.away.abbreviation, 2, 0, away_color)
    away_score_str = str(game.away.score)
    away_x = 10 if game.away.score < 10 else 6
    writer.text(away_score_str, away_x, 12, away_color, font=unscii_16)

    # Home team (right side)
    writer.text(game.home.abbreviation, 34, 0, home_color)
    home_score_str = str(game.home.score)
    home_x = 42 if game.home.score < 10 else 38
    writer.text(home_score_str, home_x, 12, home_color, font=unscii_16)

    # Divider line
    display.hline(0, 32, DISPLAY_WIDTH, WHITE)

    # Final status
    status_text = "FINAL"
    if game.status == "final/OT":
        status_text = "F/OT"
    writer.center_text(status_text, 40, WHITE, width=DISPLAY_WIDTH, font=unscii_16)


def format_quarter(quarter):
    """Format quarter for display."""
    quarter_map = {
        "first": "1ST",
        "second": "2ND",
        "third": "3RD",
        "fourth": "4TH",
        "OT": "OT",
        "OT2": "2OT",
    }
    return quarter_map.get(quarter, quarter.upper()[:3])


def render_frame(display, writer, state):
    """Render a frame based on current display state."""
    mode = state.get('mode', 'idle')

    if mode == 'idle':
        render_idle(display, writer)
    elif mode == 'setup':
        render_setup(display, writer)
    elif mode == 'error':
        render_error(display, writer, state.get('error_message'))
    elif mode == 'game':
        game = state.get('game')
        if game is None:
            render_idle(display, writer)
        elif game.state == STATE_PREGAME:
            render_pregame(display, writer, game)
        elif game.state == STATE_LIVE:
            render_live(display, writer, game)
        elif game.state == STATE_FINAL:
            render_final(display, writer, game)
        else:
            render_idle(display, writer)
    else:
        render_idle(display, writer)


async def display_loop():
    """
    Main display rendering loop.

    Runs continuously, reading from display_state and updating the display.
    Uses asyncio.sleep() to yield control between frames.
    """
    print("Initializing display...")
    driver, display, writer = init_display()
    print("Display initialized")

    while True:
        try:
            if display_state['dirty']:
                render_frame(display, writer, display_state)
                display.show()  # Non-blocking: queues DMA transfer
                display_state['dirty'] = False
        except Exception as e:
            print(f"Display loop error: {e}")
            # Don't crash - keep trying

        await asyncio.sleep(1)  # 1 second between updates

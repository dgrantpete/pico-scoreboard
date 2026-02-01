"""
Synchronous display rendering thread for the Pico Scoreboard.

Runs on Core 1 as a dedicated thread, reading from the front buffer
of the double-buffered display state and rendering to the HUB75 LED matrix.

This thread is completely isolated from networking operations on Core 0,
ensuring smooth display updates even during network blocking operations.
"""

import time
from lib.scoreboard.state import get_display_state
from lib.scoreboard.models import STATE_PREGAME, STATE_LIVE, STATE_FINAL
from lib.display_loop import (
    render_frame,
    redraw_clock_only,
    get_or_create_scroller,
    _scrollers,
    DISPLAY_WIDTH,
)


def run_display_thread(display, writer, config):
    """
    Main entry point for Core 1 display thread.

    Runs a tight synchronous loop reading state from the front buffer
    and rendering to the display. Uses time.sleep_ms() for timing
    instead of async/await.

    IMPORTANT: This function runs on Core 1 with ZERO memory allocations.
    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    UI colors are pre-computed on Core 0 and read from state['ui_colors'].

    Args:
        display: Hub75Display instance (pre-initialized on Core 0)
        writer: FontWriter instance (pre-initialized on Core 0)
        config: Config instance (unused - colors pre-computed in state)
    """
    _ = config  # Unused - colors are pre-computed in state['ui_colors']
    print("Display thread starting on Core 1...")

    while True:
        try:
            # Read from front buffer (lock-free!)
            state = get_display_state()

            # Check animation states
            game = state.get('game')
            clock_running = False
            scrolling_active = False

            if (state['mode'] == 'game' and
                game is not None and
                game.state == STATE_LIVE and
                hasattr(game, 'clock_running') and
                game.clock_running):
                clock_running = True

            if (state['mode'] == 'game' and
                game is not None and
                game.state == STATE_PREGAME and
                'venue' in _scrollers and
                _scrollers['venue'].needs_scrolling):
                scrolling_active = True

            if (state['mode'] == 'game' and
                game is not None and
                game.state == STATE_LIVE and
                'last_play' in _scrollers and
                _scrollers['last_play'].needs_scrolling):
                scrolling_active = True

            # Tick the clock locally if running
            if clock_running and state['clock_seconds'] is not None:
                now_ms = time.ticks_ms()
                elapsed_ms = time.ticks_diff(now_ms, state['clock_last_tick_ms'])

                # Decrement by whole seconds elapsed
                if elapsed_ms >= 1000:
                    seconds_elapsed = elapsed_ms // 1000
                    state['clock_seconds'] = max(0, state['clock_seconds'] - seconds_elapsed)
                    state['clock_last_tick_ms'] = now_ms
                    state['clock_dirty'] = True

            # Handle rendering
            # Use pre-computed colors from state (no allocation!)
            colors = state['ui_colors']

            if state['dirty'] or scrolling_active:
                # Full redraw (score change, quarter change, API update, scrolling, etc.)
                render_frame(display, writer, state, colors)
                display.show()
                state['dirty'] = False
                state['clock_dirty'] = False
            elif state.get('clock_dirty') and clock_running:
                # Partial redraw - clock only (zero allocations)
                redraw_clock_only(display, writer, state['clock_seconds'], colors)
                display.show()
                state['clock_dirty'] = False

        except Exception as e:
            print(f"Display thread error: {e}")
            # Don't crash - keep trying

        # Dynamic sleep based on animation state
        if scrolling_active:
            time.sleep_ms(50)   # 50ms = 20 FPS for smooth scrolling
        elif clock_running:
            time.sleep_ms(200)  # 200ms for clock updates
        else:
            time.sleep_ms(1000) # 1 second when idle

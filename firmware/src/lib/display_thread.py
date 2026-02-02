"""
Synchronous display rendering thread for the Pico Scoreboard.

Runs on Core 1 as a dedicated thread, reading from the front buffer
of the double-buffered display state and rendering to the HUB75 LED matrix.

This thread is completely isolated from networking operations on Core 0,
ensuring smooth display updates even during network blocking operations.

Runs at a constant 20 FPS (50ms) for consistent animation support.
"""

import time
from lib.scoreboard.state import get_display_state
from lib.display_loop import render_frame


def run_display_thread(display, writer, config):
    """
    Main entry point for Core 1 display thread.

    Runs a constant 20 FPS loop reading state from the front buffer
    and rendering to the display. The fixed frame rate ensures smooth
    animations (scrolling text, score flashing, clock updates).

    IMPORTANT: This function runs on Core 1 with ZERO memory allocations.
    All display hardware (PIO, DMA) is accessed exclusively from this thread.
    UI colors are pre-computed on Core 0 and read from state['ui_colors'].

    Args:
        display: Hub75Display instance (pre-initialized on Core 0)
        writer: FontWriter instance (pre-initialized on Core 0)
        config: Config instance (unused - colors pre-computed in state)
    """
    _ = config  # Unused - colors are pre-computed in state['ui_colors']
    print("Display thread starting on Core 1 (20 FPS)...")

    while True:
        try:
            # Capture current time once per frame (used for all timing-dependent rendering)
            now_ms = time.ticks_ms()

            # Read from front buffer (lock-free!)
            state = get_display_state()

            # Render frame using pre-computed colors (no allocation!)
            # now_ms is passed through for pure timing computations (clock, flash, scroll)
            colors = state['ui_colors']
            render_frame(display, writer, state, colors, now_ms)
            display.show()

        except Exception as e:
            print(f"Display thread error: {e}")
            # Don't crash - keep trying

        # Constant 20 FPS for all animations
        time.sleep_ms(50)

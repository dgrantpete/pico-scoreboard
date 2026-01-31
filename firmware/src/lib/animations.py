"""
Animation primitives for the Pico Scoreboard display.

Provides time-based animations that derive visual state from elapsed time
rather than maintaining explicit state machines.
"""

import time


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

    Args:
        text_width: Width of text in pixels
        display_width: Width of display area in pixels
        elapsed_ms: Milliseconds since animation started
        pause_ms: Duration to pause at start and end (default 2 seconds)
        pixels_per_second: Scroll speed (default 30 px/s)

    Returns:
        Pixel offset (0 to max_scroll). Returns 0 if text fits in display.
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


class ScrollingText:
    """
    Lightweight wrapper tracking timing for scrolling text.

    Stores minimal state (start time, text, dimensions) and computes
    the current scroll offset from elapsed time on each call.
    """

    def __init__(
        self,
        text: str,
        text_width: int,
        display_width: int,
        pause_ms: int = 2000,
        pixels_per_second: int = 30
    ):
        """
        Initialize a scrolling text instance.

        Args:
            text: The text being scrolled (stored for identity comparison)
            text_width: Pre-measured width of text in pixels
            display_width: Width of display area in pixels
            pause_ms: Duration to pause at start and end
            pixels_per_second: Scroll speed
        """
        self.text = text
        self.text_width = text_width
        self.display_width = display_width
        self.pause_ms = pause_ms
        self.pixels_per_second = pixels_per_second
        self.start_time = time.ticks_ms()

    @property
    def needs_scrolling(self) -> bool:
        """True if text is wider than display area."""
        return self.text_width > self.display_width

    def get_offset(self) -> int:
        """
        Get current scroll offset based on elapsed time.

        Returns 0 if text fits in display or during pause phases.
        """
        if not self.needs_scrolling:
            return 0
        elapsed = time.ticks_diff(time.ticks_ms(), self.start_time)
        return calculate_scroll_offset(
            self.text_width,
            self.display_width,
            elapsed,
            self.pause_ms,
            self.pixels_per_second
        )

    def reset(self):
        """Restart animation from beginning."""
        self.start_time = time.ticks_ms()

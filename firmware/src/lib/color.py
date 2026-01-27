"""Shared color conversion utilities."""
import micropython


@micropython.native
def rgb565(r: int, g: int, b: int) -> int:
    """Convert RGB888 to RGB565 color value."""
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)

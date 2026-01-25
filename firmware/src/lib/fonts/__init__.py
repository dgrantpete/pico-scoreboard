"""
Simple font rendering for Hub75 displays.

Usage:
    from lib.fonts import FontWriter, unscii_8, unscii_16

    writer = FontWriter(display.frame_buffer, default_font=unscii_8)
    writer.text("SCORE", 0, 0, color=0xFFFF)
    writer.text("24", 50, 0, color=0xF800, font=unscii_16)
"""
import framebuf


class FontWriter:
    """
    Renders text using converted bitmap fonts.

    The font modules provide 1-bit glyph bitmaps. We use framebuf.blit()
    with a 2-entry color palette to render them as colored text.
    """

    def __init__(self, framebuffer: framebuf.FrameBuffer, default_font=None):
        """
        Args:
            framebuffer: The RGB565 framebuffer to draw on
            default_font: Font module to use when none specified
        """
        self._fb = framebuffer
        self._default_font = default_font

        # Reusable palette buffer (2 RGB565 entries = 4 bytes)
        # Index 0 = background color, Index 1 = foreground color
        self._palette_buf = bytearray(4)
        self._palette = framebuf.FrameBuffer(
            self._palette_buf, 2, 1, framebuf.RGB565
        )

    def text(self, string: str, x: int, y: int, color: int,
             bgcolor: int = 0, font=None) -> int:
        """
        Draw text at position with specified color.

        Args:
            string: Text to render
            x: X position (pixels)
            y: Y position (pixels)
            color: Foreground color (RGB565)
            bgcolor: Background color (RGB565), default black
            font: Font module, or None to use default

        Returns:
            X position after last character (for chaining)
        """
        if font is None:
            font = self._default_font
        if font is None:
            raise ValueError("No font specified and no default set")

        # Set up palette: index 0 = bg, index 1 = fg
        self._palette.pixel(0, 0, bgcolor)
        self._palette.pixel(1, 0, color)

        # Render each character
        cursor_x = x
        for char in string:
            glyph_data, char_height, char_width = font.get_ch(char)

            # Create 1-bit framebuffer from glyph
            glyph_fb = framebuf.FrameBuffer(
                bytearray(glyph_data),
                char_width,
                char_height,
                framebuf.MONO_HLSB
            )

            # Blit with color palette
            self._fb.blit(glyph_fb, cursor_x, y, -1, self._palette)

            cursor_x += char_width

        return cursor_x

    def center_text(self, string: str, y: int, color: int,
                    bgcolor: int = 0, font=None, width: int = None) -> None:
        """Draw text centered horizontally."""
        if font is None:
            font = self._default_font
        if width is None:
            raise ValueError("width required for center_text")

        text_width = self.measure(string, font)
        x = (width - text_width) // 2
        self.text(string, x, y, color, bgcolor, font)

    def measure(self, string: str, font=None) -> int:
        """Measure text width in pixels."""
        if font is None:
            font = self._default_font
        if font is None:
            raise ValueError("No font specified and no default set")

        width = 0
        for char in string:
            _, _, char_width = font.get_ch(char)
            width += char_width
        return width


def draw_text(fb: framebuf.FrameBuffer, string: str, x: int, y: int,
              font, color: int, bgcolor: int = 0) -> int:
    """
    Draw text without creating a FontWriter instance.

    Less efficient for multiple calls since it recreates the palette each time.
    Use FontWriter for better performance when rendering multiple strings.
    """
    palette_buf = bytearray(4)
    palette = framebuf.FrameBuffer(palette_buf, 2, 1, framebuf.RGB565)
    palette.pixel(0, 0, bgcolor)
    palette.pixel(1, 0, color)

    cursor_x = x
    for char in string:
        glyph_data, char_height, char_width = font.get_ch(char)
        glyph_fb = framebuf.FrameBuffer(
            bytearray(glyph_data), char_width, char_height, framebuf.MONO_HMSB
        )
        fb.blit(glyph_fb, cursor_x, y, -1, palette)
        cursor_x += char_width

    return cursor_x


def rgb565(r: int, g: int, b: int) -> int:
    """Convert RGB888 to RGB565 color value."""
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


# Import font modules for convenience
from . import unscii_8
from . import unscii_16

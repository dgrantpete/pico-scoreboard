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

    Uses zero-allocation rendering by passing glyph data directly to blit()
    via a reusable list, avoiding FrameBuffer object creation per character.
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

        # Pre-allocated glyph spec for zero-allocation blit
        # Format: [buffer, width, height, format]
        # Updated in-place during rendering - no per-character allocation
        self._glyph_spec = [None, 0, 0, framebuf.MONO_HLSB]

        # Pre-cached clock digit glyphs (initialized via init_clock)
        # List indexed by digit (0-9) plus colon at index 10
        self._clock_glyphs = None

        # Pre-cached digit glyphs per font (initialized via init_digits)
        # Maps font id -> list of (glyph_data, width, height) for 0-9
        self._digit_glyphs = {}

        # Pre-allocated scratch space for integer() digit extraction
        # Avoids allocation during rendering - max 5 digits for scores
        self._int_digits = [0, 0, 0, 0, 0]

    def init_clock(self, font):
        """
        Initialize clock glyph cache. MUST be called on Core 0 during setup.

        This pre-caches all glyph data for digits 0-9 and colon so that
        clock() can render with zero allocations.

        Args:
            font: Font module to use for clock display (e.g., unscii_16)
        """
        self._clock_glyphs = [None] * 11
        for digit in range(10):
            glyph_data, char_height, char_width = font.get_ch(chr(ord('0') + digit))
            self._clock_glyphs[digit] = (glyph_data, char_width, char_height)

        # Colon at index 10
        glyph_data, char_height, char_width = font.get_ch(':')
        self._clock_glyphs[10] = (glyph_data, char_width, char_height)

    def clock(self, seconds: int, x: int, y: int, color: int,
              bgcolor: int = 0, centered: bool = False,
              width: int = None) -> int:
        """
        Render clock from seconds with ZERO allocations.

        IMPORTANT: init_clock() MUST have been called before using this method.
        Calling this without initialization will crash (by design - fail fast
        rather than allocate on Core 1).

        Args:
            seconds: Total seconds (e.g., 225 for "3:45")
            x, y: Position
            color: Foreground color (RGB565)
            bgcolor: Background color (RGB565), default black
            centered: Center horizontally on display
            width: Display width (required if centered=True)

        Returns:
            X position after last digit
        """
        # NO LAZY INIT - clock must be pre-initialized on Core 0
        # This will crash if not initialized, which is intentional
        glyphs = self._clock_glyphs

        # Calculate minutes and seconds (pure integer math)
        if seconds < 0:
            seconds = 0
        minutes = seconds // 60
        secs = seconds % 60

        # Set up palette
        self._palette.pixel(0, 0, bgcolor)
        self._palette.pixel(1, 0, color)

        # Use integer indices: 0-9 for digits, 10 for colon
        COLON = 10
        spec = self._glyph_spec

        # Calculate total width for centering
        if centered:
            total_width = 0
            if minutes >= 10:
                total_width += glyphs[minutes // 10][1]
            total_width += glyphs[minutes % 10][1]
            total_width += glyphs[COLON][1]
            total_width += glyphs[secs // 10][1]
            total_width += glyphs[secs % 10][1]
            x = (width - total_width) // 2

        cursor_x = x

        # Render: [M]M:SS - inline blit to avoid function allocation
        if minutes >= 10:
            g = glyphs[minutes // 10]
            spec[0], spec[1], spec[2] = g[0], g[1], g[2]
            self._fb.blit(spec, cursor_x, y, -1, self._palette)
            cursor_x += g[1]

        g = glyphs[minutes % 10]
        spec[0], spec[1], spec[2] = g[0], g[1], g[2]
        self._fb.blit(spec, cursor_x, y, -1, self._palette)
        cursor_x += g[1]

        g = glyphs[COLON]
        spec[0], spec[1], spec[2] = g[0], g[1], g[2]
        self._fb.blit(spec, cursor_x, y, -1, self._palette)
        cursor_x += g[1]

        g = glyphs[secs // 10]
        spec[0], spec[1], spec[2] = g[0], g[1], g[2]
        self._fb.blit(spec, cursor_x, y, -1, self._palette)
        cursor_x += g[1]

        g = glyphs[secs % 10]
        spec[0], spec[1], spec[2] = g[0], g[1], g[2]
        self._fb.blit(spec, cursor_x, y, -1, self._palette)
        cursor_x += g[1]

        return cursor_x

    def init_digits(self, font):
        """
        Pre-cache digit glyphs for a font. Call on Core 0 during setup.

        This enables zero-allocation integer rendering via the integer() method.
        Can be called for multiple fonts - each is cached by font id.

        Args:
            font: Font module to cache digits for
        """
        glyphs = [None] * 10
        for digit in range(10):
            glyph_data, char_height, char_width = font.get_ch(chr(ord('0') + digit))
            glyphs[digit] = (glyph_data, char_width, char_height)
        self._digit_glyphs[id(font)] = glyphs

    def integer(self, value: int, x: int, y: int, color: int,
                bgcolor: int = 0, font=None, right_align: bool = False,
                right_x: int = None) -> int:
        """
        Render an integer with ZERO allocations.

        IMPORTANT: init_digits() MUST have been called for the font before using
        this method. Calling without initialization will raise an error.

        Args:
            value: Integer to render (0-99999, handles any positive int up to 5 digits)
            x, y: Position (left edge, or ignored if right_align=True)
            color: Foreground color (RGB565)
            bgcolor: Background color (RGB565), default black
            font: Font to use (must have been passed to init_digits())
            right_align: If True, align right edge to right_x
            right_x: Right edge x position (required if right_align=True)

        Returns:
            X position after last digit
        """
        if font is None:
            font = self._default_font
        glyphs = self._digit_glyphs.get(id(font))
        if glyphs is None:
            raise ValueError("Font not initialized - call init_digits() first")

        if value < 0:
            value = 0

        # Set up palette
        self._palette.pixel(0, 0, bgcolor)
        self._palette.pixel(1, 0, color)
        spec = self._glyph_spec

        # Extract digits (reverse order) using integer math
        # Use pre-allocated array to avoid allocation
        digits = self._int_digits
        num_digits = 0
        temp = value

        if temp == 0:
            digits[0] = 0
            num_digits = 1
        else:
            while temp > 0 and num_digits < 5:
                digits[num_digits] = temp % 10
                temp //= 10
                num_digits += 1

        # Calculate total width for right alignment
        if right_align:
            total_width = 0
            for i in range(num_digits):
                total_width += glyphs[digits[i]][1]
            cursor_x = right_x - total_width
        else:
            cursor_x = x

        # Render digits (in reverse order since we extracted backwards)
        for i in range(num_digits - 1, -1, -1):
            g = glyphs[digits[i]]
            spec[0], spec[1], spec[2] = g[0], g[1], g[2]
            self._fb.blit(spec, cursor_x, y, -1, self._palette)
            cursor_x += g[1]

        return cursor_x

    def text(self, string: str, x: int, y: int, color: int,
             bgcolor: int = 0, font=None) -> int:
        """
        Draw text at position with specified color.

        Uses zero-allocation rendering by updating a pre-allocated glyph
        spec list in-place rather than creating FrameBuffer objects.

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

        # Render each character using pre-allocated glyph spec
        cursor_x = x
        spec = self._glyph_spec
        for char in string:
            glyph_data, char_height, char_width = font.get_ch(char)

            # Update pre-allocated list in-place (ZERO ALLOCATIONS)
            spec[0] = glyph_data  # memoryview from font
            spec[1] = char_width
            spec[2] = char_height
            # spec[3] is already MONO_HLSB from init

            # Blit with list-based glyph spec
            self._fb.blit(spec, cursor_x, y, -1, self._palette)

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


# Import rgb565 from shared color module for backwards compatibility
from lib.color import rgb565

# Import font modules for convenience
from . import unscii_8
from . import unscii_16
from . import spleen_5x8

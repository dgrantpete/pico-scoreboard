"""Pure-Python stand-in for MicroPython's `framebuf` module.

The firmware draws every pixel through `framebuf.FrameBuffer`. To run the real
render code on CPython we reimplement exactly enough of the C module to be
bit-for-bit compatible with the generated sprite/font data. The normative
reference for the packed byte layouts is `tools/compile_layout.py` (its
`pack_*` functions); the oracle tests in `tests/test_framebuf_shim.py` pin the
two against each other.

Only the formats the firmware actually uses are supported:

    MONO_HLSB  fonts / QR (1 bpp, MSB = leftmost pixel)
    GS2_HMSB   count dots (2 bpp, leftmost pixel in the LOW two bits)
    GS4_HMSB   field sprite (4 bpp, even x = HIGH nibble)
    GS8        (1 byte/pixel, palette index)
    RGB565     display + logos (2 bytes/pixel, little-endian)

Constant values match MicroPython's (`MONO_HLSB == 3`, etc.) so the shim reads
identically to the real module even though the firmware only ever passes the
constant through to `FrameBuffer`.

Stride is stored in pixels but rounded up per format so every row occupies a
whole number of bytes -- MicroPython does the same rounding, which is why the
generated sprites (whose row byte counts use `ceil(width / pixels_per_byte)`)
render correctly with the default `stride == width`.

`blit()` reproduces MicroPython's semantics: it clips negative and
overflowing offsets, looks the source pixel up through the palette BEFORE
comparing against the colorkey, and accepts either a `FrameBuffer` or a bare
`(buffer, width, height, format[, stride])` tuple as the source (the font
glyph tables are such tuples).
"""

# MicroPython framebuf format constants (extmod/modframebuf.c).
MONO_VLSB = 0
RGB565 = 1
GS4_HMSB = 2
MONO_HLSB = 3
MONO_HMSB = 4
GS2_HMSB = 5
GS8 = 6

# Pixels packed per byte, per format -> stride alignment (rows are byte-whole).
_PIXELS_PER_BYTE = {
    MONO_VLSB: 8,
    MONO_HLSB: 8,
    MONO_HMSB: 8,
    GS2_HMSB: 4,
    GS4_HMSB: 2,
    GS8: 1,
    RGB565: 1,
}


def _round_stride(stride: int, fmt: int) -> int:
    ppb = _PIXELS_PER_BYTE[fmt]
    if ppb == 1:
        return stride
    return (stride + ppb - 1) & ~(ppb - 1)


class FrameBuffer:
    """Minimal MicroPython-compatible framebuffer over a writable byte buffer."""

    def __init__(self, buf, width, height, format, stride=None):
        if format not in _PIXELS_PER_BYTE:
            raise ValueError(f"unsupported framebuf format: {format}")
        self._buf = buf
        self._fb_width = width
        self._fb_height = height
        self._format = format
        self._stride = _round_stride(width if stride is None else stride, format)

    # --- geometry (plain attributes; Hub75Display / Region override as properties) ---
    @property
    def width(self) -> int:
        return self._fb_width

    @property
    def height(self) -> int:
        return self._fb_height

    # --- raw per-format pixel access (no bounds check; callers clip) ---
    def _get_raw(self, x: int, y: int) -> int:
        buf = self._buf
        stride = self._stride
        fmt = self._format
        if fmt == RGB565:
            i = (x + y * stride) * 2
            return buf[i] | (buf[i + 1] << 8)
        if fmt == GS8:
            return buf[x + y * stride]
        if fmt == GS4_HMSB:
            byte = buf[(x + y * stride) >> 1]
            return (byte & 0x0F) if (x & 1) else (byte >> 4)
        if fmt == GS2_HMSB:
            byte = buf[(x + y * stride) >> 2]
            return (byte >> ((x & 0x03) << 1)) & 0x03
        if fmt == MONO_HLSB:
            byte = buf[(x + y * stride) >> 3]
            return (byte >> (7 - (x & 0x07))) & 1
        if fmt == MONO_HMSB:
            byte = buf[(x + y * stride) >> 3]
            return (byte >> (x & 0x07)) & 1
        # MONO_VLSB
        byte = buf[(y >> 3) * stride + x]
        return (byte >> (y & 0x07)) & 1

    def _set_raw(self, x: int, y: int, col: int) -> None:
        buf = self._buf
        stride = self._stride
        fmt = self._format
        if fmt == RGB565:
            i = (x + y * stride) * 2
            buf[i] = col & 0xFF
            buf[i + 1] = (col >> 8) & 0xFF
            return
        if fmt == GS8:
            buf[x + y * stride] = col & 0xFF
            return
        if fmt == GS4_HMSB:
            idx = (x + y * stride) >> 1
            if x & 1:
                buf[idx] = (buf[idx] & 0xF0) | (col & 0x0F)
            else:
                buf[idx] = (buf[idx] & 0x0F) | ((col & 0x0F) << 4)
            return
        if fmt == GS2_HMSB:
            idx = (x + y * stride) >> 2
            sh = (x & 0x03) << 1
            buf[idx] = (buf[idx] & ~(0x03 << sh) & 0xFF) | ((col & 0x03) << sh)
            return
        if fmt == MONO_HLSB:
            idx = (x + y * stride) >> 3
            bit = 7 - (x & 0x07)
            if col:
                buf[idx] |= 1 << bit
            else:
                buf[idx] &= ~(1 << bit) & 0xFF
            return
        if fmt == MONO_HMSB:
            idx = (x + y * stride) >> 3
            bit = x & 0x07
            if col:
                buf[idx] |= 1 << bit
            else:
                buf[idx] &= ~(1 << bit) & 0xFF
            return
        # MONO_VLSB
        idx = (y >> 3) * stride + x
        bit = y & 0x07
        if col:
            buf[idx] |= 1 << bit
        else:
            buf[idx] &= ~(1 << bit) & 0xFF

    # --- public drawing API ---
    def pixel(self, x, y, col=None):
        """Get (col omitted) or set a pixel. Out-of-bounds get returns None."""
        if not (0 <= x < self._fb_width and 0 <= y < self._fb_height):
            return None
        if col is None:
            return self._get_raw(x, y)
        self._set_raw(x, y, col)
        return None

    def fill(self, col) -> None:
        self.fill_rect(0, 0, self._fb_width, self._fb_height, col)

    def fill_rect(self, x, y, w, h, col) -> None:
        if w <= 0 or h <= 0:
            return
        x0 = x if x > 0 else 0
        y0 = y if y > 0 else 0
        x1 = x + w
        y1 = y + h
        if x1 > self._fb_width:
            x1 = self._fb_width
        if y1 > self._fb_height:
            y1 = self._fb_height
        for yy in range(y0, y1):
            for xx in range(x0, x1):
                self._set_raw(xx, yy, col)

    def rect(self, x, y, w, h, col, fill=False) -> None:
        if fill:
            self.fill_rect(x, y, w, h, col)
            return
        if w <= 0 or h <= 0:
            return
        self.hline(x, y, w, col)
        self.hline(x, y + h - 1, w, col)
        self.vline(x, y, h, col)
        self.vline(x + w - 1, y, h, col)

    def hline(self, x, y, w, col) -> None:
        self.fill_rect(x, y, w, 1, col)

    def vline(self, x, y, h, col) -> None:
        self.fill_rect(x, y, 1, h, col)

    def blit(self, source, x, y, key=-1, palette=None) -> None:
        """Copy `source` onto this buffer at (x, y).

        Matches MicroPython: per-pixel bounds clipping (negative offsets
        included), palette lookup applied BEFORE the colorkey comparison, and
        a tuple `(buf, w, h, format[, stride])` accepted as the source.
        """
        src = source if isinstance(source, FrameBuffer) else _coerce_source(source)
        sw = src._fb_width
        sh = src._fb_height
        dw = self._fb_width
        dh = self._fb_height
        for cy in range(sh):
            dy = y + cy
            if dy < 0 or dy >= dh:
                continue
            for cx in range(sw):
                dx = x + cx
                if dx < 0 or dx >= dw:
                    continue
                col = src._get_raw(cx, cy)
                if palette is not None:
                    col = palette._get_raw(col, 0)
                if col != key:
                    self._set_raw(dx, dy, col)

    # --- unimplemented API: fail loudly so a missing feature is obvious ---
    def line(self, *a, **k):
        raise NotImplementedError(
            "framebuf shim: line() is unimplemented; add it if a renderer needs it"
        )

    def ellipse(self, *a, **k):
        raise NotImplementedError(
            "framebuf shim: ellipse() is unimplemented; add it if a renderer needs it"
        )

    def text(self, *a, **k):
        raise NotImplementedError(
            "framebuf shim: text() is unimplemented; the firmware renders text "
            "via scoreboard.fonts.FontWriter, not framebuf.text()"
        )

    def scroll(self, *a, **k):
        raise NotImplementedError(
            "framebuf shim: scroll() is unimplemented; add it if a renderer needs it"
        )

    def poly(self, *a, **k):
        raise NotImplementedError(
            "framebuf shim: poly() is unimplemented; add it if a renderer needs it"
        )


def _coerce_source(source) -> FrameBuffer:
    """Wrap a `(buf, w, h, format[, stride])` tuple as a FrameBuffer."""
    if len(source) == 4:
        buf, w, h, fmt = source
        return FrameBuffer(buf, w, h, fmt)
    if len(source) == 5:
        buf, w, h, fmt, stride = source
        return FrameBuffer(buf, w, h, fmt, stride)
    raise TypeError(
        f"framebuf shim: blit source must be a FrameBuffer or a 4/5-tuple, got {source!r}"
    )

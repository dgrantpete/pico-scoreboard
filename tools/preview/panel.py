"""Turn raw RGB565 frame buffers into PNGs/GIFs, optionally with an LED look.

RGB565 -> RGB888 goes through a 65536-entry lookup table that expands each 5/6-
bit channel by MSB replication (`r8 = (r5 << 3) | (r5 >> 2)`), exactly what the
driver's `load_rgb565` does. No gamma is applied: the panel's own LUT is
mirrored by the monitor's sRGB decode, so correcting here would double-apply and
wash the preview out. The flat 128x64 PNG is therefore the ground truth.

The optional "LED look" nearest-neighbour upscales by `scale` and multiplies in
a tiled, antialiased dot mask (a soft circle per cell) so the preview reads like
lit pixels on a dark panel rather than flat blocks.
"""

from PIL import Image, ImageChops, ImageDraw

PANEL_W = 128
PANEL_H = 64
DEFAULT_SCALE = 8

_LUT = None            # 65536*3 bytes: RGB565 value -> RGB888
_DOT_MASKS: dict = {}  # scale -> full-size tiled L-mode mask


def _rgb565_lut() -> bytes:
    global _LUT
    if _LUT is not None:
        return _LUT
    lut = bytearray(65536 * 3)
    for v in range(65536):
        r5 = (v >> 11) & 0x1F
        g6 = (v >> 5) & 0x3F
        b5 = v & 0x1F
        j = v * 3
        lut[j] = (r5 << 3) | (r5 >> 2)
        lut[j + 1] = (g6 << 2) | (g6 >> 4)
        lut[j + 2] = (b5 << 3) | (b5 >> 2)
    _LUT = bytes(lut)
    return _LUT


def buffer_to_image(buf: bytes, width: int = PANEL_W, height: int = PANEL_H) -> Image.Image:
    """Decode a little-endian RGB565 buffer into a native-resolution RGB image."""
    lut = _rgb565_lut()
    out = bytearray(width * height * 3)
    for i in range(width * height):
        v = buf[2 * i] | (buf[2 * i + 1] << 8)
        j = v * 3
        k = i * 3
        out[k] = lut[j]
        out[k + 1] = lut[j + 1]
        out[k + 2] = lut[j + 2]
    return Image.frombytes("RGB", (width, height), bytes(out))


def _dot_cell(scale: int) -> Image.Image:
    """One antialiased dot on a `scale`x`scale` cell (4x supersampled)."""
    ss = 4
    dot_frac = 6.5 / 8.0
    diameter = dot_frac * scale
    inset = (scale - diameter) / 2.0 * ss
    big = Image.new("L", (scale * ss, scale * ss), 0)
    draw = ImageDraw.Draw(big)
    draw.ellipse(
        [inset, inset, scale * ss - inset, scale * ss - inset], fill=255
    )
    return big.resize((scale, scale), Image.LANCZOS)


def _dot_mask(scale: int, width: int, height: int) -> Image.Image:
    key = (scale, width, height)
    cached = _DOT_MASKS.get(key)
    if cached is not None:
        return cached
    cell = _dot_cell(scale)
    # Tile a single row strip, then stack strips -- cheaper than a full grid.
    strip = Image.new("L", (width, scale), 0)
    for x in range(0, width, scale):
        strip.paste(cell, (x, 0))
    mask = Image.new("L", (width, height), 0)
    for y in range(0, height, scale):
        mask.paste(strip, (0, y))
    _DOT_MASKS[key] = mask
    return mask


def led_image(rgb_image: Image.Image, scale: int = DEFAULT_SCALE) -> Image.Image:
    """Upscale nearest-neighbour and multiply in the tiled dot mask."""
    w, h = rgb_image.size
    big = rgb_image.resize((w * scale, h * scale), Image.NEAREST)
    mask = _dot_mask(scale, w * scale, h * scale).convert("RGB")
    return ImageChops.multiply(big, mask)


def nearest_upscale(image: Image.Image, scale: int) -> Image.Image:
    """Blocky nearest-neighbour upscale (no dot mask) -- the flat look, enlarged."""
    w, h = image.size
    return image.resize((w * scale, h * scale), Image.NEAREST)


def save_png(image: Image.Image, path) -> None:
    image.save(path, "PNG")


def save_gif(images: "list[Image.Image]", path, duration: int = 50) -> None:
    first = images[0]
    first.save(
        path, "GIF", save_all=True, append_images=images[1:],
        duration=duration, loop=0, disposal=1, optimize=False,
    )

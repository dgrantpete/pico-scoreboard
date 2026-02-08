"""Shared color conversion utilities."""
import micropython
import framebuf


# sRGB to linear lookup table for LED matrix correction.
# Images are typically sRGB-encoded. LED matrices respond linearly to PWM.
# The sRGB transfer function uses a linear segment for dark values (preserving
# shadow detail) and a power curve (~2.4) for brighter values.
# Formula: if x <= 0.04045: x/12.92, else: ((x+0.055)/1.055)^2.4
GAMMA_LUT = bytes([
    0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3,
    4, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 7, 7, 7,
    8, 8, 8, 8, 9, 9, 9, 10, 10, 10, 11, 11, 12, 12, 12, 13,
    13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 17, 18, 18, 19, 19, 20,
    20, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 27, 27, 28, 29, 29,
    30, 30, 31, 32, 32, 33, 34, 35, 35, 36, 37, 37, 38, 39, 40, 41,
    41, 42, 43, 44, 45, 45, 46, 47, 48, 49, 50, 51, 51, 52, 53, 54,
    55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70,
    71, 72, 73, 74, 76, 77, 78, 79, 80, 81, 82, 84, 85, 86, 87, 88,
    90, 91, 92, 93, 95, 96, 97, 99, 100, 101, 103, 104, 105, 107, 108, 109,
    111, 112, 114, 115, 116, 118, 119, 121, 122, 124, 125, 127, 128, 130, 131, 133,
    134, 136, 138, 139, 141, 142, 144, 146, 147, 149, 151, 152, 154, 156, 157, 159,
    161, 163, 164, 166, 168, 170, 171, 173, 175, 177, 179, 181, 183, 184, 186, 188,
    190, 192, 194, 196, 198, 200, 202, 204, 206, 208, 210, 212, 214, 216, 218, 220,
    222, 224, 226, 229, 231, 233, 235, 237, 239, 242, 244, 246, 248, 250, 253, 255,
])

# Bayer 4x4 ordered dither matrix. Values are 0-15; subtract 8 to get signed
# offsets (-8 to +7). This spreads quantization error spatially to reduce
# banding in dark regions where gamma correction crushes the value range.
BAYER_4X4 = bytes([
    0, 8, 2, 10,
    12, 4, 14, 6,
    3, 11, 1, 9,
    15, 7, 13, 5,
])


@micropython.viper
def rgb565(r: int, g: int, b: int) -> int:
    """Convert RGB888 to RGB565 color value."""
    return int(((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3))


@micropython.viper
def _convert_rgb888_to_rgb565_viper(src_in, dst_in, gamma_in, bayer_in,
                                     pixel_count: int, width: int):
    """Viper-optimized RGB888 to RGB565 conversion with gamma and ordered dithering."""
    src = ptr8(src_in)
    dst = ptr16(dst_in)
    gamma = ptr8(gamma_in)
    bayer = ptr8(bayer_in)

    i: int = 0
    src_idx: int = 0
    x: int = 0
    y: int = 0

    while i < pixel_count:
        # Get Bayer threshold for this pixel position (-8 to +7 range)
        dither: int = bayer[((y & 3) << 2) | (x & 3)] - 8

        # Read RGB888, apply gamma, then add dither offset
        r: int = gamma[src[src_idx]] + dither
        g: int = gamma[src[src_idx + 1]] + dither
        b: int = gamma[src[src_idx + 2]] + dither

        # Clamp to valid range
        if r < 0:
            r = 0
        elif r > 255:
            r = 255
        if g < 0:
            g = 0
        elif g > 255:
            g = 255
        if b < 0:
            b = 0
        elif b > 255:
            b = 255

        # Convert to RGB565
        dst[i] = ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)

        # Advance position
        x += 1
        if x >= width:
            x = 0
            y += 1
        i += 1
        src_idx += 3


def rgb888_to_rgb565_into_buffer(rgb888_data, output_buffer: bytearray, width: int, height: int):
    """
    Convert RGB888 data to RGB565 format with gamma correction into a pre-allocated buffer.

    Args:
        rgb888_data: Source RGB888 data (bytes or memoryview, 3 bytes per pixel)
        output_buffer: Pre-allocated bytearray for RGB565 output (2 bytes per pixel)
        width: Image width in pixels
        height: Image height in pixels

    Returns:
        framebuf.FrameBuffer wrapping the output buffer in RGB565 format
    """
    pixel_count = width * height
    _convert_rgb888_to_rgb565_viper(rgb888_data, output_buffer, GAMMA_LUT, BAYER_4X4,
                                     pixel_count, width)
    return framebuf.FrameBuffer(output_buffer, width, height, framebuf.RGB565)

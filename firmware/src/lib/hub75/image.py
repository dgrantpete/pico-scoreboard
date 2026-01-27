import micropython
import framebuf
from lib.color import rgb565

_DIGITS = {ord('0') + digit for digit in range(10)}
_NEWLINES = {ord(newline) for newline in '\r\n'}
_WHITESPACES = {ord(whitespace) for whitespace in ' \n\r\t\v\f'}

_BITMAP_FORMATS = {'P4'}
_GREYSCALE_FORMATS = {'P5'}
_COLOR_FORMATS = {'P6'}

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

class PPMImage:
    def __init__(self, *, magic_number, width, height, max_value, image_data):
        self._magic_number = magic_number
        self._width = width
        self._height = height
        self._max_value = max_value
        self._image_data = image_data

    @property
    @micropython.native
    def magic_number(self) -> str:
        return self._magic_number
    
    @property
    @micropython.native
    def width(self) -> int:
        return self._width
    
    @property
    @micropython.native
    def height(self) -> int:
        return self._height
    
    @property
    @micropython.native
    def max_value(self) -> int | None:
        return self._max_value
    
    @property
    @micropython.native
    def image_data(self) -> memoryview:
        return self._image_data
    
    @classmethod
    @micropython.native
    def from_file(cls, file) -> PPMImage:
        cursor = BufferCursor(file)

        magic_number = cls._parse_magic_number(cursor)

        cls._skip_trivia(cursor)
        width = cls._parse_number(cursor)

        cls._skip_trivia(cursor)
        height = cls._parse_number(cursor)

        image_size = width * height

        if magic_number in _BITMAP_FORMATS:
            max_value = None
            expected_byte_count = -(-image_size // 8)
        elif magic_number in _GREYSCALE_FORMATS:
            cls._skip_trivia(cursor)
            max_value = cls._parse_number(cursor)
            expected_byte_count = (2 if max_value >= 256 else 1) * image_size
        elif magic_number in _COLOR_FORMATS:
            cls._skip_trivia(cursor)
            max_value = cls._parse_number(cursor)
            expected_byte_count = (2 if max_value >= 256 else 1) * image_size * 3
        else:
            raise ValueError(f"Unsupported PPM format: {magic_number!r}")

        # Only a single whitespace character should be skipped per the specification
        cls._skip_whitespace(cursor)

        image_data = cursor.read(expected_byte_count)

        if len(image_data) != expected_byte_count:
            raise ValueError(f"PPM image data is incomplete: expected {expected_byte_count} bytes, got {len(image_data)} bytes")

        return cls(
            magic_number=magic_number,
            width=width,
            height=height,
            max_value=max_value,
            image_data=image_data
        )
    
    @classmethod
    @micropython.native
    def _parse_magic_number(cls, cursor: BufferCursor) -> str:
        if cursor.current != ord('P'):
            raise ValueError(cursor.format_error_message("magic number must begin with 'P'"))
        
        cursor.require_next("parsing magic number")

        number = cls._parse_number(cursor)

        return f"P{number}"
    
    @staticmethod
    @micropython.native
    def _parse_number(cursor: BufferCursor) -> int:
        if not cursor.current in _DIGITS:
            raise ValueError(cursor.format_error_message("expected digit while parsing number"))
        
        number = cursor.current - ord('0')

        while cursor.next() and cursor.current in _DIGITS:
            number = (number * 10) + (cursor.current - ord('0'))

        return number

    @classmethod
    @micropython.native
    def _skip_trivia(cls, cursor: BufferCursor):
        trivia_skipped = False

        while True:
            if cursor.current == ord('#'):
                cls._skip_comment(cursor)
                trivia_skipped = True
                continue

            if cursor.current in _WHITESPACES:
                cls._skip_whitespace(cursor)
                trivia_skipped = True
                continue

            break

        if not trivia_skipped:
            raise ValueError("expected trivia to skip (comments or whitespace)")

    @classmethod
    @micropython.native
    def _skip_comment(cls, cursor: BufferCursor):
        if not cursor.current == ord('#'):
            raise ValueError(cursor.format_error_message("expected comment to skip"))

        while not cursor.current in _NEWLINES:
            cursor.require_next("skipping comment")
        
        cls._skip_newline(cursor)

    @classmethod
    @micropython.native
    def _skip_whitespace(cls, cursor: BufferCursor):
        if not cursor.current in _WHITESPACES:
            raise ValueError(cursor.format_error_message("expected whitespace to skip"))

        if cursor.current in _NEWLINES:
            cls._skip_newline(cursor)
            return
        
        cursor.require_next("skipping whitespace")

    @staticmethod
    @micropython.native
    def _skip_newline(cursor: BufferCursor):
        first = cursor.current

        if not first in _NEWLINES:
            raise ValueError(cursor.format_error_message("expected newline to skip"))

        cursor.require_next("skipping newline")

        # Handling case with '\r\n' (Windows newline character)
        if first == ord('\r') and cursor.current == ord('\n'):
            cursor.require_next("skipping newline")

class BufferCursor:
    def __init__(self, buffer):
        self._buffer = buffer
        self._position = 0

    @micropython.native
    def next(self):
        if self._position >= len(self._buffer):
            return False
        
        self._position += 1
        return True
    
    @micropython.native
    def require_next(self, description: str | None = None):
        if not self.next():
            raise IndexError(self.format_error_message("unexpected EOF" if description is None else f"unexpected EOF while {description}"))

        return self.current

    def format_error_message(self, description: str):
        try:
            character_message = f", Character: {chr(self.current)!r}"
        except IndexError:
            character_message = ""

        return f"Error while parsing: {description}{character_message}, Position: {self.position}"

    @micropython.native
    def read(self, byte_count: int | None = None):
        if byte_count is None:
            byte_count = len(self._buffer) - self._position

        try:
            data = self._buffer[self._position:self._position + byte_count]

            self._position += byte_count
            return data
        except IndexError:
            raise IndexError(self.format_error_message("attempted to read past end of buffer"))
    
    @property
    @micropython.native
    def current(self) -> int:
        try:
            return self._buffer[self._position]
        except IndexError:
            raise IndexError(self.format_error_message("attempted to read past end of buffer"))
    
    @property
    @micropython.native
    def position(self):
        return self._position


@micropython.native
def ppm_to_rgb565_framebuffer(ppm_bytes):
    """
    Convert PPM P6 bytes to an RGB565 framebuffer for blitting.

    Args:
        ppm_bytes: Raw bytes of a PPM P6 image

    Returns:
        Tuple of (framebuf.FrameBuffer, width, height)

    Raises:
        ValueError: If the PPM format is not P6
    """
    ppm = PPMImage.from_file(ppm_bytes)

    if ppm.magic_number != "P6":
        raise ValueError(f"Expected P6 format, got {ppm.magic_number}")

    width = ppm.width
    height = ppm.height
    image_data = ppm.image_data  # memoryview of RGB888 bytes

    # Allocate RGB565 buffer (2 bytes per pixel)
    buffer = bytearray(width * height * 2)

    # Convert each pixel from RGB888 to RGB565 with gamma correction
    pixel_count = width * height
    for i in range(pixel_count):
        src_offset = i * 3
        # Apply gamma correction for LED matrix display
        r = GAMMA_LUT[image_data[src_offset]]
        g = GAMMA_LUT[image_data[src_offset + 1]]
        b = GAMMA_LUT[image_data[src_offset + 2]]

        # Convert to RGB565
        color = rgb565(r, g, b)

        # Write little-endian (MicroPython framebuf uses LE)
        dst_offset = i * 2
        buffer[dst_offset] = color & 0xFF
        buffer[dst_offset + 1] = (color >> 8) & 0xFF

    # Create framebuffer
    fb = framebuf.FrameBuffer(buffer, width, height, framebuf.RGB565)
    return fb, width, height


@micropython.native
def ppm_to_rgb565_into_buffer(ppm_bytes, output_buffer):
    """
    Convert PPM P6 bytes to RGB565, writing into an existing buffer.

    This avoids allocating a new buffer, reducing memory fragmentation.

    Args:
        ppm_bytes: Raw bytes of a PPM P6 image
        output_buffer: Pre-allocated bytearray (must be width*height*2 bytes)

    Returns:
        framebuf.FrameBuffer wrapping the output_buffer

    Raises:
        ValueError: If the PPM format is not P6
    """
    ppm = PPMImage.from_file(ppm_bytes)

    if ppm.magic_number != "P6":
        raise ValueError(f"Expected P6 format, got {ppm.magic_number}")

    width = ppm.width
    height = ppm.height
    image_data = ppm.image_data  # memoryview of RGB888 bytes

    # Write directly into the provided buffer (no allocation!)
    pixel_count = width * height
    for i in range(pixel_count):
        src_offset = i * 3
        # Apply gamma correction for LED matrix display
        r = GAMMA_LUT[image_data[src_offset]]
        g = GAMMA_LUT[image_data[src_offset + 1]]
        b = GAMMA_LUT[image_data[src_offset + 2]]

        # Convert to RGB565
        color = rgb565(r, g, b)

        # Write little-endian (MicroPython framebuf uses LE)
        dst_offset = i * 2
        output_buffer[dst_offset] = color & 0xFF
        output_buffer[dst_offset + 1] = (color >> 8) & 0xFF

    return framebuf.FrameBuffer(output_buffer, width, height, framebuf.RGB565)


@micropython.native
def rgb888_to_rgb565_into_buffer(rgb888_bytes, output_buffer, width, height):
    """
    Convert raw RGB888 bytes to RGB565 with gamma correction, writing into a buffer.

    This is faster than PPM parsing since there's no header to parse - just
    raw pixel data. Gamma correction is applied for proper LED matrix display.

    Args:
        rgb888_bytes: Raw RGB888 bytes (3 bytes per pixel: R, G, B)
        output_buffer: Pre-allocated bytearray (must be width*height*2 bytes)
        width: Image width in pixels
        height: Image height in pixels

    Returns:
        framebuf.FrameBuffer wrapping the output_buffer
    """
    pixel_count = width * height
    expected_size = pixel_count * 3
    if len(rgb888_bytes) != expected_size:
        raise ValueError(f"Expected {expected_size} bytes, got {len(rgb888_bytes)}")

    # Convert each pixel from RGB888 to RGB565 with gamma correction
    for i in range(pixel_count):
        src_offset = i * 3
        # Apply gamma correction for LED matrix display
        r = GAMMA_LUT[rgb888_bytes[src_offset]]
        g = GAMMA_LUT[rgb888_bytes[src_offset + 1]]
        b = GAMMA_LUT[rgb888_bytes[src_offset + 2]]

        # Convert to RGB565
        color = rgb565(r, g, b)

        # Write little-endian (MicroPython framebuf uses LE)
        dst_offset = i * 2
        output_buffer[dst_offset] = color & 0xFF
        output_buffer[dst_offset + 1] = (color >> 8) & 0xFF

    return framebuf.FrameBuffer(output_buffer, width, height, framebuf.RGB565)

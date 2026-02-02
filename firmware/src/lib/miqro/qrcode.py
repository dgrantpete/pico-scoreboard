import micropython
import framebuf
from .constants import ECC_LOW, VERSION_MIN, VERSION_MAX, MASK_AUTO
from .native import (
    generate_qr,
    generate_qr_binary,
    get_size,
    get_module,
    get_version,
    buffer_size_for_version
)


class QRCode:
    """
    QR Code generator with framebuffer integration.

    Example:
        from miqro import QRCode

        qr = QRCode("https://example.com")
        print(f"Version: {qr.version}, Size: {qr.width}x{qr.width}")

        # Blit to display using standard framebuf API
        display.blit(qr.data, x=10, y=10)
    """

    # Class-level buffer pool for memory efficiency
    # These are shared across all instances to avoid repeated allocations
    _temp_buffer: bytearray | None = None
    _qr_buffer: bytearray | None = None
    _buffer_version: int = 0

    @classmethod
    def _ensure_buffers(cls, version: int) -> tuple[bytearray, bytearray]:
        """Lazily allocate or reallocate buffers as needed."""
        if cls._buffer_version < version:
            size = buffer_size_for_version(version)
            cls._temp_buffer = bytearray(size)
            cls._qr_buffer = bytearray(size)
            cls._buffer_version = version
        return cls._temp_buffer, cls._qr_buffer  # type: ignore

    @micropython.native
    def __init__(
        self,
        data: str | bytes,
        ecc: int = ECC_LOW,
        version: int | None = None,
        mask: int = MASK_AUTO,
        boost_ecl: bool = True
    ):
        """
        Create a QR code from text or binary data.

        Args:
            data: Text string or bytes to encode
            ecc: Error correction level (ECC_LOW, ECC_MEDIUM, ECC_QUARTILE, ECC_HIGH)
            version: QR version (None for auto, 1-40 for exact)
            mask: Mask pattern (MASK_AUTO or 0-7)
            boost_ecl: Boost error correction if space allows
        """
        # Determine max version for buffer allocation
        if version is None:
            # Auto-select: allocate for version 10 by default (covers most use cases)
            # If that fails, we'll try with a larger buffer
            max_version = 10
            exact_version = -1  # Signal auto to C code
        else:
            if version < VERSION_MIN or version > VERSION_MAX:
                raise ValueError(f"Version must be {VERSION_MIN}-{VERSION_MAX}")
            max_version = version
            exact_version = version

        # Get shared buffers
        temp_buffer, qr_buffer = self._ensure_buffers(max_version)

        # Generate QR code
        if isinstance(data, str):
            size = generate_qr(
                data, temp_buffer, qr_buffer,
                ecc, exact_version, mask, boost_ecl
            )
        else:
            size = generate_qr_binary(
                data, temp_buffer, qr_buffer,
                ecc, exact_version, mask, boost_ecl
            )

        # If auto-select failed with version 10, try with larger buffer
        if size == 0 and version is None and max_version < VERSION_MAX:
            max_version = VERSION_MAX
            temp_buffer, qr_buffer = self._ensure_buffers(max_version)

            if isinstance(data, str):
                size = generate_qr(
                    data, temp_buffer, qr_buffer,
                    ecc, exact_version, mask, boost_ecl
                )
            else:
                size = generate_qr_binary(
                    data, temp_buffer, qr_buffer,
                    ecc, exact_version, mask, boost_ecl
                )

        if size == 0:
            raise ValueError("Failed to generate QR code - data too large or invalid")

        self._size = size

        # Copy QR data to instance buffer (allows multiple QRCode instances)
        actual_version = (size - 17) // 4
        copy_size = buffer_size_for_version(actual_version)
        self._qr_data = bytearray(qr_buffer[:copy_size])

        # Create the framebuffer representation
        self._create_framebuffer()

    @micropython.native
    def _create_framebuffer(self) -> None:
        """Create the MONO_HLSB framebuffer from QR data."""
        size = self._size
        # MONO_HLSB: 8 pixels per byte, MSB first, rows padded to byte boundary
        row_bytes = (size + 7) // 8
        self._fb_buffer = bytearray(row_bytes * size)
        self._framebuffer = framebuf.FrameBuffer(
            self._fb_buffer,
            size,
            size,
            framebuf.MONO_HLSB
        )

        # Fill framebuffer from QR data
        for y in range(size):
            for x in range(size):
                if get_module(self._qr_data, x, y):
                    self._framebuffer.pixel(x, y, 1)

    @property
    @micropython.native
    def width(self) -> int:
        """Width of the QR code in modules (pixels)."""
        return self._size

    @property
    @micropython.native
    def height(self) -> int:
        """Height of the QR code in modules (same as width)."""
        return self._size

    @property
    @micropython.native
    def version(self) -> int:
        """QR version number (1-40)."""
        return (self._size - 17) // 4

    @property
    def data(self) -> framebuf.FrameBuffer:
        """
        FrameBuffer containing the QR code in MONO_HLSB format.

        This can be blitted directly to displays:
            display.blit(qr.data, x, y)

        Or with a color palette:
            palette = framebuf.FrameBuffer(bytearray(4), 2, 1, framebuf.RGB565)
            palette.pixel(0, 0, 0x0000)  # Background: black
            palette.pixel(1, 0, 0xFFFF)  # Foreground: white
            display.blit(qr.data, x, y, -1, palette)
        """
        return self._framebuffer

    @micropython.native
    def get(self, x: int, y: int) -> bool:
        """
        Get the module (pixel) value at coordinates.

        Args:
            x: X coordinate (0 to width-1)
            y: Y coordinate (0 to height-1)

        Returns:
            True if the module is dark, False if light
        """
        return get_module(self._qr_data, x, y)

    @micropython.native
    def __getitem__(self, coords: tuple[int, int]) -> bool:
        """Allow qr[x, y] syntax."""
        return self.get(coords[0], coords[1])

    def packed(self) -> tuple[int, int, bytes]:
        """
        Get packed binary representation.

        Returns:
            Tuple of (width, height, packed_data) where packed_data has
            8 pixels per byte, MSB first, with each row padded to byte boundary.
        """
        return (self._size, self._size, bytes(self._fb_buffer))

    def print_ascii(self) -> None:
        """Print the QR code to console using block characters."""
        size = self._size

        # Print top quiet zone
        print(' ' * (size + 2))

        for y in range(0, size, 2):
            line = [' ']  # Left quiet zone
            for x in range(size):
                top = self.get(x, y)
                bot = self.get(x, y + 1) if y + 1 < size else False

                if top and bot:
                    line.append('\u2588')  # Full block
                elif top:
                    line.append('\u2580')  # Upper half
                elif bot:
                    line.append('\u2584')  # Lower half
                else:
                    line.append(' ')
            line.append(' ')  # Right quiet zone
            print(''.join(line))

        # Print bottom quiet zone if needed
        if size % 2 == 1:
            print(' ' * (size + 2))

    def __repr__(self) -> str:
        return f"QRCode(version={self.version}, size={self._size}x{self._size})"

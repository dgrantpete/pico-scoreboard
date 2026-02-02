"""
Dynamic WiFi QR code generation for setup screen.

Generates QR codes encoding WiFi credentials so users can
scan to connect their phone to the scoreboard's AP network.

MUST be called on Core 0 (allocates memory during generation).
"""

import framebuf
from lib.miqro import QRCode


# Pre-allocated palette buffer (4 bytes for 2 RGB565 colors)
# This is allocated once at module load time
_palette_buf = bytearray(4)
_palette = framebuf.FrameBuffer(_palette_buf, 2, 1, framebuf.RGB565)
_palette.pixel(0, 0, 0xFFFF)  # Index 0: white (QR background/light modules)
_palette.pixel(1, 0, 0x0000)  # Index 1: black (QR dark modules)


def generate_wifi_qr(ssid: str, password: str = '') -> tuple:
    """
    Generate a QR code encoding WiFi credentials.

    The QR code uses the standard WiFi QR format that mobile devices
    recognize and can use to auto-connect to the network.

    Args:
        ssid: Network name (the AP SSID, e.g., "scoreboard")
        password: Network password (empty string for open network)

    Returns:
        Tuple of (framebuffer, width, height, palette):
        - framebuffer: MONO_HLSB FrameBuffer containing QR code
        - width: QR code width in pixels
        - height: QR code height in pixels
        - palette: RGB565 palette for display blitting

    Example:
        qr_fb, qr_w, qr_h, palette = generate_wifi_qr("scoreboard")
        display.blit(qr_fb, x, y, -1, palette)
    """
    # WiFi QR code format: WIFI:T:<security>;S:<ssid>;P:<password>;;
    # T = authentication type: nopass, WPA, WEP
    # S = SSID (network name)
    # P = password (can be empty for open networks)
    if password:
        wifi_str = f"WIFI:T:WPA;S:{ssid};P:{password};;"
    else:
        wifi_str = f"WIFI:T:nopass;S:{ssid};;"

    qr = QRCode(wifi_str)

    return (qr.data, qr.width, qr.height, _palette)

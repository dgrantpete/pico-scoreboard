"""Small module stand-ins + the pure-Python HSV packer.

`micropython`  - `const`, `native`, `viper` are all identity decorators. The
                 only viper function on the render path is `fonts.rgb565`
                 (plain integer bit-twiddling), so identity is exact.
`machine`      - a no-op `Pin` (display.py imports it; the preview drives no IO).
`uasyncio`     - aliased to the stdlib `asyncio` (api_client imports it at
                 module load; the preview never runs the event loop).
`aiohttp`      - empty module (imported by api_client at load time, unused here).

`pack_hsv_to_rgb565` reproduces the native helper's contract from
`firmware/src/lib/hub75/native/__init__.pyi`. Saturation 0 is grayscale and
exact by construction (`r == g == b == value`), matching how the critical-count
dots pack `pack_hsv_to_rgb565(0, 0, v)`; the red-tint path uses s > 0 and is
verified against the real `.mpy` on device.
"""

import asyncio
import sys
from types import ModuleType


def _identity(fn):
    return fn


def _const(value):
    return value


def _hsv_to_rgb(h: int, s: int, v: int):
    """HSV (each 0..255) -> (r, g, b) 0..255. s == 0 returns (v, v, v) exactly."""
    if s == 0:
        return v, v, v
    h6 = (h / 255.0) * 6.0
    i = int(h6) % 6
    f = h6 - int(h6)
    sf = s / 255.0
    p = int(round(v * (1.0 - sf)))
    q = int(round(v * (1.0 - sf * f)))
    t = int(round(v * (1.0 - sf * (1.0 - f))))
    if i == 0:
        return v, t, p
    if i == 1:
        return q, v, p
    if i == 2:
        return p, v, t
    if i == 3:
        return p, q, v
    if i == 4:
        return t, p, v
    return v, p, q


def pack_hsv_to_rgb565(hue: int, saturation: int, value: int) -> int:
    """Pack an HSV triple (each 0..255) into little-endian RGB565."""
    r, g, b = _hsv_to_rgb(hue, saturation, value)
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def _make_micropython() -> ModuleType:
    mod = ModuleType("micropython")
    mod.const = _const
    mod.native = _identity
    mod.viper = _identity
    return mod


def _make_machine() -> ModuleType:
    mod = ModuleType("machine")

    class Pin:
        OUT = 0
        IN = 1

        def __init__(self, *args, **kwargs):
            pass

        def value(self, *args):
            return 0

        def on(self):
            pass

        def off(self):
            pass

    mod.Pin = Pin
    return mod


def _make_aiohttp() -> ModuleType:
    mod = ModuleType("aiohttp")

    class ClientSession:  # pragma: no cover - never instantiated in preview
        def __init__(self, *args, **kwargs):
            raise RuntimeError("aiohttp is stubbed out in the preview environment")

    mod.ClientSession = ClientSession
    return mod


def install() -> None:
    """Register the misc shims in sys.modules (idempotent per process)."""
    sys.modules["micropython"] = _make_micropython()
    sys.modules["machine"] = _make_machine()
    sys.modules["aiohttp"] = _make_aiohttp()
    sys.modules["uasyncio"] = asyncio

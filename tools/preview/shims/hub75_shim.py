"""Fake `hub75` package for the preview.

The firmware imports a handful of names from the real HUB75 driver:

    from hub75 import Hub75Driver, Hub75Display, row_addressing, gamma
    from hub75.native import pack_hsv_to_rgb565

None of the driver's PIO/DMA machinery can run on a PC, so this builds a
stand-in `hub75` package that provides exactly those names:

  * `Hub75Driver`   - inert placeholder (state.py keeps a reference; the
                      preview never calls its hardware methods).
  * `Hub75Display`  - `PreviewDisplay`, an RGB565 framebuffer with the
                      `.buffer` / `.width` / `.height` / `show()` surface the
                      render code and `Region` expect.
  * `gamma`         - the REAL `firmware/src/lib/hub75/gamma.py`, loaded by
                      path (state.py does `isinstance(g, gamma.Power)`).
  * `row_addressing`- empty placeholder module.
  * `hub75.native`  - `pack_hsv_to_rgb565` in pure Python (see misc_shims).

`install()` registers the package and submodules in `sys.modules` so a later
`import hub75` resolves here rather than to the real driver (which is why the
preview keeps `firmware/src/lib` OFF sys.path).
"""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

from . import framebuf_shim
from .misc_shims import pack_hsv_to_rgb565


class Hub75Driver:
    """Inert placeholder. The preview never drives real hardware."""

    def __init__(self, *args, **kwargs) -> None:
        self._args = args
        self._kwargs = kwargs

    def load_rgb565(self, buffer) -> None:  # pragma: no cover - never called
        pass

    def flip(self) -> None:  # pragma: no cover - never called
        pass


class PreviewDisplay(framebuf_shim.FrameBuffer):
    """RGB565 framebuffer standing in for `Hub75Display`.

    Mirrors the real display's public surface: a `.buffer` bytearray, `.width`
    / `.height`, and a no-op `show()`. The preview reads `.buffer` directly to
    capture rendered frames instead of pushing them to a panel.
    """

    def __init__(self, width: int = 128, height: int = 64) -> None:
        self._backing = bytearray(width * height * 2)
        super().__init__(self._backing, width, height, framebuf_shim.RGB565)
        self._w = width
        self._h = height

    @property
    def buffer(self) -> bytearray:
        return self._backing

    @property
    def width(self) -> int:
        return self._w

    @property
    def height(self) -> int:
        return self._h

    def show(self) -> None:
        # Frame capture happens in the render loop; nothing to push here.
        pass


def _load_real_gamma(firmware_src: Path) -> ModuleType:
    gamma_path = firmware_src / "lib" / "hub75" / "gamma.py"
    if not gamma_path.is_file():
        raise FileNotFoundError(f"real hub75 gamma.py not found at {gamma_path}")
    spec = importlib.util.spec_from_file_location("hub75.gamma", gamma_path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def install(firmware_src: Path) -> ModuleType:
    """Register the fake `hub75` package (and submodules) in sys.modules."""
    hub75 = ModuleType("hub75")
    hub75.__path__ = []  # mark as a package so submodule imports resolve

    gamma = _load_real_gamma(firmware_src)

    row_addressing = ModuleType("hub75.row_addressing")

    native = ModuleType("hub75.native")
    native.pack_hsv_to_rgb565 = pack_hsv_to_rgb565

    hub75.Hub75Driver = Hub75Driver
    hub75.Hub75Display = PreviewDisplay
    hub75.PreviewDisplay = PreviewDisplay
    hub75.gamma = gamma
    hub75.row_addressing = row_addressing
    hub75.native = native

    sys.modules["hub75"] = hub75
    sys.modules["hub75.gamma"] = gamma
    sys.modules["hub75.row_addressing"] = row_addressing
    sys.modules["hub75.native"] = native
    return hub75

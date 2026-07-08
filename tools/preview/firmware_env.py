"""Load the real firmware render code on CPython under MicroPython shims.

`load_firmware(clock)` installs every shim the firmware needs (framebuf, time,
micropython, machine, uasyncio, aiohttp, and a fake `hub75` package), puts
`firmware/src` -- and ONLY `firmware/src` -- on `sys.path`, imports the
`scoreboard.{state, display, mlb, fonts, config}` modules for real, and returns
them bundled in a `FirmwareEnv`.

Import order matters: the shims must be in `sys.modules` before any firmware
module is imported, or the real (hardware) modules would win. The generated
`layout/` and `fonts/` modules must already exist on disk -- the preview cannot
regenerate them (that needs Aseprite / the build toolchain), so a missing
artifact is a hard, actionable error.

`miqro` is deliberately NOT stubbed: the firmware imports it lazily inside a
try/except (WiFi-setup QR generation only), so the setup scenario degrades
gracefully (no QR) rather than the whole environment failing to load.
"""

import sys
from pathlib import Path
from types import SimpleNamespace

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
FIRMWARE_SRC = REPO_ROOT / "firmware" / "src"

_LAYOUT_DIR = FIRMWARE_SRC / "scoreboard" / "layout"
_FONTS_DIR = FIRMWARE_SRC / "scoreboard" / "fonts"

_ARTIFACT_HINT = (
    "Generated layout/font modules are missing. Build them with:\n"
    "    python tools/compile_layout.py && python tools/compile_fonts.py\n"
    "(not tools/build.py -- that needs bun for the frontend)."
)


class FirmwareEnv(SimpleNamespace):
    """Bundle of imported firmware modules + the virtual clock driving them."""


def _check_artifacts() -> None:
    layout_ok = _LAYOUT_DIR.is_dir() and any(
        p.name != "__init__.py" for p in _LAYOUT_DIR.glob("*.py")
    )
    fonts_ok = _FONTS_DIR.is_dir() and any(
        p.name != "__init__.py" for p in _FONTS_DIR.glob("*.py")
    )
    if not layout_ok or not fonts_ok:
        missing = []
        if not layout_ok:
            missing.append(str(_LAYOUT_DIR.relative_to(REPO_ROOT)))
        if not fonts_ok:
            missing.append(str(_FONTS_DIR.relative_to(REPO_ROOT)))
        raise SystemExit(
            f"Missing generated modules under: {', '.join(missing)}\n{_ARTIFACT_HINT}"
        )


def _install_shims(clock):
    from .shims import framebuf_shim
    from .shims import misc_shims
    from .shims import hub75_shim
    from .shims.time_shim import make_time_module

    # framebuf: register the shim module itself under the name `framebuf`.
    sys.modules["framebuf"] = framebuf_shim
    # micropython / machine / aiohttp / uasyncio.
    misc_shims.install()
    # time: virtual-clock-backed, delegating unknown attrs to real time.
    sys.modules["time"] = make_time_module(clock)
    # hub75 package (+ native, gamma, row_addressing).
    hub75_shim.install(FIRMWARE_SRC)


def load_firmware(clock) -> FirmwareEnv:
    """Install shims and import the firmware render modules. Returns FirmwareEnv."""
    _check_artifacts()
    _install_shims(clock)

    src = str(FIRMWARE_SRC)
    if src not in sys.path:
        sys.path.insert(0, src)

    import scoreboard.config as config
    import scoreboard.fonts as fonts
    import scoreboard.mlb as mlb
    import scoreboard.state as state
    import scoreboard.display as display

    return FirmwareEnv(
        clock=clock,
        config=config,
        fonts=fonts,
        mlb=mlb,
        state=state,
        display=display,
    )

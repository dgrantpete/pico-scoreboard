#!/usr/bin/env python3
"""Generate the toast-icon PNGs (spinner, lock closed/open) for compilation.

Writes RGBA PNGs into `firmware/assets/layout/`, where compile_layout.py
picks them up as plain-PNG sources (one `__relative`-style module each).
The PNGs are committed asset art like the `.aseprite` files; this script is
their editable source — rerun it after changing the geometry below.

Spinner color contract (relied on by scoreboard/display.py): dot `k` in
angular order is painted RGB (0, 0, 8*(k+1)), whose RGB565 encoding is
exactly `k + 1`. compile_layout.py assigns palette indices in row-major
first-seen order (NOT angular order), so the firmware inverts the compiled
palette at import time — palette entry value -> angular index — instead of
hardcoding the permutation. Recoloring the dots breaks that inversion.
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print("Pillow is required: pip install Pillow", file=sys.stderr)
    sys.exit(1)

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
ASSETS_DIR = REPO_ROOT / "firmware" / "assets" / "layout"

WHITE = (255, 255, 255, 255)

# --- Spinner -----------------------------------------------------------------
# 12 3x3 dots on a ~radius-12 ring (30-degree steps), angular order with dot 0
# at 3 o'clock advancing clockwise. Top-left coords copied verbatim from the
# old display.py _SPINNER_DOTS tuple (hand-tuned rounding — do not re-derive
# from trig), shifted by the on-screen bounding-box origin (51, 20).
SPINNER_DOTS = (
    (23, 11), (21, 17), (17, 21), (12, 22), (6, 21), (2, 17),
    (0, 11), (2, 5), (6, 1), (12, 0), (17, 1), (21, 5),
)
SPINNER_DOT_PX = 3
SPINNER_SIZE = (26, 25)

# --- Lock --------------------------------------------------------------------
# Both variants share a 14x22 canvas with the body at rows 12-21 so a single
# blit position serves both and the body doesn't jump between states. The
# open shackle lifts straight up by LOCK_LIFT px: the right leg stretches to
# stay anchored in the body while the left leg keeps its length, leaving a
# gap underneath it.
LOCK_SIZE = (14, 22)
LOCK_BODY_TOP = 12       # body occupies rows 12-21 (14x10)
LOCK_LIFT = 3            # how far the open shackle rises above the closed one


def _rect(img: Image.Image, x0: int, y0: int, x1: int, y1: int, color) -> None:
    """Fill the inclusive pixel rectangle [x0..x1] x [y0..y1]."""
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            img.putpixel((x, y), color)


def gen_spinner() -> Image.Image:
    img = Image.new("RGBA", SPINNER_SIZE, (0, 0, 0, 0))
    for k, (x, y) in enumerate(SPINNER_DOTS):
        color = (0, 0, 8 * (k + 1), 255)  # RGB565 value == k + 1 (see docstring)
        _rect(img, x, y, x + SPINNER_DOT_PX - 1, y + SPINNER_DOT_PX - 1, color)
    return img


def _lock_body(img: Image.Image) -> None:
    _rect(img, 0, LOCK_BODY_TOP, 13, 21, WHITE)
    # Keyhole punched back out to transparent: one plain 2x6 rectangle (the
    # old head-gap-stem shape read as a stray horizontal line at panel size).
    _rect(img, 6, 14, 7, 19, (0, 0, 0, 0))


def gen_lock(is_open: bool) -> Image.Image:
    img = Image.new("RGBA", LOCK_SIZE, (0, 0, 0, 0))
    _lock_body(img)
    top = 3 - LOCK_LIFT if is_open else 3
    _rect(img, 2, top, 11, top + 1, WHITE)                    # shackle top bar
    _rect(img, 10, top + 2, 11, LOCK_BODY_TOP - 1, WHITE)     # right leg, anchored
    left_tip = top + 8 if is_open else LOCK_BODY_TOP - 1      # open: gap below
    _rect(img, 2, top + 2, 3, left_tip, WHITE)                # left leg
    return img


def main() -> None:
    outputs = {
        "toast_spinner.png": gen_spinner(),
        "toast_lock_closed.png": gen_lock(is_open=False),
        "toast_lock_open.png": gen_lock(is_open=True),
    }
    for name, img in outputs.items():
        path = ASSETS_DIR / name
        img.save(path)
        print(f"wrote {path.relative_to(REPO_ROOT)}")


if __name__ == "__main__":
    main()

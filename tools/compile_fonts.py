#!/usr/bin/env python3
"""Compile font source files into MicroPython modules via font_to_py.

Reads fonts from `firmware/assets/fonts/` and writes generated `.py` modules
to `firmware/src/scoreboard/fonts/`. The output directory is partly generated
(individual font modules) and partly hand-written (`__init__.py` has the
FontWriter class and helpers). Only the individual font modules are gitignored.

font_to_py is a third-party CLI script vendored at `tools/vendor/font_to_py.py`.
Invoked as a subprocess to avoid forking upstream.
"""

import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
ASSETS_DIR = REPO_ROOT / "firmware" / "assets" / "fonts"
OUTPUT_DIR = REPO_ROOT / "firmware" / "src" / "scoreboard" / "fonts"
FONT_TO_PY = SCRIPT_DIR / "vendor" / "font_to_py.py"

# (source-filename-in-assets, height, output-module-filename)
# All fonts use -x (horizontal mapping), matching the firmware FontWriter.
FONTS = [
    ("unscii_8.pcf",   8,  "unscii_8.py"),
    ("unscii_16.pcf", 16,  "unscii_16.py"),
    ("spleen-5x8.bdf", 8,  "spleen_5x8.py"),
]


def compile_all() -> None:
    if not ASSETS_DIR.is_dir():
        print(f"Error: assets directory not found: {ASSETS_DIR}", file=sys.stderr)
        sys.exit(1)
    if not FONT_TO_PY.is_file():
        print(f"Error: vendored font_to_py.py not found: {FONT_TO_PY}", file=sys.stderr)
        sys.exit(1)

    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    for src_name, height, out_name in FONTS:
        src = ASSETS_DIR / src_name
        out = OUTPUT_DIR / out_name

        if not src.is_file():
            print(f"Error: missing font source {src}", file=sys.stderr)
            sys.exit(1)

        print(f"[{out_name}] {src_name} -> {out_name} (height {height})")
        result = subprocess.run(
            [sys.executable, str(FONT_TO_PY), "-x", str(src), str(height), str(out)],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            print(f"  font_to_py failed:\n{result.stderr}", file=sys.stderr)
            sys.exit(result.returncode)

    print(f"\nDone: {len(FONTS)} font(s) written to "
          f"{OUTPUT_DIR.relative_to(REPO_ROOT).as_posix()}/")


def main():
    compile_all()


if __name__ == "__main__":
    main()

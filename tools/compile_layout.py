#!/usr/bin/env python3
"""Compile sprite/layout source art into MicroPython framebuf modules.

Reads source art from `firmware/assets/layout/` and writes generated `.py`
modules to `firmware/src/scoreboard/layout/`. The output directory is
gitignored — these modules are build artifacts, not source code. The
`.aseprite` files are the source of truth.

Source types:

1. `.aseprite` file (primary): invoked via the Aseprite CLI (`--batch
   --split-layers --list-slices --format json-hash ...`) into a temp
   directory, producing a layer-split PNG atlas plus a JSON sidecar. Each
   layer becomes a frame and each Aseprite slice becomes a coordinate
   region. Layers are filtered by suffix:

     <name>__relative   -> emit a sprite module (data + palette), no coordinates.
                           Runtime decides where to blit.
     <name>__absolute   -> emit a sprite module (data + palette) plus `X`/`Y`
                           constants for fixed-position blitting.
     <name> (no suffix) -> skip. Treated as Aseprite-only visual reference.

   Slices are always emitted as coordinate-only modules (`X`, `Y`, `WIDTH`,
   `HEIGHT` constants, no framebuf import, no pixel data). Each slice becomes
   its own file named after the slice.

2. Plain `.png` file (fallback): the entire PNG becomes one module, same
   output shape as a `__relative` layer. Used for standalone sprites not
   sourced from an Aseprite layout.

The compiler auto-selects the tightest framebuf format (MONO_HLSB, GS2_HMSB,
GS4_HMSB, GS8, or RGB565) per sprite based on total byte cost (pixel bytes
plus palette overhead). Palettes are built by scanning the image in row-major
order and assigning indices first-seen.

Transparent pixels (alpha == 0) are flattened to bright magenta (255, 0, 255)
as a transparency sentinel. When a sprite has any transparent pixels, the
compiler reserves palette index 0 for magenta (row-major first-seen scanning
starts at index 1 for the remaining colors) and emits a `KEY` constant that
is the correct value to pass as the `key` argument of `FrameBuffer.blit()`:

  - Any sprite with transparency:       KEY = 0xF81F     (RGB565 of magenta)
  - Any sprite with no transparent px:  KEY = -1         (no transparency)

The key is 0xF81F for BOTH paletted and RGB565 sprites because MicroPython's
`framebuf.blit()` applies the palette lookup BEFORE the key comparison (see
extmod/modframebuf.c) — the key is matched against the palette-MAPPED color,
never the raw palette index. Palette index 0 maps to magenta, so transparent
pixels compare as 0xF81F regardless of the packed format.

This lets the firmware use `sprite.KEY` uniformly without caring about
which format the sprite was compiled into. Note: if your source art uses
bright magenta (or any color that quantizes to RGB565 0xF81F) as a real
color, it will be treated as transparent at blit time. Pick a different
color for real art, or adjust the sentinel here.

Output filename collisions across sources (plain PNG, layer, or slice) are
hard errors — the compiler prints the conflicting sources and exits without
writing anything.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print("Pillow is required: pip install Pillow", file=sys.stderr)
    sys.exit(1)

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
ASSETS_DIR = REPO_ROOT / "firmware" / "assets" / "layout"
OUTPUT_DIR = REPO_ROOT / "firmware" / "src" / "scoreboard" / "layout"

_RELATIVE_SUFFIX = "__relative"
_ABSOLUTE_SUFFIX = "__absolute"

# Transparency sentinel: any alpha==0 pixel becomes this color before the
# palette scan. Chosen as bright magenta because no realistic sprite art
# should use it, and its RGB565 encoding (0xF81F) is distinct from common
# colors. Keep in sync with MAGENTA_RGB565 in firmware scoreboard/fonts/__init__.py.
_TRANSPARENT_RGB = (255, 0, 255)
_TRANSPARENT_RGB565 = 0xF81F


def rgb565(r: int, g: int, b: int) -> int:
    """Convert RGB888 to RGB565 (matches firmware rgb565 function)."""
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def format_bytes(data: bytes, name: str, bytes_per_line: int = 16) -> str:
    """Format raw bytes as a Python byte string literal with line continuations."""
    lines = [f"{name} =\\"]
    for offset in range(0, len(data), bytes_per_line):
        chunk = data[offset : offset + bytes_per_line]
        hex_str = "".join(f"\\x{b:02x}" for b in chunk)
        if offset + bytes_per_line < len(data):
            lines.append(f"b'{hex_str}'\\")
        else:
            lines.append(f"b'{hex_str}'")
    return "\n".join(lines)


def flatten_rgba(img: Image.Image):
    """Return (rgb_pixels, has_transparent).

    Converts the input to RGBA first so any mode (RGB, RGBA, P, L, LA, ...)
    is handled uniformly. Any pixel with alpha == 0 is replaced with the
    bright-magenta transparency sentinel. `has_transparent` is True if any
    such flattening occurred.
    """
    rgba = img.convert("RGBA")
    has_transparent = False
    result = []
    for (r, g, b, a) in rgba.getdata():
        if a > 0:
            result.append((r, g, b))
        else:
            has_transparent = True
            result.append(_TRANSPARENT_RGB)
    return result, has_transparent


def scan_palette(rgb_pixels: list, reserve_transparent: bool):
    """Build a palette in row-major first-seen order.

    If `reserve_transparent` is True, palette index 0 is pre-populated with
    the magenta sentinel and subsequent row-major first-seen colors start
    at index 1. Pixels that happen to match magenta (either from flattened
    transparency or genuine magenta in the source art) all map to index 0.

    Returns (indices, palette) where `indices` is a list of ints of length
    len(rgb_pixels) and `palette` is a list of (r, g, b) tuples in the
    order they were placed.
    """
    color_to_index = {}
    palette = []
    indices = []

    if reserve_transparent:
        palette.append(_TRANSPARENT_RGB)
        color_to_index[_TRANSPARENT_RGB] = 0

    for color in rgb_pixels:
        idx = color_to_index.get(color)
        if idx is None:
            idx = len(palette)
            color_to_index[color] = idx
            palette.append(color)
        indices.append(idx)
    return indices, palette


def pack_mono_hlsb(indices: list, width: int, height: int) -> bytes:
    """Pack indexed pixels as MONO_HLSB (1 bit/pixel, MSB=leftmost)."""
    row_bytes = (width + 7) // 8
    data = bytearray(row_bytes * height)
    for y in range(height):
        for x in range(width):
            if indices[y * width + x]:
                byte_pos = y * row_bytes + x // 8
                bit_pos = 7 - (x % 8)
                data[byte_pos] |= 1 << bit_pos
    return bytes(data)


def pack_gs2_hmsb(indices: list, width: int, height: int) -> bytes:
    """Pack indexed pixels as GS2_HMSB (2 bits/pixel).

    Four pixels per byte. Matches MicroPython's framebuf GS2_HMSB layout
    (see extmod/modframebuf.c): the leftmost pixel in each byte occupies
    the LOW two bits, with successive pixels moving toward the high end.
    So x%4==0 occupies bits 1-0, x%4==1 occupies bits 3-2, x%4==2 occupies
    bits 5-4, and x%4==3 occupies bits 7-6.

    Note this differs from GS4_HMSB, which puts the leftmost pixel in the
    HIGH nibble. The two formats have inconsistent conventions in the
    MicroPython source, and we must match each one exactly.
    """
    row_bytes = (width + 3) // 4
    data = bytearray(row_bytes * height)
    for y in range(height):
        for x in range(width):
            idx = indices[y * width + x] & 0x03
            byte_pos = y * row_bytes + x // 4
            shift = (x % 4) * 2
            data[byte_pos] |= idx << shift
    return bytes(data)


def pack_gs4_hmsb(indices: list, width: int, height: int) -> bytes:
    """Pack indexed pixels as GS4_HMSB (4 bits/pixel, high nibble=leftmost)."""
    row_bytes = (width + 1) // 2
    data = bytearray(row_bytes * height)
    for y in range(height):
        for x in range(width):
            idx = indices[y * width + x] & 0x0F
            byte_pos = y * row_bytes + x // 2
            if x % 2 == 0:
                data[byte_pos] |= idx << 4
            else:
                data[byte_pos] |= idx
    return bytes(data)


def pack_gs8(indices: list, width: int, height: int) -> bytes:
    """Pack indexed pixels as GS8 (1 byte/pixel)."""
    data = bytearray(width * height)
    for i, idx in enumerate(indices):
        data[i] = idx & 0xFF
    return bytes(data)


def pack_rgb565(rgb_pixels: list, width: int, height: int) -> bytes:
    """Pack a flat list of (r, g, b) tuples as little-endian RGB565."""
    data = bytearray(width * height * 2)
    for i, (r, g, b) in enumerate(rgb_pixels):
        val = rgb565(r, g, b)
        data[i * 2] = val & 0xFF
        data[i * 2 + 1] = (val >> 8) & 0xFF
    return bytes(data)


def palette_to_rgb565_bytes(palette: list) -> bytes:
    """Encode a list of (r, g, b) tuples as little-endian RGB565 bytes."""
    data = bytearray(len(palette) * 2)
    for i, (r, g, b) in enumerate(palette):
        val = rgb565(r, g, b)
        data[i * 2] = val & 0xFF
        data[i * 2 + 1] = (val >> 8) & 0xFF
    return bytes(data)


# Paletted format candidates in ascending bits-per-pixel order.
# Each entry: (display_name, max_colors, bits_per_pixel, pack_fn, framebuf_const)
_PALETTED_CANDIDATES = [
    ("MONO_HLSB", 2, 1, pack_mono_hlsb, "framebuf.MONO_HLSB"),
    ("GS2_HMSB", 4, 2, pack_gs2_hmsb, "framebuf.GS2_HMSB"),
    ("GS4_HMSB", 16, 4, pack_gs4_hmsb, "framebuf.GS4_HMSB"),
    ("GS8", 256, 8, pack_gs8, "framebuf.GS8"),
]


def _pixel_bytes(bits_per_pixel: int, width: int, height: int) -> int:
    """Compute packed pixel byte count for a given bit depth and image size."""
    if bits_per_pixel == 1:
        return ((width + 7) // 8) * height
    if bits_per_pixel == 2:
        return ((width + 3) // 4) * height
    if bits_per_pixel == 4:
        return ((width + 1) // 2) * height
    if bits_per_pixel == 8:
        return width * height
    if bits_per_pixel == 16:
        return width * height * 2
    raise ValueError(f"unsupported bits_per_pixel: {bits_per_pixel}")


def choose_format(num_colors: int, width: int, height: int) -> dict:
    """Pick the smallest-total-byte format for a sprite with `num_colors` unique colors.

    Evaluates MONO_HLSB, GS2_HMSB, GS4_HMSB, and GS8 (each only if the color
    budget fits), plus RGB565 as an always-available no-palette fallback.
    Total byte cost is pixel bytes plus palette overhead (num_colors * 2
    for paletted formats, 0 for RGB565). On ties the earlier (smaller
    bits-per-pixel) candidate wins because we only replace `best` on
    strict improvement.

    Returns a dict with:
      name          - short format name for logs
      fmt_const     - framebuf constant as a source string
      pack_fn       - packer function (or None for RGB565)
      paletted      - True if the choice requires a palette
      pixel_bytes   - packed pixel byte count
      palette_bytes - palette overhead byte count
      total_bytes   - pixel_bytes + palette_bytes
    """
    best = None

    for name, budget, bpp, pack_fn, fmt_const in _PALETTED_CANDIDATES:
        if num_colors > budget:
            continue
        pixel_bytes = _pixel_bytes(bpp, width, height)
        palette_bytes = num_colors * 2
        total = pixel_bytes + palette_bytes
        candidate = {
            "name": name,
            "fmt_const": fmt_const,
            "pack_fn": pack_fn,
            "paletted": True,
            "pixel_bytes": pixel_bytes,
            "palette_bytes": palette_bytes,
            "total_bytes": total,
        }
        if best is None or total < best["total_bytes"]:
            best = candidate

    rgb_pixel_bytes = _pixel_bytes(16, width, height)
    rgb_candidate = {
        "name": "RGB565",
        "fmt_const": "framebuf.RGB565",
        "pack_fn": None,
        "paletted": False,
        "pixel_bytes": rgb_pixel_bytes,
        "palette_bytes": 0,
        "total_bytes": rgb_pixel_bytes,
    }
    if best is None or rgb_pixel_bytes < best["total_bytes"]:
        best = rgb_candidate

    return best


def convert_image(img: Image.Image) -> dict:
    """Unified converter: normalize to RGBA, scan palette, auto-select format, pack."""
    width, height = img.size
    rgb_pixels, has_transparent = flatten_rgba(img)
    indices, palette = scan_palette(rgb_pixels, reserve_transparent=has_transparent)
    num_colors = len(palette)

    choice = choose_format(num_colors, width, height)

    if choice["paletted"]:
        data = choice["pack_fn"](indices, width, height)
        palette_data = palette_to_rgb565_bytes(palette)
        # framebuf.blit() maps the source pixel through the palette BEFORE
        # comparing against key, so the key is the mapped RGB565 color of the
        # reserved index-0 magenta — NOT the index itself.
        key = _TRANSPARENT_RGB565 if has_transparent else -1
        transparency_note = " (transparent reserved)" if has_transparent else ""
        mode_desc = (
            f"{choice['name']} auto, {num_colors} colors{transparency_note}, "
            f"{choice['pixel_bytes']}+{choice['palette_bytes']}="
            f"{choice['total_bytes']} bytes"
        )
    else:
        data = pack_rgb565(rgb_pixels, width, height)
        palette_data = None
        # For RGB565 blits, key is compared against the raw RGB565 value.
        key = _TRANSPARENT_RGB565 if has_transparent else -1
        transparency_note = " (magenta key)" if has_transparent else ""
        mode_desc = (
            f"RGB565 auto, {num_colors} unique colors{transparency_note}, "
            f"{choice['total_bytes']} bytes"
        )

    return {
        "width": width,
        "height": height,
        "format": choice["fmt_const"],
        "data": data,
        "palette": palette_data,
        "palette_count": num_colors if choice["paletted"] else 0,
        "mode_desc": mode_desc,
        "key": key,
    }


def generate_module(
    info: dict,
    *,
    source_desc: str,
    extra_constants: "dict[str, int] | None" = None,
) -> str:
    """Generate a sprite .py module source text.

    `source_desc` goes into the leading comment (e.g. "ball.png" or
    "mlb_layout.png layer 'field__absolute'"). `extra_constants` is an
    optional ordered dict of name -> int constants to emit before WIDTH/HEIGHT
    (used for X/Y on __absolute layers).
    """
    lines = [
        f"# Generated by tools/compile_layout.py from {source_desc}",
        f"# Mode: {info['mode_desc']}, {info['width']}x{info['height']}",
        "import framebuf",
        "",
    ]

    # KEY is emitted for every sprite (always present in `info`).
    # Put it first, followed by any extra positional constants (e.g. X, Y).
    merged_constants = {"KEY": info["key"]}
    if extra_constants:
        merged_constants.update(extra_constants)
    for const_name, value in merged_constants.items():
        lines.append(f"{const_name} = {value}")
    lines.append("")

    lines.extend(
        [
            f"WIDTH = {info['width']}",
            f"HEIGHT = {info['height']}",
            "",
            format_bytes(info["data"], "_data"),
            "",
        ]
    )

    # Construct a mutable bytearray for FrameBuffer (FrameBuffer requires a
    # writable buffer, so the bytes literal can't be passed directly). Then
    # `del _data` drops the module's reference to the transient bytes literal
    # so the GC can reclaim it — the bytearray, referenced by FrameBuffer, is
    # the only persistent copy.
    if info["palette"] is not None:
        lines.append(format_bytes(info["palette"], "_palette_data"))
        lines.append("")
        lines.append(
            f"data = framebuf.FrameBuffer(bytearray(_data), WIDTH, HEIGHT, {info['format']})"
        )
        lines.append(
            f"palette = framebuf.FrameBuffer(bytearray(_palette_data), {info['palette_count']}, 1, framebuf.RGB565)"
        )
        lines.append("del _data, _palette_data")
    else:
        lines.append(
            f"data = framebuf.FrameBuffer(bytearray(_data), WIDTH, HEIGHT, {info['format']})"
        )
        lines.append("del _data")

    lines.append("")
    return "\n".join(lines)


def generate_slice_module(source_desc: str, slice_name: str, x: int, y: int, w: int, h: int) -> str:
    """Generate a coordinate-only slice module."""
    return (
        f"# Generated by tools/compile_layout.py from {source_desc}\n"
        f"# Slice: {slice_name!r} @ ({x}, {y}) {w}x{h}\n"
        f"\n"
        f"X = {x}\n"
        f"Y = {y}\n"
        f"WIDTH = {w}\n"
        f"HEIGHT = {h}\n"
    )


def _parse_layer_suffix(name: str):
    """Return (base_name, suffix_kind) where suffix_kind is 'relative', 'absolute', or None."""
    if name.endswith(_RELATIVE_SUFFIX):
        return name[: -len(_RELATIVE_SUFFIX)], "relative"
    if name.endswith(_ABSOLUTE_SUFFIX):
        return name[: -len(_ABSOLUTE_SUFFIX)], "absolute"
    return name, None


def _validate_identifier(name: str, kind: str, source_stem: str) -> None:
    """Fail hard if `name` isn't a valid Python module/identifier."""
    if not name:
        raise SystemExit(
            f"ERROR: {source_stem}.json has a {kind} with an empty name after suffix stripping"
        )
    if not name.isidentifier():
        raise SystemExit(
            f"ERROR: {source_stem}.json has a {kind} {name!r} that is not a valid Python identifier"
        )


def compile_layout(png_path: Path, json_path: Path) -> list:
    """Parse an Aseprite sprite sheet + JSON sidecar into output modules.

    Returns a list of (output_name, source_description, log_message, module_source)
    tuples, one per emitted module. Layers without a `__relative` or `__absolute`
    suffix are silently skipped (logged as skipped in the caller if desired).
    """
    source_stem = png_path.stem
    with json_path.open("r") as f:
        layout = json.load(f)

    sheet = Image.open(png_path).convert("RGBA")
    sheet_w, sheet_h = sheet.size

    outputs = []

    # --- Process layers (frames keyed by layer name) ---
    frames = layout.get("frames", {})
    for layer_name, frame_entry in frames.items():
        base_name, kind = _parse_layer_suffix(layer_name)

        if kind is None:
            # Unsuffixed layer: purely visual reference in Aseprite, skip silently.
            continue

        _validate_identifier(base_name, f"layer {layer_name!r}", source_stem)

        f = frame_entry["frame"]
        fx, fy, fw, fh = f["x"], f["y"], f["w"], f["h"]
        if fx < 0 or fy < 0 or fx + fw > sheet_w or fy + fh > sheet_h:
            raise SystemExit(
                f"ERROR: layer {layer_name!r} frame {fx},{fy} {fw}x{fh} "
                f"exceeds sprite sheet bounds {sheet_w}x{sheet_h}"
            )

        cropped = sheet.crop((fx, fy, fx + fw, fy + fh))
        info = convert_image(cropped)

        desc = f"{source_stem}.png layer {layer_name!r}"

        if kind == "relative":
            source = generate_module(info, source_desc=desc)
            log = (
                f"  layer {layer_name!r} -> {base_name}.py "
                f"({info['mode_desc']}, relative)"
            )
        else:  # absolute
            spos = frame_entry["spriteSourceSize"]
            x, y = spos["x"], spos["y"]
            source = generate_module(
                info,
                source_desc=desc,
                extra_constants={"X": x, "Y": y},
            )
            log = (
                f"  layer {layer_name!r} -> {base_name}.py "
                f"({info['mode_desc']}, absolute @ ({x}, {y}))"
            )

        outputs.append((base_name, desc, log, source))

    # --- Process slices (all emitted, no suffix filtering) ---
    meta = layout.get("meta", {})
    for slice_entry in meta.get("slices", []):
        slice_name = slice_entry["name"]
        _validate_identifier(slice_name, f"slice {slice_name!r}", source_stem)

        keys = slice_entry.get("keys", [])
        if len(keys) != 1:
            raise SystemExit(
                f"ERROR: slice {slice_name!r} in {source_stem}.json has "
                f"{len(keys)} keys; expected exactly 1 "
                f"(animated slices are not supported)"
            )
        bounds = keys[0]["bounds"]
        x, y = bounds["x"], bounds["y"]
        w, h = bounds["w"], bounds["h"]

        desc = f"{source_stem}.png slice {slice_name!r}"
        source = generate_slice_module(desc, slice_name, x, y, w, h)
        log = f"  slice {slice_name!r} -> {slice_name}.py ({w}x{h} @ ({x}, {y}))"

        outputs.append((slice_name, desc, log, source))

    return outputs


def _register_output(outputs: dict, name: str, desc: str, source: str) -> None:
    """Register an output under `name`, failing hard on collision."""
    if name in outputs:
        existing_desc = outputs[name][0]
        print(
            f"\nERROR: output filename collision: {name}.py\n"
            f"  First source:  {existing_desc}\n"
            f"  Second source: {desc}\n"
            f"Rename, remove, or re-suffix one of them.",
            file=sys.stderr,
        )
        sys.exit(1)
    outputs[name] = (desc, source)


def _resolve_aseprite() -> str:
    """Locate the Aseprite CLI. Checks $ASEPRITE, then PATH, then Windows fallbacks."""
    env = os.environ.get("ASEPRITE")
    if env and Path(env).exists():
        return env

    found = shutil.which("aseprite") or shutil.which("Aseprite")
    if found:
        return found

    fallbacks = [
        r"C:\Program Files\Aseprite\Aseprite.exe",
        r"C:\Program Files (x86)\Steam\steamapps\common\Aseprite\Aseprite.exe",
    ]
    for fb in fallbacks:
        if Path(fb).exists():
            return fb

    raise SystemExit(
        "Aseprite CLI not found. Set the ASEPRITE env var to the executable "
        "path, add aseprite to PATH, or install to a standard location."
    )


def _export_aseprite(exe: str, aseprite_path: Path, out_dir: Path) -> "tuple[Path, Path]":
    """Export an .aseprite file to a layer-split PNG + JSON pair in out_dir.

    Flags chosen to reproduce the existing manual export:
      --split-layers            one frame per Aseprite layer (drives per-layer modules)
      --list-layers             include meta.layers[] in the JSON
      --list-slices             include meta.slices[] (coordinate regions)
      --sheet-type packed       tightest atlas packing
      --format json-hash        frames keyed by name (compile_layout expects this)
      --filename-format {layer} frame keys are bare layer names (e.g. "field__absolute")
                                rather than Aseprite's CLI default of
                                "{title} ({layer}).{extension}"
    """
    png = out_dir / f"{aseprite_path.stem}.png"
    js = out_dir / f"{aseprite_path.stem}.json"
    cmd = [
        exe, "--batch",
        "--split-layers", "--list-layers", "--list-slices",
        "--trim",
        "--ignore-empty",
        "--merge-duplicates",
        "--sheet-type", "packed",
        "--filename-format", "{layer}",
        "--sheet", str(png),
        "--data", str(js),
        "--format", "json-hash",
        str(aseprite_path),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(
            f"Aseprite export failed for {aseprite_path.name}:\n"
            f"  cmd: {' '.join(cmd)}\n"
            f"  stderr: {result.stderr}"
        )
    return png, js


def compile_all() -> None:
    """Compile every .aseprite and plain .png in ASSETS_DIR into OUTPUT_DIR.

    Aseprite exports are routed through a temp directory and never hit the
    source tree. Stale `.py` files in OUTPUT_DIR that aren't part of the new
    output set are removed so renamed layers/slices don't leave orphans.
    `archive/` subdirectories are skipped — they hold inert reference art.
    """
    if not ASSETS_DIR.is_dir():
        print(f"Error: assets directory not found: {ASSETS_DIR}", file=sys.stderr)
        sys.exit(1)

    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    aseprite_files = sorted(ASSETS_DIR.glob("*.aseprite"))
    plain_pngs = sorted(p for p in ASSETS_DIR.glob("*.png") if p.is_file())

    if not aseprite_files and not plain_pngs:
        print(f"No source files found in {ASSETS_DIR.relative_to(REPO_ROOT)}/")
        return

    outputs: "dict[str, tuple[str, str]]" = {}

    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)

        if aseprite_files:
            aseprite_exe = _resolve_aseprite()
            for asp in aseprite_files:
                print(f"[{asp.stem}] exporting via Aseprite CLI")
                png, js = _export_aseprite(aseprite_exe, asp, tmp_dir)
                for out_name, desc, log_msg, source in compile_layout(png, js):
                    _register_output(outputs, out_name, desc, source)
                    print(log_msg)

        for png_path in plain_pngs:
            stem = png_path.stem
            print(f"[{stem}] processing plain PNG")
            img = Image.open(png_path)
            info = convert_image(img)
            source = generate_module(info, source_desc=f"{stem}.png")
            _register_output(outputs, stem, f"{stem}.png", source)
            print(f"  -> {stem}.py ({info['mode_desc']})")

    # Write new outputs, then remove stale modules that aren't in the new set.
    new_filenames = set()
    for out_name, (_desc, source) in sorted(outputs.items()):
        out_path = OUTPUT_DIR / f"{out_name}.py"
        out_path.write_text(source)
        new_filenames.add(out_path.name)

    removed = 0
    for existing in OUTPUT_DIR.glob("*.py"):
        if existing.name not in new_filenames:
            existing.unlink()
            removed += 1
            print(f"  Removed stale: {existing.name}")

    out_rel = OUTPUT_DIR.relative_to(REPO_ROOT).as_posix()
    stale_note = f" ({removed} stale removed)" if removed else ""
    print(f"\nDone: {len(outputs)} module(s) written to {out_rel}/{stale_note}")


def main():
    compile_all()


if __name__ == "__main__":
    main()

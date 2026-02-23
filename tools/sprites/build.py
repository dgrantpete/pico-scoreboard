#!/usr/bin/env python3
"""Convert PNG sprites to MicroPython framebuf modules.

Reads all .png files from src/ and writes .py modules to build/.
Each module exports a framebuf.FrameBuffer object named `img`.
Palette-mode images also export a `palette` FrameBuffer for use with blit().
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print("Pillow is required: pip install Pillow", file=sys.stderr)
    sys.exit(1)

SCRIPT_DIR = Path(__file__).resolve().parent
SRC_DIR = SCRIPT_DIR / "src"
BUILD_DIR = SCRIPT_DIR / "build"


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


def pack_rgb565(pixels, width: int, height: int, has_alpha: bool) -> bytes:
    """Pack pixel data as little-endian RGB565."""
    data = bytearray(width * height * 2)
    for i, px in enumerate(pixels):
        if has_alpha and px[3] == 0:
            val = 0x0000
        else:
            val = rgb565(px[0], px[1], px[2])
        data[i * 2] = val & 0xFF
        data[i * 2 + 1] = (val >> 8) & 0xFF
    return bytes(data)


def pack_mono_hlsb(pixels, width: int, height: int, remap: dict) -> bytes:
    """Pack indexed pixels as MONO_HLSB (1 bit/pixel, MSB=leftmost)."""
    row_bytes = (width + 7) // 8
    data = bytearray(row_bytes * height)
    for y in range(height):
        for x in range(width):
            idx = remap[pixels[y * width + x]]
            if idx:
                byte_pos = y * row_bytes + x // 8
                bit_pos = 7 - (x % 8)
                data[byte_pos] |= 1 << bit_pos
    return bytes(data)


def pack_gs4_hmsb(pixels, width: int, height: int, remap: dict) -> bytes:
    """Pack indexed pixels as GS4_HMSB (4 bits/pixel, high nibble=leftmost)."""
    row_bytes = (width + 1) // 2
    data = bytearray(row_bytes * height)
    for y in range(height):
        for x in range(width):
            idx = remap[pixels[y * width + x]]
            byte_pos = y * row_bytes + x // 2
            if x % 2 == 0:
                data[byte_pos] |= (idx & 0x0F) << 4
            else:
                data[byte_pos] |= idx & 0x0F
    return bytes(data)


def pack_gs8(pixels, width: int, height: int, remap: dict) -> bytes:
    """Pack indexed pixels as GS8 (1 byte/pixel)."""
    data = bytearray(width * height)
    for i, px in enumerate(pixels):
        data[i] = remap[px]
    return bytes(data)


def build_palette_data(raw_palette: list, remap: dict, num_colors: int) -> bytes:
    """Build RGB565 palette data (little-endian) for the remapped indices."""
    # Invert remap: new_index -> original_index
    inv = {v: k for k, v in remap.items()}
    palette_data = bytearray(num_colors * 2)
    for new_idx in range(num_colors):
        orig = inv[new_idx]
        r = raw_palette[orig * 3]
        g = raw_palette[orig * 3 + 1]
        b = raw_palette[orig * 3 + 2]
        val = rgb565(r, g, b)
        palette_data[new_idx * 2] = val & 0xFF
        palette_data[new_idx * 2 + 1] = (val >> 8) & 0xFF
    return bytes(palette_data)


def convert_rgb(img: Image.Image) -> dict:
    """Convert an RGB-mode image."""
    w, h = img.size
    pixels = list(img.getdata())
    return {
        "width": w,
        "height": h,
        "format": "framebuf.RGB565",
        "data": pack_rgb565(pixels, w, h, has_alpha=False),
        "palette": None,
        "palette_count": 0,
        "mode_desc": "RGB",
    }


def convert_rgba(img: Image.Image) -> dict:
    """Convert an RGBA-mode image (alpha=0 becomes black)."""
    w, h = img.size
    pixels = list(img.getdata())
    return {
        "width": w,
        "height": h,
        "format": "framebuf.RGB565",
        "data": pack_rgb565(pixels, w, h, has_alpha=True),
        "palette": None,
        "palette_count": 0,
        "mode_desc": "RGBA",
    }


def convert_palette(img: Image.Image) -> dict:
    """Convert a palette-mode image using the smallest indexed format."""
    w, h = img.size
    pixels = list(img.getdata())
    unique = sorted(set(pixels))
    num_colors = len(unique)

    # Remap original palette indices to contiguous 0..N-1
    remap = {orig: new for new, orig in enumerate(unique)}

    # Choose smallest format
    if num_colors <= 2:
        fmt = "framebuf.MONO_HLSB"
        data = pack_mono_hlsb(pixels, w, h, remap)
    elif num_colors <= 16:
        fmt = "framebuf.GS4_HMSB"
        data = pack_gs4_hmsb(pixels, w, h, remap)
    else:
        fmt = "framebuf.GS8"
        data = pack_gs8(pixels, w, h, remap)

    raw_palette = img.getpalette()
    palette_data = build_palette_data(raw_palette, remap, num_colors)

    return {
        "width": w,
        "height": h,
        "format": fmt,
        "data": data,
        "palette": palette_data,
        "palette_count": num_colors,
        "mode_desc": f"P ({num_colors} colors)",
    }


def generate_module(name: str, info: dict) -> str:
    """Generate the .py module source text."""
    lines = [
        f"# Generated by tools/sprites/build.py from {name}.png",
        f"# Mode: {info['mode_desc']}, {info['width']}x{info['height']}",
        "import framebuf",
        "",
        f"WIDTH = {info['width']}",
        f"HEIGHT = {info['height']}",
        "",
        format_bytes(info["data"], "_data"),
        "",
    ]

    if info["palette"] is not None:
        lines.append(format_bytes(info["palette"], "_palette_data"))
        lines.append("")
        lines.append(
            f"data = framebuf.FrameBuffer(bytearray(_data), WIDTH, HEIGHT, {info['format']})"
        )
        lines.append(
            f"palette = framebuf.FrameBuffer(bytearray(_palette_data), {info['palette_count']}, 1, framebuf.RGB565)"
        )
    else:
        lines.append(
            f"data = framebuf.FrameBuffer(bytearray(_data), WIDTH, HEIGHT, {info['format']})"
        )

    lines.append("")  # trailing newline
    return "\n".join(lines)


def main():
    if not SRC_DIR.is_dir():
        print(f"Error: source directory not found: {SRC_DIR}", file=sys.stderr)
        sys.exit(1)

    BUILD_DIR.mkdir(parents=True, exist_ok=True)

    pngs = sorted(SRC_DIR.glob("*.png"))
    if not pngs:
        print("No .png files found in src/")
        return

    for png_path in pngs:
        img = Image.open(png_path)
        stem = png_path.stem

        if img.mode == "RGB":
            info = convert_rgb(img)
        elif img.mode == "RGBA":
            info = convert_rgba(img)
        elif img.mode == "P":
            info = convert_palette(img)
        else:
            print(f"  {stem}: converting {img.mode} -> RGB")
            img = img.convert("RGB")
            info = convert_rgb(img)

        source = generate_module(stem, info)
        out_path = BUILD_DIR / f"{stem}.py"
        out_path.write_text(source)
        print(f"  {stem}.png -> build/{stem}.py ({info['mode_desc']}, {len(info['data'])} bytes)")

    print(f"Done: {len(pngs)} sprite(s) converted")


if __name__ == "__main__":
    main()

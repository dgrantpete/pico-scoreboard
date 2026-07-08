"""Oracle tests for the framebuf shim against tools/compile_layout.py packers.

`compile_layout.py`'s `pack_*` functions are the NORMATIVE definition of how
sprite bytes are laid out per format. This test packs indexed pixels with those
packers, wraps the bytes in a shim `FrameBuffer`, and asserts the shim reads
back exactly the indices that went in -- for every supported format, at odd
widths, with the tricky bit orders (GS2 low-bits-leftmost, GS4 even-x-high-
nibble). It then covers blit semantics: palette-lookup-before-colorkey, tuple
sources, negative-offset clipping, Region-style stride over a parent slice,
and hline/vline clipping.

Runnable two ways:
    python -m pytest tools/preview/tests/test_framebuf_shim.py -q
    python tools/preview/tests/test_framebuf_shim.py
"""

import os
import sys

_REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
if _REPO_ROOT not in sys.path:
    sys.path.insert(0, _REPO_ROOT)

from tools.compile_layout import (
    pack_mono_hlsb, pack_gs2_hmsb, pack_gs4_hmsb, pack_gs8, pack_rgb565,
    rgb565 as ref_rgb565,
)
from tools.preview.shims import framebuf_shim as fb


def _roundtrip(pack_fn, fmt, indices, w, h):
    """Pack `indices` with the oracle packer, read them back through the shim."""
    data = bytearray(pack_fn(indices, w, h))
    frame = fb.FrameBuffer(data, w, h, fmt)
    return [frame.pixel(x, y) for y in range(h) for x in range(w)]


def test_mono_hlsb_bit_order():
    w, h = 11, 3  # odd width forces stride rounding to 16
    indices = [(x + y) % 2 for y in range(h) for x in range(w)]
    assert _roundtrip(pack_mono_hlsb, fb.MONO_HLSB, indices, w, h) == indices


def test_gs2_hmsb_low_bits_leftmost():
    w, h = 7, 2  # odd for 4-px packing
    indices = [(x * 3 + y) % 4 for y in range(h) for x in range(w)]
    got = _roundtrip(pack_gs2_hmsb, fb.GS2_HMSB, indices, w, h)
    assert got == indices
    # Explicit check: x=0 lives in the LOW two bits of its byte.
    data = bytearray(pack_gs2_hmsb([1, 0, 0, 0], 4, 1))
    assert data[0] & 0x03 == 1


def test_gs4_hmsb_even_x_high_nibble():
    w, h = 5, 2  # odd for 2-px packing
    indices = [(x * 5 + y) % 16 for y in range(h) for x in range(w)]
    got = _roundtrip(pack_gs4_hmsb, fb.GS4_HMSB, indices, w, h)
    assert got == indices
    # Explicit check: even x=0 occupies the HIGH nibble.
    data = bytearray(pack_gs4_hmsb([0xA, 0x0], 2, 1))
    assert data[0] >> 4 == 0xA


def test_gs8_roundtrip():
    w, h = 3, 3
    indices = [(x * 7 + y * 13) % 256 for y in range(h) for x in range(w)]
    assert _roundtrip(pack_gs8, fb.GS8, indices, w, h) == indices


def test_rgb565_little_endian_roundtrip():
    w, h = 3, 2
    pixels = [(255, 0, 0), (0, 255, 0), (0, 0, 255),
              (255, 255, 255), (0, 0, 0), (18, 52, 86)]
    data = bytearray(pack_rgb565(pixels, w, h))
    frame = fb.FrameBuffer(data, w, h, fb.RGB565)
    got = [frame.pixel(x, y) for y in range(h) for x in range(w)]
    assert got == [ref_rgb565(*p) for p in pixels]
    # First pixel bytes are little-endian.
    assert data[0] == (ref_rgb565(255, 0, 0) & 0xFF)
    assert data[1] == (ref_rgb565(255, 0, 0) >> 8)


def test_blit_palette_lookup_before_colorkey():
    # Paletted source: index 0 -> magenta (the key), index 1 -> white.
    src_data = bytearray(pack_mono_hlsb([1, 0, 1, 0], 4, 1))
    src = fb.FrameBuffer(src_data, 4, 1, fb.MONO_HLSB)
    pal_data = bytearray(pack_rgb565([(255, 0, 255), (255, 255, 255)], 2, 1))
    palette = fb.FrameBuffer(pal_data, 2, 1, fb.RGB565)

    dest = fb.FrameBuffer(bytearray(4 * 1 * 2), 4, 1, fb.RGB565)
    dest.fill(0x1234)
    dest.blit(src, 0, 0, 0xF81F, palette)  # key = magenta = mapped index 0

    white = ref_rgb565(255, 255, 255)
    assert dest.pixel(0, 0) == white   # index 1 -> white, drawn
    assert dest.pixel(1, 0) == 0x1234  # index 0 -> magenta == key, skipped
    assert dest.pixel(2, 0) == white
    assert dest.pixel(3, 0) == 0x1234


def test_blit_tuple_source():
    # Font glyphs are (memoryview, w, h, MONO_HLSB) tuples.
    glyph = (bytearray(pack_mono_hlsb([1, 1, 0, 0], 4, 1)), 4, 1, fb.MONO_HLSB)
    pal = fb.FrameBuffer(bytearray(pack_rgb565([(0, 0, 0), (255, 255, 255)], 2, 1)),
                         2, 1, fb.RGB565)
    dest = fb.FrameBuffer(bytearray(4 * 2), 4, 1, fb.RGB565)
    dest.blit(glyph, 0, 0, -1, pal)
    white = ref_rgb565(255, 255, 255)
    assert [dest.pixel(x, 0) for x in range(4)] == [white, white, 0, 0]


def test_blit_negative_offset_clipping():
    src = fb.FrameBuffer(bytearray(pack_gs8([1, 2, 3, 4], 4, 1)), 4, 1, fb.GS8)
    pal = fb.FrameBuffer(
        bytearray(pack_rgb565([(0, 0, 0), (10, 0, 0), (20, 0, 0), (30, 0, 0), (40, 0, 0)], 5, 1)),
        5, 1, fb.RGB565)
    dest = fb.FrameBuffer(bytearray(4 * 2), 4, 1, fb.RGB565)
    dest.blit(src, -2, 0, -1, pal)  # first two source pixels clipped off-left
    # src pixel cx=2 (value 3) lands at dx=0; palette[3] = (30,0,0)
    assert dest.pixel(0, 0) == ref_rgb565(30, 0, 0)
    assert dest.pixel(1, 0) == ref_rgb565(40, 0, 0)  # cx=3 value 4 -> pal[4]
    assert dest.pixel(2, 0) == 0
    assert dest.pixel(3, 0) == 0


def test_region_style_stride_over_parent_slice():
    # A 4x4 RGB565 parent; a 2x2 sub-view at (1,1) with stride == parent width.
    parent = fb.FrameBuffer(bytearray(4 * 4 * 2), 4, 4, fb.RGB565)
    parent.fill(0x0000)
    view = memoryview(parent._buf)[(1 * 4 + 1) * 2:]
    region = fb.FrameBuffer(view, 2, 2, fb.RGB565, 4)
    region.fill(0xABCD)
    # Only the inner 2x2 block should be set in the parent.
    for y in range(4):
        for x in range(4):
            inside = 1 <= x <= 2 and 1 <= y <= 2
            assert parent.pixel(x, y) == (0xABCD if inside else 0x0000), (x, y)


def test_region_blit_clips_to_width():
    # Writing past a region's width must not spill into the next parent row.
    parent = fb.FrameBuffer(bytearray(4 * 2 * 2), 4, 2, fb.RGB565)
    view = memoryview(parent._buf)
    region = fb.FrameBuffer(view, 2, 2, fb.RGB565, 4)  # full-height narrow view
    region.fill_rect(0, 0, 10, 1, 0x7777)  # width 10 clipped to region width 2
    assert parent.pixel(0, 0) == 0x7777
    assert parent.pixel(1, 0) == 0x7777
    assert parent.pixel(2, 0) == 0x0000  # not spilled
    assert parent.pixel(3, 0) == 0x0000


def test_hline_vline_clipping():
    frame = fb.FrameBuffer(bytearray(8 * 8 * 2), 8, 8, fb.RGB565)
    frame.hline(-3, 2, 20, 0x1111)  # over-long, negative start
    assert all(frame.pixel(x, 2) == 0x1111 for x in range(8))
    assert frame.pixel(0, 1) == 0x0000
    frame.vline(5, -2, 20, 0x2222)
    assert all(frame.pixel(5, y) == 0x2222 for y in range(8))


def test_odd_width_stride_isolated_rows():
    # 5-wide GS4: stride rounds to 6 px (3 bytes/row); row 1 must not read row 0.
    w, h = 5, 2
    indices = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
    frame = fb.FrameBuffer(bytearray(pack_gs4_hmsb(indices, w, h)), w, h, fb.GS4_HMSB)
    assert [frame.pixel(x, 0) for x in range(w)] == [1, 2, 3, 4, 5]
    assert [frame.pixel(x, 1) for x in range(w)] == [6, 7, 8, 9, 10]


def _run_standalone():
    funcs = [g for name, g in sorted(globals().items()) if name.startswith("test_")]
    failed = 0
    for fn in funcs:
        try:
            fn()
            print(f"  PASS  {fn.__name__}")
        except AssertionError as exc:
            failed += 1
            print(f"  FAIL  {fn.__name__}: {exc}")
    print(f"\n{'ALL PASSED' if not failed else f'{failed} FAILED'} ({len(funcs)} tests)")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(_run_standalone())

"""Golden-value generator for the scoreboard-render crate's parity tests.

Every number this emits is produced by running the REAL reference — never by
transcribing it:

* packed sprite bytes come from `tools/compile_layout.py`'s `pack_*` functions,
  which are the normative definition of the five formats' bit layouts (the
  MicroPython framebuf shim is pinned against these same functions);
* glyph records come from `tools/compile_fonts.py`'s `build_blobs`, resolved
  through the same index arithmetic the Rust `FontFace` performs, so a wrong
  offset or a missed absent-glyph fallback shows up as different bits;
* the dim-frame masks come from the CPython branch of `display._dim_frame`,
  extracted from `display.py` at generation time and executed — not copied, so
  it cannot drift;
* the QR matrices come from the independent `qrcode` package, not from the
  encoder under test.

Re-run after any change to the packers, the font pipeline, `_dim_frame`, or the
QR parameters:

    py crates/scoreboard-render/tests/gen_goldens.py

Needs: Pillow (compile_layout imports it), freetype, and `qrcode`.
"""

import random
import sys
import textwrap
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
OUT_PATH = Path(__file__).resolve().parent / "goldens" / "mod.rs"
sys.path.insert(0, str(REPO))

from tools.compile_layout import (  # noqa: E402
    pack_gs2_hmsb,
    pack_gs4_hmsb,
    pack_gs8,
    pack_mono_hlsb,
    pack_rgb565,
    rgb565,
)
from tools.compile_fonts import ABSENT, MINCHAR, build_blobs  # noqa: E402

FONTS = [
    ("UNSCII_8", "unscii_8.pcf", 8),
    ("UNSCII_16", "unscii_16.pcf", 16),
    ("SPLEEN_5X8", "spleen-5x8.bdf", 8),
]

# One glyph per interesting lookup path: a legitimately blank one, a digit, a
# letter, the default glyph itself, two Latin-1 codepoints (drawn by unscii,
# stand-ins in spleen), a codepoint inside the C1 control gap (absent -> falls
# back to the default), and the last table entry.
SPOT_CODEPOINTS = [0x20, 0x30, 0x41, 0x3F, 0xE9, 0xF1, 0x7F, 0xFF]

# The setup QR's payload, and the parameters QrBitmap::encode uses. The SSID is
# a realistic AP name; the length matters, because it decides the version.
QR_PAYLOAD = "WIFI:T:nopass;S:pico-scoreboard;;"


def fnv1a64(data: bytes) -> int:
    """FNV-1a, 64-bit. Small enough to reimplement identically in Rust."""
    value = 0xCBF29CE484222325
    for byte in data:
        value = ((value ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return value


def rust_bytes(data) -> str:
    return "&[" + ", ".join(f"0x{b:02x}" for b in data) + "]"


def rust_u16s(values) -> str:
    return "&[" + ", ".join(f"0x{v:04x}" for v in values) + "]"


# --- Packed-format cases -----------------------------------------------------

def packed_cases() -> str:
    """Pack index patterns with the normative packers, at odd widths so the
    per-row byte rounding is exercised."""
    cases = []

    def case(name, pack_fn, width, height, indices):
        packed = pack_fn(indices, width, height)
        cases.append(
            f"    PackedCase {{ name: \"{name}\", width: {width}, height: {height}, "
            f"indices: {rust_bytes(indices)}, packed: {rust_bytes(packed)} }},"
        )

    # Odd width forces the stride to round up to a whole byte.
    case("mono_11x3", pack_mono_hlsb, 11, 3,
         [(x + y) % 2 for y in range(3) for x in range(11)])
    case("gs2_7x2", pack_gs2_hmsb, 7, 2,
         [(x * 3 + y) % 4 for y in range(2) for x in range(7)])
    case("gs4_5x2", pack_gs4_hmsb, 5, 2,
         [(x * 5 + y) % 16 for y in range(2) for x in range(5)])
    case("gs8_3x3", pack_gs8, 3, 3,
         [(x * 7 + y * 13) % 256 for y in range(3) for x in range(3)])
    # The odd-width GS4 row-isolation case from the shim's own tests.
    case("gs4_rows_5x2", pack_gs4_hmsb, 5, 2, list(range(1, 11)))
    # The two cases that pin the formats' disagreeing bit orders: GS2 puts the
    # leftmost pixel in the LOW two bits, GS4 in the HIGH nibble.
    case("gs2_leftmost", pack_gs2_hmsb, 4, 1, [1, 0, 0, 0])
    case("gs4_leftmost", pack_gs4_hmsb, 2, 1, [0xA, 0x0])

    pixels = [(255, 0, 0), (0, 255, 0), (0, 0, 255),
              (255, 255, 255), (0, 0, 0), (18, 52, 86)]
    rgb = pack_rgb565(pixels, 3, 2)
    colors = [rgb565(*p) for p in pixels]

    return (
        "/// Index patterns packed by `tools/compile_layout.py`'s `pack_*`\n"
        "/// functions — the normative definition of each format's bit order.\n"
        "pub const PACKED_CASES: &[PackedCase] = &[\n"
        + "\n".join(cases)
        + "\n];\n\n"
        "/// A 3x2 RGB565 sprite: the packed little-endian bytes and the colors\n"
        "/// they must read back as.\n"
        f"pub const RGB565_PACKED: &[u8] = {rust_bytes(rgb)};\n"
        f"pub const RGB565_COLORS: &[u16] = {rust_u16s(colors)};\n"
    )


# --- Font tables -------------------------------------------------------------

def resolve(heap: bytes, index: bytes, codepoint: int, height: int):
    """The Rust FontFace's lookup, in Python: slot -> offset -> record."""
    slot = codepoint - MINCHAR
    offset = ABSENT
    if 0 <= slot < 224:
        entry = 1 + slot
        offset = int.from_bytes(index[entry * 2:entry * 2 + 2], "little")
    if offset == ABSENT:
        offset = int.from_bytes(index[0:2], "little")
    width = int.from_bytes(heap[offset:offset + 2], "little")
    row_bytes = (width + 7) // 8
    bits = heap[offset + 2:offset + 2 + row_bytes * height]
    return width, bits


def font_goldens() -> str:
    entries = []
    for const_name, filename, height in FONTS:
        heap, index, _stats = build_blobs(REPO / "firmware" / "assets" / "fonts" / filename, height)

        # Digest every table entry, resolved the way a consumer resolves it, so
        # the check covers all 224 codepoints — shared records, stand-ins and
        # absent-glyph fallbacks included — rather than the handful spelled out
        # below. Nothing private has to be exposed to recompute it.
        digest = bytearray()
        for codepoint in range(MINCHAR, 0x100):
            width, bits = resolve(heap, index, codepoint, height)
            digest += width.to_bytes(2, "little") + bits

        spots = []
        for codepoint in SPOT_CODEPOINTS:
            width, bits = resolve(heap, index, codepoint, height)
            spots.append(
                f"        GlyphGolden {{ codepoint: {codepoint}, width: {width}, "
                f"bits: {rust_bytes(bits)} }},"
            )
        entries.append(
            f"    FontGolden {{\n"
            f'        name: "{const_name}",\n'
            f"        height: {height},\n"
            f"        glyphs_fnv: 0x{fnv1a64(bytes(digest)):016x},\n"
            f"        glyphs: &[\n" + "\n".join(spots) + "\n        ],\n"
            f"    }},"
        )
    return (
        "/// Glyph tables as `tools/compile_fonts.py` builds them, one entry per\n"
        "/// font in the order `tests/fonts.rs` pairs them with.\n"
        "pub const FONTS: &[FontGolden] = &[\n" + "\n".join(entries) + "\n];\n"
    )


# --- Dim frame ---------------------------------------------------------------

def load_dim_frame():
    """Execute the CPython branch of `display._dim_frame` from its own source.

    Extracting it rather than copying it is the point: the preview branch must
    stay mask-identical to the viper one, and this golden must stay identical to
    both. Copying would let all three drift apart silently.
    """
    source = (REPO / "firmware" / "src" / "scoreboard" / "display.py").read_text(
        encoding="utf-8"
    )
    lines = source.splitlines()
    starts = [i for i, line in enumerate(lines) if line.strip().startswith("def _dim_frame")]
    if len(starts) != 2:
        raise SystemExit(
            f"display.py should define _dim_frame twice (viper + CPython); found {len(starts)}"
        )
    start = starts[1]
    indent = len(lines[start]) - len(lines[start].lstrip())
    end = start + 1
    while end < len(lines):
        line = lines[end]
        if line.strip() and (len(line) - len(line.lstrip())) <= indent:
            break
        end += 1
    body = textwrap.dedent("\n".join(lines[start:end]))
    namespace = {}
    exec(compile(body, "display.py:_dim_frame", "exec"), namespace)
    return namespace["_dim_frame"]


def dim_goldens() -> str:
    dim_frame = load_dim_frame()
    random.seed(0x5C0DE)
    words = 16
    source = bytearray(random.getrandbits(8) for _ in range(words * 4))
    cases = []
    # (t2, t3) pairs are display._FADE_TERMS: 7/8, 3/4, 5/8, 1/2.
    for index, (t2, t3) in enumerate(((1, 1), (1, 0), (0, 1), (0, 0))):
        buffer = bytearray(source)
        dim_frame(buffer, words, t2, t3)
        cases.append(
            f"    DimCase {{ step: {index}, dimmed: {rust_bytes(buffer)} }},"
        )
    return (
        "/// `display._dim_frame`'s CPython branch, run over a fixed pseudorandom\n"
        "/// buffer at each rung of the fade ladder.\n"
        f"pub const DIM_SOURCE: &[u8] = {rust_bytes(source)};\n\n"
        "pub const DIM_CASES: &[DimCase] = &[\n" + "\n".join(cases) + "\n];\n"
    )


# --- QR ----------------------------------------------------------------------

def qr_golden() -> str:
    """The same payload encoded by the independent `qrcode` package, once per
    mask pattern.

    Two degrees of freedom are the encoder's to choose, so both are pinned here
    rather than compared:

    * **Segmentation.** A QR encoder may split a payload into segments of
      different modes. `qrcodegen`'s `encode_text` emits a single segment;
      others split this payload into an alphanumeric prefix plus a byte
      remainder. Both decode to the same string and neither is wrong, so the
      golden forces the single byte segment both can produce.
    * **Mask selection.** The standard's penalty rules leave room at the
      margins and implementations do disagree — `qrcodegen` advertises
      detecting finder-like patterns "more accurately than other
      implementations". The golden therefore carries all eight masks, and the
      test asserts the encoder matches exactly one.

    What is left compared is the substance: byte-mode encoding, the terminator
    and padding, the Reed-Solomon codewords, block interleaving, module
    placement, the mask XOR itself, and the format and version bits. Two
    independent implementations agreeing on all of that is the check worth
    having.
    """
    import qrcode
    from qrcode.util import MODE_8BIT_BYTE, QRData

    masks = []
    version = None
    size = None
    for mask in range(8):
        code = qrcode.QRCode(
            error_correction=qrcode.constants.ERROR_CORRECT_M,
            border=0,
            mask_pattern=mask,
        )
        code.add_data(QRData(QR_PAYLOAD.encode(), mode=MODE_8BIT_BYTE))
        code.make(fit=True)
        matrix = code.get_matrix()
        version = code.version
        size = len(matrix)
        rows = [
            "        &[" + ", ".join("true" if cell else "false" for cell in row) + "],"
            for row in matrix
        ]
        masks.append("    &[\n" + "\n".join(rows) + "\n    ],")

    return (
        "/// The setup QR's payload, and the symbol the `qrcode` package\n"
        "/// produces for it at medium ECC in byte mode — one matrix per mask\n"
        "/// pattern. See gen_goldens.py's `qr_golden` for what is pinned and why.\n"
        f'pub const QR_PAYLOAD: &str = "{QR_PAYLOAD}";\n'
        f"pub const QR_VERSION: u8 = {version};\n"
        f"pub const QR_SIZE: i32 = {size};\n"
        "pub const QR_BY_MASK: &[&[&[bool]]] = &[\n" + "\n".join(masks) + "\n];\n"
    )


# --- Critical-count pulse ----------------------------------------------------

def pulse_golden() -> str:
    """The warm-red tint the MLB count dots pulse through.

    `display.py` packs it with `hub75.native.pack_hsv_to_rgb565`, which ships as
    an opaque precompiled `.mpy`. The preview's pure-Python stand-in reproduces
    that helper's documented contract and is verified against the real module on
    device (see `misc_shims`' own docstring), which makes it the best oracle
    available. Every saturation/value pair the pulse can produce is covered:
    there are only 257 of them, one per step of the triangle wave.
    """
    sys.path.insert(0, str(REPO / "tools" / "preview"))
    from shims.misc_shims import pack_hsv_to_rgb565

    rows = []
    for step in range(257):
        value = 191 + ((step * 64) >> 8)
        saturation = (step * 80) >> 8
        packed = pack_hsv_to_rgb565(0, saturation, value)
        rows.append(f"    PulseCase {{ step: {step}, packed: 0x{packed:04x} }},")
    return (
        "/// The count-dot pulse, one entry per step of the triangle wave, as\n"
        "/// the preview's stand-in for `hub75.native.pack_hsv_to_rgb565` packs\n"
        "/// it. `step` is the triangle's value; the saturation and brightness\n"
        "/// the renderer derives from it are part of what is being checked.\n"
        "pub const PULSE_CASES: &[PulseCase] = &[\n" + "\n".join(rows) + "\n];\n"
    )


HEADER = """\
// GENERATED by tests/gen_goldens.py -- do not edit by hand.
//
// Values produced by running the real references: tools/compile_layout.py's
// packers, tools/compile_fonts.py's font builder, display.py's own CPython
// _dim_frame, and the independent `qrcode` package.
#![allow(dead_code)]

pub struct PackedCase {
    pub name: &'static str,
    pub width: usize,
    pub height: usize,
    /// Row-major palette indices that went in.
    pub indices: &'static [u8],
    /// The packed bytes that came out.
    pub packed: &'static [u8],
}

pub struct GlyphGolden {
    pub codepoint: u32,
    pub width: i32,
    pub bits: &'static [u8],
}

pub struct FontGolden {
    pub name: &'static str,
    pub height: i32,
    /// FNV-1a over every codepoint 32..=255 resolved to `(u16 width, bits)` and
    /// concatenated — the whole table, reachable through the public API.
    pub glyphs_fnv: u64,
    pub glyphs: &'static [GlyphGolden],
}

pub struct PulseCase {
    pub step: u32,
    pub packed: u16,
}

pub struct DimCase {
    /// Index into the fade ladder.
    pub step: usize,
    pub dimmed: &'static [u8],
}

/// FNV-1a, 64-bit — the digest the font goldens are recorded with.
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut value: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in data {
        value = (value ^ *byte as u64).wrapping_mul(0x100_0000_01B3);
    }
    value
}
"""


def main() -> None:
    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    sections = [
        HEADER,
        packed_cases(),
        font_goldens(),
        dim_goldens(),
        pulse_golden(),
        qr_golden(),
    ]
    OUT_PATH.write_text("\n".join(sections), encoding="utf-8", newline="\n")
    print(f"wrote {OUT_PATH.relative_to(REPO).as_posix()}")


if __name__ == "__main__":
    main()

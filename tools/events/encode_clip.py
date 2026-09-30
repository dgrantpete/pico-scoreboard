#!/usr/bin/env python3
"""Encode a directory of 128x64 PNG frames into an `.sbev` event clip.

    uv run --with numpy --with pillow tools/events/encode_clip.py <frames_dir> <out.sbev> --fps 30

The format is specified normatively in `crates/scoreboard-render/src/event.rs`
(the decoder the firmware runs); this is its only encoder. In short: one
RGB565 palette of at most 256 colours for the whole clip, then one op stream
per frame that rewrites the previous frame into the next — frame 0 in full,
every later frame as the pixels that changed.

Why this shape and not deflate: the decoder writes straight into the render
loop's existing 16 KB frame surface and needs no other RAM. An inflater needs
~11 KB of tables plus a history window, and the firmware's static-RAM budget
has ~14.5 KB left before it breaks its 40 % headroom target (BUDGET.md).

Besides the clip, this writes `<out>.frames` — one FNV-1a-64 hash per frame of
the RGB565 frame the clip must decode to — which the render crate's test
checks the Rust decoder against, frame by frame. The encoder also decodes its
own output before writing anything, so a clip that reaches disk round-trips.
"""

import argparse
import struct
import sys
from pathlib import Path

try:
    import numpy as np
    from PIL import Image
except ImportError:
    print("numpy and Pillow are required: uv run --with numpy --with pillow ...", file=sys.stderr)
    sys.exit(1)

WIDTH, HEIGHT = 128, 64
PIXELS = WIDTH * HEIGHT
MAGIC = b"SBEV"
VERSION = 1
HEADER = struct.Struct("<4sBBHHBB")  # magic, version, fps, frames, palette_len, width, height
MAX_RUN = 64

OP_SKIP, OP_FILL, OP_LIT = 0x00, 0x40, 0x80


def rgb565(rgb: np.ndarray) -> np.ndarray:
    r, g, b = (rgb[..., i].astype(np.uint16) for i in range(3))
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def fnv1a64(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for byte in data:
        h ^= byte
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def quantize(frames: list[np.ndarray]) -> tuple[np.ndarray, np.ndarray]:
    """One palette for the whole clip. Returns (indices[N, PIXELS], palette565[K])."""
    colours = np.unique(np.concatenate([rgb565(f).ravel() for f in frames]))
    if len(colours) <= 256:
        # Exact: the clip is lossless against its RGB565 source.
        lookup = {int(c): i for i, c in enumerate(colours)}
        index = np.vectorize(lookup.__getitem__, otypes=[np.uint8])
        return np.stack([index(rgb565(f).ravel()) for f in frames]), colours.astype(np.uint16)
    # Median cut over every third frame, stacked — enough to see every colour
    # family, and far cheaper than quantizing the whole clip at once.
    sample = Image.fromarray(np.concatenate(frames[::3], axis=0))
    palette_image = sample.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    indices = np.stack([
        np.asarray(Image.fromarray(f).quantize(palette=palette_image, dither=Image.Dither.NONE)).ravel()
        for f in frames
    ])
    rgb = np.array(palette_image.getpalette()[: 256 * 3], dtype=np.uint8).reshape(-1, 3)
    used = int(indices.max()) + 1
    return indices.astype(np.uint8), rgb565(rgb[:used])


def encode_frame(cur: np.ndarray, prev: np.ndarray | None) -> bytes:
    """Greedy op stream turning `prev` into `cur` (`prev=None`: a full keyframe)."""
    out = bytearray()
    literal: list[int] = []
    n = len(cur)

    def flush() -> None:
        while literal:
            run = literal[:MAX_RUN]
            del literal[:MAX_RUN]
            out.append(OP_LIT | (len(run) - 1))
            out.extend(run)

    i = 0
    while i < n:
        if prev is not None and cur[i] == prev[i]:
            j = i
            while j < n and cur[j] == prev[j]:
                j += 1
            # A one-pixel gap inside a literal is cheaper carried than skipped:
            # 1 byte of index against 1 byte of skip plus a fresh literal header.
            if j - i == 1 and literal and j < n:
                literal.append(int(cur[i]))
                i = j
                continue
            flush()
            while i < j:
                run = min(MAX_RUN, j - i)
                out.append(OP_SKIP | (run - 1))
                i += run
            continue
        j = i
        while j < n and cur[j] == cur[i] and j - i < MAX_RUN:
            j += 1
        if j - i >= 3:
            flush()
            out.append(OP_FILL | (j - i - 1))
            out.append(int(cur[i]))
            i = j
        else:
            literal.append(int(cur[i]))
            i += 1
    flush()
    return bytes(out)


def decode_frame(stream: bytes, surface: np.ndarray, keyframe: bool) -> None:
    """Reference decoder, mirroring `event.rs` — used to verify before writing."""
    at = pos = 0
    while pos < len(stream):
        op = stream[pos]
        run = (op & 0x3F) + 1
        kind = op & 0xC0
        pos += 1
        if kind == OP_SKIP:
            assert not keyframe, "skip in a keyframe"
        elif kind == OP_FILL:
            surface[at:at + run] = stream[pos]
            pos += 1
        elif kind == OP_LIT:
            surface[at:at + run] = list(stream[pos:pos + run])
            pos += run
        else:
            raise AssertionError("reserved op")
        at += run
    assert at == PIXELS, f"frame covers {at} px"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("frames", type=Path, help="directory of f0000.png, f0001.png, ...")
    parser.add_argument("out", type=Path, help="the .sbev to write")
    parser.add_argument("--fps", type=int, required=True, help="must divide the render loop's 60")
    args = parser.parse_args()

    if 60 % args.fps:
        parser.error("fps must divide 60 (time::FPS) so every frame lasts whole ticks")
    paths = sorted(args.frames.glob("f*.png"))
    if not paths:
        parser.error(f"no f*.png frames in {args.frames}")
    frames = [np.asarray(Image.open(p).convert("RGB")) for p in paths]
    for p, f in zip(paths, frames):
        if f.shape != (HEIGHT, WIDTH, 3):
            parser.error(f"{p.name} is {f.shape[1]}x{f.shape[0]}, not {WIDTH}x{HEIGHT}")

    indices, palette = quantize(frames)
    streams = [encode_frame(indices[k], indices[k - 1] if k else None) for k in range(len(indices))]

    # Round-trip before anything touches disk.
    surface = np.zeros(PIXELS, dtype=np.uint8)
    hashes = []
    for k, stream in enumerate(streams):
        decode_frame(stream, surface, keyframe=(k == 0))
        assert np.array_equal(surface, indices[k]), f"frame {k} does not round-trip"
        hashes.append(fnv1a64(palette[surface].astype("<u2").tobytes()))

    offsets = [0]
    for s in streams:
        offsets.append(offsets[-1] + len(s))
    blob = bytearray(HEADER.pack(MAGIC, VERSION, args.fps, len(frames), len(palette), WIDTH, HEIGHT))
    blob += palette.astype("<u2").tobytes()
    blob += struct.pack(f"<{len(offsets)}I", *offsets)
    blob += b"".join(streams)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(blob)
    args.out.with_suffix(args.out.suffix + ".frames").write_text("".join(f"{h:016x}\n" for h in hashes))

    source565 = np.stack([rgb565(f).ravel() for f in frames])
    exact = np.array_equal(palette[indices], source565)
    print(f"{args.out.name}: {len(frames)} frames @ {args.fps} fps, {len(palette)} colours "
          f"({'lossless' if exact else 'quantized'}), {len(blob):,} B "
          f"(largest frame {max(map(len, streams)):,} B)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

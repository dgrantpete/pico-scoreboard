#!/usr/bin/env python3
"""Drive a scoreboard built with `--features diag` over HTTP.

The device shows whatever this tool stages, and hands back what core 1 drew
and how the pack came out, so a panel, a ribbon or a pack bug is cornered one
controlled frame at a time. See firmware-rs/app/src/diag.rs for the contract.

    python tools/diag.py [--host scoreboard.local] COMMAND ...

    pattern NAME            black | white | red | green | blue | lower_blue |
                            upper_blue | halves | bars
    fill X Y W H R G B      paint a rectangle onto the staged frame
    push IMAGE              any image file -> 128x64 RGB565, staged in chunks
    shot OUT.png            the frame core 1 last drew, as a PNG (+ a summary)
    lanes [EXPECT]          set-bit counts per data lane over the live planes;
                            EXPECT is a pattern name to compare against
    led TOGGLES [MS]        an IOCTL storm on the LED task
    exit                    hand the panel back to the scoreboard
    status                  /api/status, pretty-printed

Requires Pillow for `push` and `shot`.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
import urllib.error
import urllib.request

WIDTH, HEIGHT = 128, 64
FRAME_BYTES = WIDTH * HEIGHT * 2
# The device's HTTP request buffer is 4 KB including headers.
CHUNK = 2048
LANES = ("r1", "g1", "b1", "r2", "g2", "b2")


def request(host: str, method: str, path: str, body: bytes | None = None, content_type: str | None = None) -> tuple[int, bytes]:
    req = urllib.request.Request(f"http://{host}{path}", data=body, method=method)
    if content_type:
        req.add_header("Content-Type", content_type)
    try:
        with urllib.request.urlopen(req, timeout=15) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def say(status: int, body: bytes) -> None:
    text = body.decode(errors="replace")
    print(f"HTTP {status} {text}")
    if status >= 400:
        sys.exit(1)


def rgb565(r: int, g: int, b: int) -> int:
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def cmd_pattern(args: argparse.Namespace) -> None:
    say(*request(args.host, "POST", "/api/diag/fill", json.dumps({"pattern": args.name}).encode(), "application/json"))


def cmd_fill(args: argparse.Namespace) -> None:
    body = {"x": args.x, "y": args.y, "w": args.w, "h": args.h, "rgb": [args.r, args.g, args.b]}
    say(*request(args.host, "POST", "/api/diag/fill", json.dumps(body).encode(), "application/json"))


def image_to_rgb565(path: str) -> bytes:
    from PIL import Image

    image = Image.open(path).convert("RGB")
    if image.size != (WIDTH, HEIGHT):
        image = image.resize((WIDTH, HEIGHT), Image.NEAREST)
    out = bytearray()
    for r, g, b in image.getdata():
        out += struct.pack("<H", rgb565(r, g, b))
    return bytes(out)


def cmd_push(args: argparse.Namespace) -> None:
    frame = image_to_rgb565(args.image)
    assert len(frame) == FRAME_BYTES
    for offset in range(0, FRAME_BYTES, CHUNK):
        status, body = request(
            args.host, "PUT", f"/api/diag/frame?offset={offset}", frame[offset : offset + CHUNK], "application/octet-stream"
        )
        if status != 200:
            say(status, body)
    print(f"pushed {args.image} as {FRAME_BYTES} B in {FRAME_BYTES // CHUNK} chunks")


def summarize(frame: bytes) -> None:
    pixels = struct.unpack(f"<{WIDTH * HEIGHT}H", frame)
    for half, rows in (("upper", range(0, HEIGHT // 2)), ("lower", range(HEIGHT // 2, HEIGHT))):
        red = green = blue = 0
        for y in rows:
            for x in range(WIDTH):
                p = pixels[y * WIDTH + x]
                red += (p >> 11) != 0
                green += ((p >> 5) & 0x3F) != 0
                blue += (p & 0x1F) != 0
        total = WIDTH * HEIGHT // 2
        print(f"{half} half: pixels with red {red}/{total}, green {green}/{total}, blue {blue}/{total}")


def cmd_shot(args: argparse.Namespace) -> None:
    status, body = request(args.host, "GET", "/api/diag/frame")
    if status != 200 or len(body) != FRAME_BYTES:
        print(f"HTTP {status}, {len(body)} B: core 1 did not answer with a frame")
        sys.exit(1)
    from PIL import Image

    image = Image.new("RGB", (WIDTH, HEIGHT))
    pixels = struct.unpack(f"<{WIDTH * HEIGHT}H", body)
    image.putdata([(((p >> 11) & 0x1F) << 3, ((p >> 5) & 0x3F) << 2, (p & 0x1F) << 3) for p in pixels])
    scale = max(1, args.scale)
    if scale > 1:
        image = image.resize((WIDTH * scale, HEIGHT * scale), Image.NEAREST)
    image.save(args.out)
    print(f"saved {args.out} ({WIDTH * scale}x{HEIGHT * scale})")
    summarize(body)


def cmd_lanes(args: argparse.Namespace) -> None:
    status, body = request(args.host, "GET", "/api/diag/lanes")
    if status != 200:
        say(status, body)
    counts = json.loads(body)
    total = counts["bytes"]
    for lane in LANES:
        print(f"{lane}: {counts[lane]:>7} set bits  ({100.0 * counts[lane] / total:5.1f} % of {total} bytes)")
    if args.expect:
        # Lanes that a pattern leaves entirely dark must count zero: that is
        # the pack being right. A non-zero count on a dark lane is the pack
        # (or something writing its buffer); a zero count under a lit panel
        # lane is the wire or the panel.
        dark = {
            "black": set(LANES),
            "white": set(),
            "red": {"g1", "b1", "g2", "b2"},
            "green": {"r1", "b1", "r2", "b2"},
            "blue": {"r1", "g1", "r2", "g2"},
            "lower_blue": {"r1", "g1", "b1", "r2", "g2"},
            "upper_blue": {"r1", "g1", "r2", "g2", "b2"},
            "halves": {"r2", "g2", "b2"},
        }.get(args.expect)
        if dark is None:
            print(f"no expectation table for pattern {args.expect!r}")
            return
        wrong = [lane for lane in sorted(dark) if counts[lane] != 0]
        if wrong:
            print(f"PACK WRONG: {', '.join(wrong)} should be dark for {args.expect} but carry set bits")
            sys.exit(2)
        print(f"pack consistent with {args.expect}: every dark lane counts zero")


def cmd_led(args: argparse.Namespace) -> None:
    body = {"toggles": args.toggles, "interval_ms": args.ms}
    say(*request(args.host, "POST", "/api/diag/led", json.dumps(body).encode(), "application/json"))


def cmd_exit(args: argparse.Namespace) -> None:
    say(*request(args.host, "POST", "/api/diag/exit", b"", "application/json"))


def cmd_status(args: argparse.Namespace) -> None:
    status, body = request(args.host, "GET", "/api/status")
    if status != 200:
        say(status, body)
    print(json.dumps(json.loads(body), indent=1))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default="scoreboard.local")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("pattern")
    p.add_argument("name")
    p.set_defaults(run=cmd_pattern)

    p = sub.add_parser("fill")
    for name in ("x", "y", "w", "h", "r", "g", "b"):
        p.add_argument(name, type=int)
    p.set_defaults(run=cmd_fill)

    p = sub.add_parser("push")
    p.add_argument("image")
    p.set_defaults(run=cmd_push)

    p = sub.add_parser("shot")
    p.add_argument("out")
    p.add_argument("--scale", type=int, default=4)
    p.set_defaults(run=cmd_shot)

    p = sub.add_parser("lanes")
    p.add_argument("expect", nargs="?")
    p.set_defaults(run=cmd_lanes)

    p = sub.add_parser("led")
    p.add_argument("toggles", type=int)
    p.add_argument("ms", type=int, nargs="?", default=50)
    p.set_defaults(run=cmd_led)

    sub.add_parser("exit").set_defaults(run=cmd_exit)
    sub.add_parser("status").set_defaults(run=cmd_status)

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()

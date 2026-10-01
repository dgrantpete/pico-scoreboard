#!/usr/bin/env python3
"""Colin's birthday (2026-09-30) — the source of the `colin-birthday-2026` event clip.

Draws every frame procedurally at the panel's native 128x64 and writes them as
PNGs; `encode_clip.py` turns that directory into the `.sbev` clip the firmware
embeds (see `firmware-rs/app/assets/events/README.md`). This script is the
editable source, the same way `gen_toast_icons.py` is for the toast icons.

    uv run --with numpy --with pillow tools/events/colin_birthday_2026.py <out_dir>

Two things are deliberately *not* in the frames, because the player does them
at playback and baking them in costs flash:

* **The fade in and out.** The player ramps the panel's brightness instead
  (`scoreboard_render::event`). A baked fade rewrites every pixel of ~36
  frames, which measured at ~150 KB of the clip.
* **Glow flicker.** The flames still flicker frame to frame, but the dithered
  candle glow holds a steady intensity: a flickering glow re-dithers a
  ~30x30 px area every frame, for no difference anyone sees on the panel.

The frame rate is 30 because the render loop runs at 60 (`time::FPS`) and a
clip frame must last a whole number of ticks — 25 fps would show frames for
alternately 2 and 3 ticks, which reads as judder.
"""
import math
import random
import sys
from pathlib import Path

import numpy as np
from PIL import Image

W, H = 128, 64
FPS = 30
DUR = 13.0
NF = int(round(FPS * DUR))
DT = 1.0 / FPS

BAYER4 = (np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]) + 0.5) / 16.0


# ----------------------------------------------------------------------------- helpers
def clamp01(x):
    return max(0.0, min(1.0, x))


def lerp(a, b, t):
    return a + (b - a) * t


def smooth(t):
    t = clamp01(t)
    return t * t * (3 - 2 * t)


def ease_out_back(t, s=2.2):
    t = clamp01(t) - 1
    return 1 + t * t * ((s + 1) * t + s)


def ease_out_bounce(t):
    t = clamp01(t)
    n1, d1 = 7.5625, 2.75
    if t < 1 / d1:
        return n1 * t * t
    if t < 2 / d1:
        t -= 1.5 / d1
        return n1 * t * t + 0.75
    if t < 2.5 / d1:
        t -= 2.25 / d1
        return n1 * t * t + 0.9375
    t -= 2.625 / d1
    return n1 * t * t + 0.984375


def C(*c):
    return np.array(c, dtype=float)


def mix(a, b, t):
    return np.asarray(a, float) * (1 - t) + np.asarray(b, float) * t


class Canvas:
    def __init__(self, base=None):
        self.a = np.zeros((H, W, 3)) if base is None else base.copy()

    def px(self, x, y, c, alpha=1.0):
        x, y = int(round(x)), int(round(y))
        if 0 <= x < W and 0 <= y < H and alpha > 0:
            if alpha >= 1:
                self.a[y, x] = c
            else:
                self.a[y, x] = self.a[y, x] * (1 - alpha) + np.asarray(c, float) * alpha

    def add(self, x, y, c, k):
        x, y = int(round(x)), int(round(y))
        if 0 <= x < W and 0 <= y < H:
            self.a[y, x] += np.asarray(c, float) * k

    def blit(self, spr, x, y, alpha=1.0, tint=None):
        """spr: (h, w, 4) float array, alpha channel 0..1. (x, y) = top-left."""
        x, y = int(round(x)), int(round(y))
        h, w = spr.shape[:2]
        x0, y0 = max(0, x), max(0, y)
        x1, y1 = min(W, x + w), min(H, y + h)
        if x0 >= x1 or y0 >= y1:
            return
        s = spr[y0 - y:y1 - y, x0 - x:x1 - x]
        rgb = s[..., :3]
        if tint is not None:
            rgb = rgb * (1 - tint[1]) + np.asarray(tint[0], float) * tint[1]
        a = s[..., 3:4] * alpha
        d = self.a[y0:y1, x0:x1]
        self.a[y0:y1, x0:x1] = d * (1 - a) + rgb * a

    def line(self, x0, y0, x1, y1, c, alpha=1.0):
        n = int(max(abs(x1 - x0), abs(y1 - y0))) + 1
        seen = set()
        for i in range(n + 1):
            t = i / max(1, n)
            p = (int(round(lerp(x0, x1, t))), int(round(lerp(y0, y1, t))))
            if p not in seen:
                seen.add(p)
                self.px(p[0], p[1], c, alpha)


def sprite_from_ascii(rows, pal):
    h, w = len(rows), max(len(r) for r in rows)
    s = np.zeros((h, w, 4))
    for y, r in enumerate(rows):
        for x, ch in enumerate(r):
            if ch in pal:
                s[y, x, :3] = pal[ch]
                s[y, x, 3] = 1
    return s


# ----------------------------------------------------------------------------- palette
# Hue-shifted 5-step ramps: [outline, shadow, base, light, highlight]
RAMPS = {
    "red": [C(92, 10, 48), C(178, 22, 58), C(238, 52, 64), C(255, 128, 112), C(255, 232, 220)],
    "orange": [C(110, 34, 20), C(214, 88, 22), C(255, 146, 38), C(255, 202, 110), C(255, 246, 222)],
    "yellow": [C(120, 58, 16), C(212, 142, 18), C(255, 204, 40), C(255, 238, 128), C(255, 255, 232)],
    "green": [C(12, 64, 58), C(22, 148, 74), C(64, 212, 96), C(166, 246, 142), C(242, 255, 232)],
    "blue": [C(22, 24, 96), C(32, 72, 196), C(52, 134, 248), C(126, 198, 255), C(232, 246, 255)],
    "purple": [C(50, 16, 96), C(112, 42, 184), C(172, 84, 242), C(216, 162, 255), C(250, 236, 255)],
    "pink": [C(100, 18, 74), C(204, 48, 134), C(255, 102, 182), C(255, 172, 216), C(255, 242, 250)],
    "teal": [C(10, 60, 80), C(20, 140, 160), C(40, 210, 210), C(150, 245, 235), C(235, 255, 252)],
}

CONFETTI_COLS = [C(255, 84, 96), C(255, 212, 60), C(84, 224, 120), C(84, 172, 255),
                 C(204, 112, 255), C(255, 142, 204), C(255, 255, 255), C(255, 152, 52),
                 C(60, 225, 215)]

SKY_BANDS = [C(6, 5, 24), C(12, 9, 36), C(20, 12, 48), C(30, 15, 58), C(40, 17, 66), C(50, 19, 72)]


# ----------------------------------------------------------------------------- background
def make_background():
    bg = np.zeros((H, W, 3))
    n = len(SKY_BANDS) - 1
    for y in range(H):
        f = (y / (H - 1)) ** 1.1 * n
        i = min(int(f), n - 1)
        fr = f - i
        for x in range(W):
            bg[y, x] = SKY_BANDS[i + 1] if fr > BAYER4[y % 4, x % 4] else SKY_BANDS[i]
    # table: top highlight + candy-stripe tablecloth
    for x in range(W):
        bg[61, x] = C(255, 226, 238)
        stripe = ((x // 3) % 2) == 0
        bg[62, x] = C(255, 128, 176) if stripe else C(250, 240, 248)
        bg[63, x] = C(196, 72, 138) if stripe else C(200, 180, 210)
    return bg


rng_star = random.Random(7)
STARS = []
while len(STARS) < 34:
    x, y = rng_star.randrange(W), rng_star.randrange(0, 46)
    if all(abs(x - sx) + abs(y - sy) > 5 for sx, sy, *_ in STARS):
        STARS.append((x, y, rng_star.random() * 6.28, 1.5 + rng_star.random() * 2.5,
                      0.35 + rng_star.random() * 0.65, rng_star.random() < 0.18))


def draw_stars(cv, t):
    for x, y, ph, sp, br, big in STARS:
        k = br * (0.55 + 0.45 * math.sin(t * sp + ph))
        col = C(255, 250, 225) if big else C(200, 210, 255)
        cv.px(x, y, col, clamp01(k))
        if big and k > 0.7:
            a = (k - 0.7) / 0.3 * 0.6
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                cv.px(x + dx, y + dy, C(170, 190, 255), a)


# ----------------------------------------------------------------------------- balloons
def make_balloon(bw, bh, ramp):
    """Returns (sprite, knot_x, knot_y) — knot = where the string starts."""
    L = np.array([-0.52, -0.66, 0.54])
    L /= np.linalg.norm(L)
    sh = bh + 2
    spr = np.zeros((sh, bw, 4))
    cx = bw / 2
    top_r = bh * 0.47
    bot_r = bh - top_r
    inside = np.zeros((bh, bw), bool)
    shade = np.zeros((bh, bw))
    uv = np.zeros((bh, bw, 2))
    for y in range(bh):
        for x in range(bw):
            px_, py_ = x + 0.5, y + 0.5
            v = (py_ - top_r) / (top_r if py_ < top_r else bot_r)
            narrow = 1 - 0.30 * max(0.0, v) ** 1.6
            u = (px_ - cx) / (bw / 2 * narrow)
            r2 = u * u + v * v
            if r2 <= 1.0:
                inside[y, x] = True
                nz = math.sqrt(max(0.0, 1 - r2))
                shade[y, x] = u * L[0] + v * L[1] + nz * L[2]
                uv[y, x] = (u, v)
    for y in range(bh):
        for x in range(bw):
            if not inside[y, x]:
                continue
            edge = any(not (0 <= x + dx < bw and 0 <= y + dy < bh and inside[y + dy, x + dx])
                       for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))
            s = shade[y, x]
            u, v = uv[y, x]
            if edge:
                col = ramp[1] if s > 0.30 else ramp[0]
            elif s > 0.80:
                col = ramp[3]
            elif s > 0.38:
                col = ramp[2]
            elif s > 0.02:
                col = ramp[1]
            else:
                col = ramp[0]
            # bounce light on lower-right rim
            if not edge and u > 0.55 and v > 0.25 and s <= 0.38:
                col = mix(ramp[1], ramp[2], 0.5)
            if (u + 0.40) ** 2 / 0.045 + (v + 0.45) ** 2 / 0.07 < 1:
                col = ramp[4]
            spr[y, x, :3] = col
            spr[y, x, 3] = 1
    k = bw // 2
    # knot
    spr[bh - 1, k, :3] = ramp[1]
    spr[bh - 1, k, 3] = 1
    spr[bh, k, :3] = ramp[1]
    spr[bh, k, 3] = 1
    for dx in (-1, 1):
        spr[bh + 1, k + dx, :3] = ramp[0]
        spr[bh + 1, k + dx, 3] = 1
    spr[bh + 1, k, :3] = ramp[1]
    spr[bh + 1, k, 3] = 1
    return spr, k, bh + 2


BALLOON_CACHE = {}


def balloon(size, color):
    key = (size, color)
    if key not in BALLOON_CACHE:
        bw, bh = {"S": (7, 9), "M": (9, 11), "L": (11, 13), "XL": (13, 15)}[size]
        BALLOON_CACHE[key] = make_balloon(bw, bh, RAMPS[color])
    return BALLOON_CACHE[key]


STRING_COL = C(196, 196, 220)


def draw_hanging_string(cv, kx, ky, length, t, phase, alpha=0.65, drift=0.0):
    prev = None
    for i in range(int(length) + 1):
        s = i / max(1, length)
        x = kx + math.sin(s * 3.4 + t * 4.0 + phase) * 1.6 * s + drift * s * s
        y = ky + i
        p = (int(round(x)), int(round(y)))
        if prev is not None and abs(p[0] - prev[0]) > 1:
            cv.line(prev[0], prev[1], p[0], p[1], STRING_COL, alpha)
        cv.px(p[0], p[1], STRING_COL, alpha)
        prev = p


def draw_tied_string(cv, kx, ky, ax, ay, rest_len, t, phase, alpha=0.65):
    d = math.hypot(ax - kx, ay - ky)
    sag = math.sqrt(max(0.0, rest_len ** 2 - d ** 2)) * 0.55
    mx, my = (kx + ax) / 2, (ky + ay) / 2 + sag
    n = int(max(d, 8) * 1.6)
    pts = []
    for i in range(n + 1):
        s = i / n
        x = (1 - s) ** 2 * kx + 2 * (1 - s) * s * mx + s * s * ax
        y = (1 - s) ** 2 * ky + 2 * (1 - s) * s * my + s * s * ay
        x += math.sin(s * math.pi) * math.sin(t * 2.2 + phase) * 0.8
        pts.append((int(round(x)), int(round(y))))
    seen = set()
    for p in pts:
        if p not in seen:
            seen.add(p)
            cv.px(p[0], p[1], STRING_COL, alpha)


# loose balloons for the intro "release" — (t0, x0, speed, size, color, depth, sway, phase)
rng_b = random.Random(3)
LOOSE = []
_specs = [
    (0.15, 18, 30, "S", "teal", "far"), (0.35, 60, 34, "M", "red", "near"),
    (0.55, 100, 28, "S", "purple", "far"), (0.80, 36, 38, "L", "yellow", "near"),
    (1.00, 84, 31, "S", "pink", "far"), (1.20, 118, 40, "M", "green", "near"),
    (1.45, 8, 33, "M", "blue", "near"), (1.65, 72, 29, "S", "orange", "far"),
    (1.90, 48, 41, "XL", "pink", "near"), (2.20, 104, 36, "M", "orange", "near"),
    (2.45, 26, 30, "S", "green", "far"), (2.70, 90, 43, "L", "purple", "near"),
    (3.00, 58, 32, "S", "blue", "far"),
]
for t0, x0, sp, size, col, depth in _specs:
    LOOSE.append(dict(t0=t0, x0=x0, sp=sp, size=size, col=col, depth=depth,
                      sway=1.5 + rng_b.random() * 2.0, ph=rng_b.random() * 6.28,
                      w=1.2 + rng_b.random() * 1.2, slen=12 + rng_b.randrange(8)))


def draw_loose(cv, t, depth):
    for b in LOOSE:
        if b["depth"] != depth or t < b["t0"]:
            continue
        spr, kx, ky = balloon(b["size"], b["col"])
        age = t - b["t0"]
        y = H + 2 - age * b["sp"]
        x = b["x0"] + math.sin(age * b["w"] + b["ph"]) * b["sway"]
        if y + spr.shape[0] + b["slen"] < -2:
            continue
        top_x = int(round(x - spr.shape[1] / 2))
        top_y = int(round(y))
        tint = (C(30, 16, 62), 0.42) if depth == "far" else None
        salpha = 0.35 if depth == "far" else 0.6
        draw_hanging_string(cv, top_x + kx, top_y + ky, b["slen"], t, b["ph"], salpha)
        cv.blit(spr, top_x, top_y, tint=tint)


# ----------------------------------------------------------------------------- gifts
def make_gift(w, h, box, ribbon, lid_extra=1):
    """box/ribbon: ramps. returns sprite with bow on top; sprite height = h + 3."""
    sw = w + 2 * lid_extra
    s = np.zeros((h + 3, sw, 4))

    def put(x, y, c):
        if 0 <= x < sw and 0 <= y < h + 3:
            s[y, x, :3] = c
            s[y, x, 3] = 1

    rc = sw // 2  # ribbon centre column
    # body
    for y in range(3 + 2, 3 + h):
        for x in range(lid_extra, lid_extra + w):
            xr = (x - lid_extra) / (w - 1)
            c = box[3] if xr < 0.18 else (box[2] if xr < 0.72 else box[1])
            if x == lid_extra + w - 1 or y == 3 + h - 1:
                c = box[0] if y == 3 + h - 1 else box[1]
            put(x, y, c)
    # lid (2 rows, 1px wider each side)
    for y in (3, 4):
        for x in range(sw):
            xr = x / (sw - 1)
            c = box[3] if y == 3 else (box[2] if xr < 0.75 else box[1])
            if x == 0 or x == sw - 1:
                c = box[1] if y == 3 else box[0]
            put(x, y, c)
    # shadow under lid
    for x in range(lid_extra, lid_extra + w):
        put(x, 5, box[1] if x < lid_extra + w - 1 else box[0])
    # vertical ribbon
    for y in range(3, 3 + h):
        put(rc - 1, y, ribbon[3] if y > 4 else ribbon[4])
        put(rc, y, ribbon[2])
        put(rc + 1, y, ribbon[1])
    # bow
    bow = ["##.##", "#.#.#", ".#.#."]
    bow = [".#.#.", "#o#o#", ".###."]
    pal = {"#": ribbon[2], "o": ribbon[3]}
    for y, row in enumerate(bow):
        for x, ch in enumerate(row):
            if ch in pal:
                put(rc - 2 + x, y, pal[ch])
    put(rc - 1, 0, ribbon[3])
    put(rc + 1, 0, ribbon[3])
    put(rc, 2, ribbon[4])
    return s


GIFT_A = make_gift(13, 10, RAMPS["blue"], RAMPS["yellow"])
GIFT_B = make_gift(9, 7, RAMPS["pink"], RAMPS["green"])
GIFT_C = make_gift(9, 8, RAMPS["purple"], RAMPS["teal"])  # balloon anchor (right)
GIFT_A_POS = (3, 61 - GIFT_A.shape[0])
GIFT_B_POS = (19, 61 - GIFT_B.shape[0])
GIFT_C_POS = (116, 61 - GIFT_C.shape[0])
ANCHOR = (GIFT_C_POS[0] + GIFT_C.shape[1] // 2, GIFT_C_POS[1] + 1)


# ----------------------------------------------------------------------------- cake
CAKE_W = 39
CAKE_H = 32
CANDLE_COLS = ["red", "yellow", "green", "blue", "purple"]
CANDLE_X = [10, 14, 18, 22, 26]  # left col of each 3-wide candle (sprite coords)


def make_cake():
    s = np.zeros((CAKE_H, CAKE_W, 4))

    def put(x, y, c):
        if 0 <= x < CAKE_W and 0 <= y < CAKE_H:
            s[y, x, :3] = c
            s[y, x, 3] = 1

    def tier(x0, x1, y0, y1, body, drips, frost, sprinkles):
        w = x1 - x0 + 1
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                xr = (x - x0) / (w - 1)
                c = body[3] if xr < 0.10 else body[2] if xr < 0.70 else body[1] if xr < 0.94 else body[0]
                put(x, y, c)
        # frosting top (2 rows) + drips
        for i, x in enumerate(range(x0, x1 + 1)):
            xr = (x - x0) / (w - 1)
            fc = frost[3] if xr < 0.80 else frost[2]
            put(x, y0, frost[4] if xr < 0.6 else frost[3])
            put(x, y0 + 1, fc)
            d = drips[i % len(drips)]
            for k in range(d):
                c = fc if k < d - 1 else (frost[2] if xr < 0.8 else frost[1])
                put(x, y0 + 2 + k, c)
        put(x0 - 1, y0, frost[3]); put(x0 - 1, y0 + 1, frost[2])
        put(x1 + 1, y0, frost[2]); put(x1 + 1, y0 + 1, frost[1])
        # piping beads along the bottom
        for i, x in enumerate(range(x0, x1 + 1)):
            xr = (x - x0) / (w - 1)
            put(x, y1, frost[3] if xr < 0.8 else frost[2])
            if i % 3 != 2:
                put(x, y1 - 1, frost[3] if xr < 0.8 else frost[2])
            if i % 3 == 0:
                put(x, y1 - 1, frost[4] if xr < 0.8 else frost[3])
        for (x, y, c) in sprinkles:
            put(x0 + x, y0 + y, c)

    frost = [C(120, 100, 140), C(190, 176, 212), C(228, 220, 240), C(252, 248, 252), C(255, 255, 255)]
    pink = [C(120, 30, 90), C(206, 70, 132), C(250, 112, 164), C(255, 170, 204)]
    lav = [C(80, 52, 150), C(128, 92, 210), C(176, 140, 250), C(218, 190, 255)]

    sp_bot = [(3, 7, CONFETTI_COLS[1]), (7, 9, CONFETTI_COLS[3]), (11, 6, CONFETTI_COLS[2]),
              (14, 9, CONFETTI_COLS[8]), (18, 7, CONFETTI_COLS[1]), (21, 9, CONFETTI_COLS[6]),
              (25, 6, CONFETTI_COLS[3]), (28, 8, CONFETTI_COLS[2]), (5, 10, CONFETTI_COLS[6]),
              (16, 10, CONFETTI_COLS[4]), (23, 7, CONFETTI_COLS[4]), (9, 7, CONFETTI_COLS[6])]
    drips_bot = [2, 3, 3, 2, 1, 1, 3, 4, 4, 3, 1, 2, 2, 1, 2, 4, 5, 4, 2, 1, 1, 3, 3, 2, 1, 2, 4, 4, 3, 1, 1, 2, 3]
    tier(3, 35, 17, 29, pink, drips_bot, frost, sp_bot)

    # top tier: lavender with white drips and a row of cherries/hearts
    drips_top = [1, 1, 2, 2, 1, 0, 1, 2, 1, 0, 1, 1, 2, 2, 1, 0, 1, 2, 1, 1, 0]
    tier(9, 29, 8, 16, lav, drips_top, frost, [])
    # hearts row on top tier
    for hx in (12, 17, 22, 27):
        heart = ["#.#", "###", ".#."]
        for yy, r in enumerate(heart):
            for xx, ch in enumerate(r):
                if ch == "#":
                    put(hx - 1 + xx, 12 + yy, C(255, 90, 140) if not (xx == 0 and yy == 0) else C(255, 190, 210))

    # plate
    for x in range(CAKE_W):
        edge = x in (0, CAKE_W - 1)
        put(x, 30, C(196, 200, 228) if edge else (C(250, 250, 255) if x < 28 else C(216, 220, 240)))
        if not edge:
            put(x, 31, C(140, 144, 186) if x < 30 else C(110, 112, 160))

    # candles (rows 1..7), wick row 0 drawn dynamically
    for ci, cx in enumerate(CANDLE_X):
        r = RAMPS[CANDLE_COLS[ci]]
        for y in range(1, 8):
            for dx in range(3):
                stripe = ((y + dx) % 4) in (0, 1)
                c = C(255, 252, 245) if stripe else r[2]
                if dx == 0:
                    c = mix(c, C(255, 255, 255), 0.35)
                if dx == 2:
                    c = c * 0.72
                put(cx + dx, y, c)
    return s


CAKE = make_cake()
CAKE_X = 74
CAKE_REST_Y = 61 - CAKE_H  # plate bottom sits on row 60


FLAME_FRAMES = [
    [".R.", ".O.", "OYO", "OWO", "OWO", ".O."],
    ["R..", ".O.", "OYO", "OWO", "OWO", ".O."],
    ["..R", ".O.", "OYO", "OWO", "OWO", ".O."],
    ["...", ".R.", "OOO", "OWO", "OWO", ".O."],
    [".R.", ".O.", ".YO", "OWO", "OWO", ".O."],
]
FLAME_GROW = [
    ["...", "...", "...", "...", ".Y.", ".O."],
    ["...", "...", ".R.", ".O.", "OWO", ".O."],
    ["...", ".R.", ".O.", "OYO", "OWO", ".O."],
]
FLAME_BLOW = [
    ["...", "..R", ".OO", "OYO", "OW.", ".O."],
    ["...", "...", "..R", ".OO", ".WO", ".O."],
    ["...", "...", "...", "..R", ".OO", ".O."],
    ["...", "...", "...", "...", "..R", ".O."],
]
FLAME_PAL = {"R": C(236, 84, 40), "O": C(255, 156, 40), "Y": C(255, 226, 110), "W": C(255, 252, 228)}
FLAME_SPR = [sprite_from_ascii(f, FLAME_PAL) for f in FLAME_FRAMES]
GROW_SPR = [sprite_from_ascii(f, FLAME_PAL) for f in FLAME_GROW]
BLOW_SPR = [sprite_from_ascii(f, FLAME_PAL) for f in FLAME_BLOW]


# ----------------------------------------------------------------------------- fonts
SMALL = {
    "H": ["##..##", "##..##", "##..##", "######", "##..##", "##..##", "##..##"],
    "A": [".####.", "##..##", "##..##", "######", "##..##", "##..##", "##..##"],
    "P": ["#####.", "##..##", "##..##", "#####.", "##....", "##....", "##...."],
    "Y": ["##..##", "##..##", "##..##", ".####.", "..##..", "..##..", "..##.."],
    "B": ["#####.", "##..##", "##..##", "#####.", "##..##", "##..##", "#####."],
    "I": ["####", ".##.", ".##.", ".##.", ".##.", ".##.", "####"],
    "R": ["#####.", "##..##", "##..##", "#####.", "##.##.", "##..##", "##..##"],
    "T": ["######", "..##..", "..##..", "..##..", "..##..", "..##..", "..##.."],
    "D": ["#####.", "##..##", "##..##", "##..##", "##..##", "##..##", "#####."],
}

BIG = {
    "C": ["..#####..", ".#######.", "###...###", "##.......", "##.......", "##.......",
          "##.......", "##.......", "##.......", "###...###", ".#######.", "..#####.."],
    "O": ["..#####..", ".#######.", "###...###", "##.....##", "##.....##", "##.....##",
          "##.....##", "##.....##", "##.....##", "###...###", ".#######.", "..#####.."],
    "L": ["##......", "##......", "##......", "##......", "##......", "##......",
          "##......", "##......", "##......", "##......", "########", "########"],
    "I": ["######", "######", "..##..", "..##..", "..##..", "..##..",
          "..##..", "..##..", "..##..", "..##..", "######", "######"],
    "N": ["###....##", "####...##", "####...##", "##.##..##", "##.##..##", "##.##..##",
          "##..##.##", "##..##.##", "##..##.##", "##...####", "##...####", "##....###"],
    "!": ["##", "##", "##", "##", "##", "##", "##", "##", "..", "..", "##", "##"],
    "B": ["#######..", "########.", "##....###", "##.....##", "##....###", "#######..",
          "########.", "##....###", "##.....##", "##....###", "########.", "#######.."],
    "A": ["..#####..", ".#######.", "###...###", "##.....##", "##.....##", "#########",
          "#########", "##.....##", "##.....##", "##.....##", "##.....##", "##.....##"],
    "K": ["##....###", "##...###.", "##..###..", "##.###...", "#####....", "####.....",
          "####.....", "#####....", "##.###...", "##..###..", "##...###.", "##....###"],
    "E": ["########", "########", "##......", "##......", "##......", "#######.",
          "#######.", "##......", "##......", "##......", "########", "########"],
}


def make_glyph(rows, fill_fn, outline, shadow, shadow_depth=1):
    """Returns (sprite, fillmask) with a 1px outline and a drop shadow below."""
    gh, gw = len(rows), len(rows[0])
    m = np.array([[ch == "#" for ch in r.ljust(gw, ".")] for r in rows])
    sh, sw = gh + 2 + shadow_depth, gw + 2
    mask = np.zeros((sh, sw), bool)
    mask[1:1 + gh, 1:1 + gw] = m
    dil = mask.copy()
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            dil |= np.roll(np.roll(mask, dy, 0), dx, 1)
    shadow_m = np.zeros_like(dil)
    for k in range(1, shadow_depth + 1):
        shadow_m |= np.roll(dil, k, 0)
    spr = np.zeros((sh, sw, 4))
    spr[shadow_m, :3] = shadow
    spr[shadow_m, 3] = 1
    spr[dil, :3] = outline
    spr[dil, 3] = 1
    for y in range(sh):
        for x in range(sw):
            if mask[y, x]:
                spr[y, x, :3] = fill_fn(y - 1, x - 1, gh, m)
    return spr, mask


def small_fill(y, x, gh, m):
    grad = [C(255, 255, 255), C(255, 255, 248), C(255, 248, 214), C(255, 236, 170),
            C(255, 220, 128), C(255, 196, 92), C(252, 170, 70)]
    return grad[y]


def big_fill_factory(ramp):
    def f(y, x, gh, m):
        if y <= 2:
            c = ramp[3]
        elif y <= 7:
            c = ramp[2]
        else:
            c = ramp[1]
        # top-left specular: first two filled pixels of a run in the top rows
        if y <= 2 and x > 0 and m[y, x - 1] == 0:
            c = ramp[4]
        if y == 3 and x > 0 and m[y, x - 1] == 0:
            c = mix(ramp[3], ramp[4], 0.5)
        return c
    return f


OUT_SMALL, SHD_SMALL = C(46, 12, 72), C(18, 5, 34)
OUT_BIG, SHD_BIG = C(26, 6, 44), C(12, 3, 24)


def layout(word, font, gap, cx):
    widths = [len(font[ch][0]) for ch in word]
    total = sum(widths) + gap * (len(word) - 1)
    x = cx - total // 2
    xs = []
    for wdt in widths:
        xs.append(x)
        x += wdt + gap
    return xs


TEXT_CX = 34
HAPPY = [(ch, make_glyph(SMALL[ch], small_fill, OUT_SMALL, SHD_SMALL)) for ch in "HAPPY"]
BIRTH = [(ch, make_glyph(SMALL[ch], small_fill, OUT_SMALL, SHD_SMALL)) for ch in "BIRTHDAY"]
HAPPY_X = layout("HAPPY", SMALL, 1, TEXT_CX)
BIRTH_X = layout("BIRTHDAY", SMALL, 1, TEXT_CX)
HAPPY_Y, BIRTH_Y = 3, 13
COLIN_COLS = ["red", "orange", "green", "blue", "purple", "pink"]
COLIN = [(ch, make_glyph(BIG[ch], big_fill_factory(RAMPS[c]), OUT_BIG, SHD_BIG, 2))
         for ch, c in zip("BLAKE!", COLIN_COLS)]
COLIN_X = layout("BLAKE!", BIG, 2, TEXT_CX)
COLIN_Y = 25


def blit_glyph(cv, glyph, x, y, flash=0.0, shine=None):
    spr, mask = glyph
    cv.blit(spr, x - 1, y - 1)
    if flash > 0 or shine is not None:
        h, w = mask.shape
        for yy in range(h):
            for xx in range(w):
                if not mask[yy, xx]:
                    continue
                X, Y = x - 1 + xx, y - 1 + yy
                if flash > 0:
                    cv.px(X, Y, C(255, 255, 255), flash)
                if shine is not None:
                    d = (X + Y * 0.6) - shine
                    if -1.5 <= d <= 1.5:
                        cv.px(X, Y, C(255, 255, 255), 0.75 if abs(d) <= 0.5 else 0.4)


# ----------------------------------------------------------------------------- effects
def sparkle(cv, x, y, p, col=C(255, 255, 220)):
    """4-point twinkle, p in [0,1]."""
    if p < 0 or p > 1:
        return
    r = [0, 1, 2, 2, 1, 0][min(5, int(p * 6))]
    cv.px(x, y, C(255, 255, 255))
    for k in range(1, r + 1):
        a = 1.0 if k < r else 0.6
        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            cv.px(x + dx * k, y + dy * k, col, a)


class Particles:
    def __init__(self):
        self.p = []

    def emit(self, **kw):
        self.p.append(kw)


FIREWORKS = [(7.35, 81, 10, "yellow"), (8.15, 69, 6, "teal"), (8.95, 92, 8, "pink"),
             (9.7, 75, 13, "green"), (10.3, 87, 5, "orange")]
rng_fw = random.Random(11)
FW_PARTS = []
for (t0, fx, fy, col) in FIREWORKS:
    n = 24
    parts = []
    for i in range(n):
        ang = 2 * math.pi * i / n + rng_fw.uniform(-0.12, 0.12)
        sp = rng_fw.uniform(27, 33)
        parts.append((ang, sp, rng_fw.random()))
    FW_PARTS.append((t0, fx, fy, col, parts))


def draw_fireworks(cv, t):
    k = 2.6
    for (t0, fx, fy, col, parts) in FW_PARTS:
        tau = t - t0
        if tau < 0 or tau > 1.6:
            continue
        r = RAMPS[col]
        if tau < 0.12:
            sparkle(cv, fx, fy, 0.4 + tau * 2, r[4])
        for ang, sp, rnd in parts:
            for lag, la in ((0.0, 1.0), (0.07, 0.55), (0.14, 0.28)):
                tt = tau - lag
                if tt <= 0:
                    continue
                dist = sp * (1 - math.exp(-k * tt)) / k
                x = fx + math.cos(ang) * dist
                y = fy + math.sin(ang) * dist + 7 * tt * tt
                if tt < 0.18:
                    c = r[4]
                elif tt < 0.55:
                    c = r[3]
                elif tt < 0.95:
                    c = r[2]
                else:
                    c = r[1]
                a = la * (1.0 if tt < 0.9 else max(0.0, 1 - (tt - 0.9) / 0.6))
                if tt > 0.9 and ((int(t * FPS) + int(rnd * 10)) % 3 == 0):
                    a *= 0.2  # crackle
                cv.px(x, y, c, a)


# ----------------------------------------------------------------------------- timeline
T_CAKE0, T_CAKE_LAND = 1.15, 1.85
T_CLUSTER = 2.05
T_SPARK = 2.55
SPARK_SEG0 = 0.5
SPARK_HOP = 0.2
T_LIT = [T_SPARK + SPARK_SEG0 + i * SPARK_HOP for i in range(5)]
T_HAPPY, T_BIRTH = 4.25, 4.8
T_COLIN = 5.9
COLIN_STEP = 0.15
T_SLAM = T_COLIN + 5 * COLIN_STEP + 0.12
T_BLOW = 10.75
BLOW_STAGGER = 0.07
T_RELEASE = 11.45
# Where the player's brightness fade-out runs (its last 0.7 s). Nothing here
# fades any more; these only stop new sparkles and confetti starting under it.
T_FADE0, T_FADE1 = 12.3, 13.0

CLUSTER = [  # (size, color, rest centre x, rest top y, delay, phase)
    ("L", "blue", 104, 3, 0.00, 0.3),
    ("XL", "pink", 114, -1, 0.12, 1.9),
    ("L", "yellow", 123, 5, 0.24, 4.1),
]


def cake_y(t):
    if t < T_CAKE0:
        return None
    if t < T_CAKE_LAND:
        s = (t - T_CAKE0) / (T_CAKE_LAND - T_CAKE0)
        return lerp(-CAKE_H - 2, CAKE_REST_Y, s * s)
    tb = t - T_CAKE_LAND
    if tb < 0.24:
        return CAKE_REST_Y - 3 * math.sin(math.pi * tb / 0.24)
    if tb < 0.36:
        return CAKE_REST_Y - 1 * math.sin(math.pi * (tb - 0.24) / 0.12)
    return CAKE_REST_Y


def wick_pos(i, cy):
    return CAKE_X + CANDLE_X[i] + 1, cy  # wick is row 0 of the cake sprite


def flame_state(i, t):
    """Returns (sprite or None, intensity)."""
    tl = T_LIT[i]
    tb = T_BLOW + i * BLOW_STAGGER
    if t < tl:
        return None, 0.0
    if t >= tb:
        k = int((t - tb) / 0.06)
        if k < len(BLOW_SPR):
            return BLOW_SPR[k], 0.8 - 0.2 * k
        return None, 0.0
    age = t - tl
    if age < 0.18:
        k = min(2, int(age / 0.06))
        return GROW_SPR[k], 0.4 + 0.2 * k
    f = int(t * FPS)
    h = (f // 2 * 7 + i * 13) % 17
    idx = [0, 0, 1, 0, 2, 0, 3, 4, 0, 1, 0, 0, 2, 4, 0, 3, 0][h]
    return FLAME_SPR[idx], 1.0


def spark_pos(t):
    """Magic spark that hops along the wicks lighting them. Returns (x,y) or None."""
    if t < T_SPARK or t >= T_LIT[-1]:
        return None
    cy = CAKE_REST_Y
    if t < T_LIT[0]:
        s = (t - T_SPARK) / SPARK_SEG0
        x0, y0 = 62, 22
        x1, y1 = wick_pos(0, cy)
        y1 -= 3
        return lerp(x0, x1, smooth(s)), lerp(y0, y1, s) - math.sin(math.pi * s) * 8
    j = int((t - T_LIT[0]) / SPARK_HOP)
    s = (t - T_LIT[0] - j * SPARK_HOP) / SPARK_HOP
    x0, y0 = wick_pos(j, cy)
    x1, y1 = wick_pos(j + 1, cy)
    return lerp(x0, x1, s), lerp(y0, y1, s) - 3 - math.sin(math.pi * s) * 5


# ----------------------------------------------------------------------------- main render
def render(out_dir):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    bg = make_background()
    rng = random.Random(42)

    confetti = []
    resting = []  # (x, color)
    embers = []  # candle-light sparks
    dust = []
    smoke = []
    spark_trail = []
    twinkles = []  # ambient sparkles around COLIN!
    released = None

    def cannon(x, dir_):
        for _ in range(70):
            confetti.append(dict(
                x=x + rng.uniform(-1, 1), y=60 + rng.uniform(-2, 1),
                vx=dir_ * rng.uniform(22, 78), vy=-rng.uniform(62, 118),
                c=rng.choice(CONFETTI_COLS), ph=rng.random() * 6.28,
                spin=rng.uniform(8, 16), sway=rng.random() * 6.28, vert=rng.random() < 0.5))

    for f in range(NF):
        t = f / FPS
        cv = Canvas(bg)
        draw_stars(cv, t)
        draw_fireworks(cv, t)
        draw_loose(cv, t, "far")
        # redraw table over far balloons (they rise from behind it)
        cv.a[61:64] = bg[61:64]

        # resting confetti on the tablecloth
        for (rx, rc) in resting:
            cv.px(rx, 61, rc)

        # ---- balloon cluster, tied to right gift
        cluster_now = []
        for (size, col, rx, ry, delay, ph) in CLUSTER:
            spr, kx, ky = balloon(size, col)
            bw = spr.shape[1]
            rest_top = (rx - bw // 2, ry)
            ts = t - T_CLUSTER - delay
            if ts < 0:
                continue
            e = ease_out_back(ts / 0.95, 1.8)
            sx, sy = ANCHOR[0] - bw // 2, ANCHOR[1] + 2
            x = lerp(sx, rest_top[0], e)
            y = lerp(sy, rest_top[1], e)
            bob = min(1.0, ts / 1.2)
            x += math.sin(t * 1.3 + ph) * 1.2 * bob
            y += math.sin(t * 1.7 + ph * 1.3) * 1.0 * bob
            if released is not None and t >= T_RELEASE:
                tr = t - T_RELEASE - delay * 0.5
                if tr > 0:
                    y -= 14 * tr + 26 * tr * tr
                    x += math.sin(tr * 2 + ph) * 3 + tr * 4
            cluster_now.append((spr, kx, ky, x, y, rest_top, ph))
        for (spr, kx, ky, x, y, rest_top, ph) in cluster_now:
            ix, iy = int(round(x)), int(round(y))
            if t >= T_RELEASE:
                draw_hanging_string(cv, ix + kx, iy + ky, 20, t, ph, 0.6, drift=-2)
            else:
                rest_len = math.hypot(ANCHOR[0] - (rest_top[0] + kx), ANCHOR[1] - (rest_top[1] + ky))
                draw_tied_string(cv, ix + kx, iy + ky, ANCHOR[0], ANCHOR[1], rest_len, t, ph)
        for (spr, kx, ky, x, y, rest_top, ph) in cluster_now:
            cv.blit(spr, x, y)
        if t >= T_RELEASE and released is None:
            released = t

        # ---- gifts
        cv.blit(GIFT_A, *GIFT_A_POS)
        cv.blit(GIFT_B, *GIFT_B_POS)
        cv.blit(GIFT_C, *GIFT_C_POS)

        # ---- cake
        cy = cake_y(t)
        if cy is not None:
            cyi = int(round(cy))
            cv.blit(CAKE, CAKE_X, cyi)
            if abs(t - T_CAKE_LAND) < DT / 2 + 1e-9:
                for k in range(14):
                    side = -1 if k % 2 == 0 else 1
                    dust.append(dict(x=CAKE_X + (0 if side < 0 else CAKE_W - 1), y=60,
                                     vx=side * rng.uniform(10, 30), vy=-rng.uniform(2, 12), age=0.0,
                                     life=rng.uniform(0.25, 0.45)))
            # wicks
            for i in range(5):
                wx, wy = wick_pos(i, cyi)
                cv.px(wx, wy, C(70, 52, 52))

        # ---- candle glow (dithered, additive)
        lit = []
        if cy is not None:
            for i in range(5):
                spr, inten = flame_state(i, t)
                if spr is not None:
                    wx, wy = wick_pos(i, int(round(cy)))
                    lit.append((wx, wy - 3, inten))
        if lit:
            ys, xs = np.mgrid[0:H, 0:W]
            g = np.zeros((H, W))
            for (lx, ly, inten) in lit:
                g += 0.085 * inten * np.exp(-((xs - lx) ** 2 + (ys - ly) ** 2) / (2 * 10.0 ** 2))
            gq = np.floor(g * 10 + BAYER4[ys % 4, xs % 4]) / 10
            cv.a += gq[..., None] * C(255, 150, 60)[None, None, :]

        # ---- flames
        if cy is not None:
            cyi = int(round(cy))
            for i in range(5):
                spr, inten = flame_state(i, t)
                if spr is not None:
                    wx, wy = wick_pos(i, cyi)
                    cv.blit(spr, wx - 1, wy - 5)
                if abs(t - T_LIT[i]) < DT / 2 + 1e-9:
                    wx, wy = wick_pos(i, cyi)
                    for _ in range(7):
                        a = rng.uniform(-math.pi * 0.95, -math.pi * 0.05)
                        s = rng.uniform(12, 30)
                        embers.append(dict(x=wx, y=wy - 2, vx=math.cos(a) * s, vy=math.sin(a) * s,
                                           age=0.0, life=rng.uniform(0.25, 0.45)))
                tb = T_BLOW + i * BLOW_STAGGER + 0.12
                if tb <= t < tb + 0.5 and f % 2 == 0:
                    wx, wy = wick_pos(i, cyi)
                    smoke.append(dict(x=wx, y=wy - 1, age=0.0, ph=rng.random() * 6.28, life=1.5))

        # ---- smoke
        for s in smoke:
            s["age"] += DT
            a = s["age"]
            x = s["x"] + math.sin(a * 4 + s["ph"]) * 1.2 * min(1, a * 2) + a * 3
            y = s["y"] - a * 12
            al = max(0.0, 1 - a / s["life"]) * 0.55
            cv.px(x, y, C(176, 176, 200), al)
        smoke = [s for s in smoke if s["age"] < s["life"]]

        # ---- embers + dust
        for e in embers:
            e["age"] += DT
            e["vy"] += 60 * DT
            e["x"] += e["vx"] * DT
            e["y"] += e["vy"] * DT
            p = e["age"] / e["life"]
            cv.px(e["x"], e["y"], C(255, 240, 150) if p < 0.4 else C(255, 150, 50), 1 - p * 0.6)
        embers = [e for e in embers if e["age"] < e["life"]]
        for d in dust:
            d["age"] += DT
            d["vx"] *= math.exp(-6 * DT)
            d["x"] += d["vx"] * DT
            d["y"] += d["vy"] * DT
            d["vy"] *= math.exp(-6 * DT)
            p = d["age"] / d["life"]
            cv.px(d["x"], d["y"], C(230, 225, 245), 0.8 * (1 - p))
        dust = [d for d in dust if d["age"] < d["life"]]

        # ---- magic lighting spark
        sp = spark_pos(t)
        if sp is not None:
            spark_trail.append(sp)
            spark_trail = spark_trail[-6:]
            for k, (tx, ty) in enumerate(spark_trail[:-1]):
                cv.px(tx, ty, C(255, 220, 120), 0.15 + 0.12 * k)
            sparkle(cv, int(round(sp[0])), int(round(sp[1])), 0.35 + 0.3 * ((f % 3) / 2), C(255, 236, 140))
        else:
            spark_trail = []

        # ---- text
        wave_amp = smooth((t - 7.0) / 0.6) * (1 - smooth((t - 11.8) / 0.4))
        for row, (glyphs, xs, y0, t0, step) in enumerate(((HAPPY, HAPPY_X, HAPPY_Y, T_HAPPY, 0.09),
                                                           (BIRTH, BIRTH_X, BIRTH_Y, T_BIRTH, 0.075))):
            for i, ((ch, g), x) in enumerate(zip(glyphs, xs)):
                tt = t - (t0 + i * step)
                if tt < 0:
                    continue
                yoff = (1 - ease_out_bounce(tt / 0.55)) * -(y0 + 12)
                gi = i + (0 if row == 0 else 5)
                yoff += round(wave_amp * 1.45 * math.sin(t * 5.2 - gi * 0.55))
                blit_glyph(cv, g, x, int(round(y0 + yoff)))

        shine = None
        for k in range(4):
            ts = 7.2 + k * 1.3
            if ts <= t < ts + 0.7:
                shine = lerp(TEXT_CX - 45, TEXT_CX + 50, (t - ts) / 0.7)
        for i, ((ch, g), x) in enumerate(zip(COLIN, COLIN_X)):
            tt = t - (T_COLIN + i * COLIN_STEP)
            if tt < 0:
                continue
            # pop: rise from 3px low with overshoot, white flash on first frames
            yoff = (1 - ease_out_back(tt / 0.25, 3.0)) * 4
            flash = max(0.0, 1 - tt / 0.16)
            blit_glyph(cv, g, x, int(round(COLIN_Y + yoff)), flash=flash, shine=shine)
            if tt < 0.35:
                gw = g[1].shape[1]
                sparkle(cv, x - 2, COLIN_Y - 2, tt / 0.35)
                sparkle(cv, x + gw - 1, COLIN_Y + 12, tt / 0.35 + 0.1)

        # ---- ambient twinkles around the name once it's all in
        if T_SLAM < t < T_FADE0 and f % 4 == 0:
            for _ in range(2):
                twinkles.append(dict(x=rng.randrange(3, 66), y=rng.randrange(1, 44), age=0.0,
                                     life=rng.uniform(0.35, 0.6),
                                     c=rng.choice([C(255, 255, 220), C(200, 230, 255), C(255, 210, 240)])))
        for s in twinkles:
            s["age"] += DT
            sparkle(cv, s["x"], s["y"], s["age"] / s["life"], s["c"])
        twinkles = [s for s in twinkles if s["age"] < s["life"]]

        # ---- confetti
        if abs(t - T_SLAM) < DT / 2 + 1e-9:
            cannon(-1, 1)
            cannon(W, -1)
        if T_SLAM + 0.8 < t < T_FADE0 - 0.4 and rng.random() < 0.3:
            confetti.append(dict(x=rng.uniform(0, W), y=-1, vx=rng.uniform(-4, 4), vy=rng.uniform(6, 12),
                                 c=rng.choice(CONFETTI_COLS), ph=rng.random() * 6.28,
                                 spin=rng.uniform(8, 16), sway=rng.random() * 6.28, vert=rng.random() < 0.5))
        keep = []
        for p in confetti:
            p["vx"] *= math.exp(-2.2 * DT)
            p["vy"] += 90 * DT
            if p["vy"] > 13:
                p["vy"] -= (p["vy"] - 13) * (1 - math.exp(-4 * DT))
            p["sway"] += 3 * DT
            p["x"] += (p["vx"] + math.sin(p["sway"]) * 7) * DT
            p["y"] += p["vy"] * DT
            p["ph"] += p["spin"] * DT
            if p["y"] >= 61 and p["vy"] > 0:
                if 0 <= p["x"] < W:
                    resting.append((int(p["x"]), p["c"] * 0.85))
                continue
            if -3 <= p["x"] <= W + 3:
                keep.append(p)
            s = math.sin(p["ph"])
            x, y = int(round(p["x"])), int(round(p["y"]))
            if abs(s) > 0.45:
                cv.px(x, y, p["c"])
                if p["vert"]:
                    cv.px(x, y + 1, p["c"] * 0.8)
                else:
                    cv.px(x + 1, y, p["c"] * 0.8)
            else:
                cv.px(x, y, p["c"] * 0.6)
        confetti = keep

        # ---- near loose balloons (in front)
        draw_loose(cv, t, "near")

        # ---- camera shake + fades
        img = np.clip(cv.a, 0, 255)
        ts = t - T_SLAM
        if 0 <= ts < 5 * DT:
            dx, dy = [(0, 1), (0, -1), (1, 0), (-1, 1), (0, 0)][int(ts / DT + 1e-6)]
            img = np.roll(np.roll(img, dy, 0), dx, 1)
        Image.fromarray(img.round().astype(np.uint8), "RGB").save(out_dir / f"f{f:04d}.png")
    print(f"rendered {NF} frames to {out_dir}")


if __name__ == "__main__":
    render(sys.argv[1] if len(sys.argv) > 1 else "frames")

# Desktop preview pipeline

Runs the **real** firmware render code (`scoreboard.display.render_frame` and
friends) on CPython so screen designs can be iterated without flashing a Pico.
It installs MicroPython shims, drives named scenarios through the actual
`scoreboard.state` mailbox, intercepts the RGB565 framebuffer, and renders it to
PNGs/GIFs plus a click-to-zoom HTML gallery.

## Usage

```
python -m tools.preview                       # all scenarios, placeholder logos
python -m tools.preview --list                # list scenarios + variants
python -m tools.preview --scenario live-basic --open
python -m tools.preview --flat                # flat panel image (ground truth)
python -m tools.preview --backend-url https://your-backend --api-key KEY --refresh-logos
```

Output lands in `tools/preview/out/` (gitignored): `index.html`, one
LED-look `<scenario>__<variant>.png` (or `.gif` for animated scenarios) per
cell, and a flat native-resolution `<scenario>__<variant>_flat.png` alongside
every cell as the pixel ground truth.

### Options

| flag | meaning |
| --- | --- |
| `--scenario N` | render only this scenario (repeatable) |
| `--variant N` | render only this variant (repeatable) |
| `--backend-url URL` | fetch real 24×24 logos instead of placeholders |
| `--api-key K` | API key (else `$SCOREBOARD_API_KEY`) |
| `--refresh-logos` | ignore the logo cache and re-fetch |
| `--scale N` | LED-look upscale factor (default 8) |
| `--flat` | gallery shows the flat panel image, not the LED look |
| `--out DIR` | output directory (default `tools/preview/out`) |
| `--open` | open the gallery in a browser when done |
| `--list` | list scenarios/variants and exit |

## How it works

`firmware_env.load_firmware(clock)` installs the shims (in `shims/`) **before**
importing any firmware module, so the real hardware modules never load:

- `framebuf` — a pure-Python `FrameBuffer` that is bit-for-bit compatible with
  the generated sprite/font data. Its packed layouts are pinned against
  `tools/compile_layout.py`'s `pack_*` packers by `tests/test_framebuf_shim.py`.
- `time` — backed by a `VirtualClock` (starts at 100 000 ms so the
  `updated_ms == 0` "never" sentinel still reads correctly); unknown attributes
  delegate to the real `time` module.
- `micropython` (`const`/`native`/`viper` identity), `machine` (no-op `Pin`),
  `uasyncio`→`asyncio`, `aiohttp` (empty).
- a fake `hub75` package: inert `Hub75Driver`, a `PreviewDisplay` framebuffer,
  the **real** `gamma.py` loaded by path, and a pure-Python
  `pack_hsv_to_rgb565`.

Only `firmware/src` goes on `sys.path` (not `firmware/src/lib`), so the fake
`hub75` shadows the real driver.

The generated `layout/` and `fonts/` modules must already exist on disk. They
are build artifacts (gitignored); if missing, the preview exits with:

```
run `python tools/compile_layout.py && python tools/compile_fonts.py`
```

## Caveats / firmware quirks

- **`miqro` is not stubbed.** The firmware imports it lazily inside a
  try/except for WiFi-setup QR generation, so the `setup-fresh` scenario
  degrades gracefully to a QR-less screen (you'll see
  `[MAIN] qr generation failed: No module named 'miqro'` — that is expected).
- **`pack_hsv_to_rgb565` for saturation > 0** (the critical-count red tint) is a
  pure-Python approximation of the native `.mpy`. Saturation 0 (grayscale) is
  exact by construction; verify the tinted look on real hardware once.
- **No gamma is applied** when converting RGB565→RGB888 for display: the panel's
  own LUT is mirrored by the monitor's sRGB decode, so the flat PNG is the
  ground truth.

## Adding a scenario

Add a function to `scenarios.py` decorated with `@scenario(name, duration_ms=…)`
that builds domain objects via their plain constructors and publishes them
through the real setters (`get_write_state`/`commit_state`, `set_toast`, …).
`duration_ms > 0` makes it an animated GIF (`duration_ms // 50` frames at the
50 ms display tick).

## Tests

```
python -m pytest tools/preview/tests -q
python tools/preview/tests/test_framebuf_shim.py   # runs without pytest too
python tools/preview/tests/test_golden.py
```

`test_framebuf_shim.py` is the shim oracle suite; `test_golden.py` hashes the
`live-critical-count` frame at a fixed virtual instant (skips with a build hint
if the generated artifacts are missing; prints the new hash on an intended
change).

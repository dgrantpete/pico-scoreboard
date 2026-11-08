# The true-colour art oracle (BACKLOG 116)

Every sprite `tools/compile_layout.py` compiles into
`src/generated/layout.rs`, at full 8-bit RGBA, as the test fixtures the art's
replacements are pinned to. `tests/art_oracle.rs` proves each one is its
compiled sprite pixel for pixel, exactly — so this directory is
what the panel shows today, at the precision the source art had.

## Where they came from (2026-11-08, from main at 2a34275)

- `mlb_layout.json`, `hockey_layout.json`, `football_layout.json`: the
  Aseprite CLI's sidecars, verbatim, from `compile_layout.py`'s own flags:

      aseprite --batch --split-layers --list-layers --list-slices --trim
          --ignore-empty --merge-duplicates --sheet-type packed
          --filename-format "{layer}" --sheet <stem>.png --data <stem>.json
          --format json-hash firmware/assets/layout/<stem>.aseprite

  They carry every slice and each layer's screen position
  (`spriteSourceSize`); their `image` sheets are not committed.
- `<layer>.png`: each `__relative`/`__absolute` layer cropped out of that
  sheet exactly as `compile_layout.py` crops it (Pillow,
  `sheet.convert("RGBA").crop(frame)`), named without its suffix.
- `toast_*.png`: byte copies of `firmware/assets/layout/toast_*.png`, the
  plain-PNG sources `compile_layout.py` reads (`tools/gen_toast_icons.py`
  still draws exactly these pixels).

Alpha is 0 or 255 everywhere; 0 is transparent.

## Lifetime

Phase 2 of BACKLOG 116 deletes this directory with the sprites it pins,
once the procedural replacements are wired in and the golden frames are
proven byte-identical. The four hand-drawn sprites' exports — the MLB
`field` and `base_marker`, the `football_ball` and `football_field` — are
not here: cropped the same way, they are the new source of truth, in
`crates/scoreboard-render/art/`, and outlive the pipeline.

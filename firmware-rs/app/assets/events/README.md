# `events/` — event clips, committed as build artifacts

Each `.sbev` here is a full-screen animated clip that an event plays while its
date window is open (`src/event.rs`, `EVENTS`). The format and the player are
`crates/scoreboard-render/src/event.rs`; the clip is `include_bytes!`d into the
image and decoded in place from flash.

Each clip sits beside a `.sbev.frames` file: one FNV-1a-64 hash per frame of
the RGB565 frame the clip must decode to, written by the encoder.
`crates/scoreboard-render/tests/event.rs` decodes every shipped clip through
the firmware's decoder and checks every frame against it.

This is the same **committed-generated** pattern as `../index.html.gz`, for
the same reason: regenerating needs Python, numpy and Pillow, but *building*
needs only cargo.

## Regenerating a clip

```sh
uv run --with numpy --with pillow tools/events/<art_script>.py <frames_dir>
uv run --with numpy --with pillow tools/events/encode_clip.py <frames_dir> \
    firmware-rs/app/assets/events/<event-name>.sbev --fps 30
```

The art script is the editable source, and it stays in `tools/events/` after
its clip is retired (`colin_birthday_2026.py` is the first). Both steps are deterministic (seeded
RNGs, no timestamps), so rerunning them on unchanged sources reproduces the
same bytes. **Commit a clip in the same commit as the source change that
produced it.**

## Clips are temporary

A clip costs flash in every image that carries it: the first one took the
image from 71 % to 92 % of the active partition. When an event's window has
closed, delete its entry from `EVENTS`, its clip and `.frames` here, and its
row in the test's `SHIPPED` table, in the next release.

## Provenance

No clip ships today. Each one that does gets a row here:

| clip | window | frames | size | SHA-1 | built from |
|---|---|---|---|---|---|

//! The true-colour oracle is the truth (BACKLOG 116): every sprite
//! `tools/compile_layout.py` compiled into `generated/layout.rs`, decoded
//! through the same `Source`/`blit` path the renderers use, equals its 8-bit
//! export quantized by `rgb565()` — pixel for pixel, with every transparent
//! pixel left untouched. If this holds, a replacement pinned to the export is
//! pinned to what the panel shows today, and stays exact at eight bits once
//! the canvas is (BACKLOG 115).
//!
//! Eight exports live under `tests/art/` and go with the sprites. The four
//! hand-drawn ones live under `art/`: they are the source of truth from here
//! on, and this test is what made them so.

mod oracle;

use oracle::{Art, assert_quantized, buffer};
use scoreboard_render::blit::{Canvas, Slice, Sprite};
use scoreboard_render::generated::layout;

/// The sprites the procedural replacements take over, pinned through
/// `tests/art/`.
const ORACLE: [(&str, Sprite); 8] = [
    ("dot", layout::dot::SPRITE),
    ("inning_bottom", layout::inning_bottom::SPRITE),
    ("inning_top", layout::inning_top::SPRITE),
    ("rink", layout::rink::SPRITE),
    ("star", layout::star::SPRITE),
    ("toast_lock_closed", layout::toast_lock_closed::SPRITE),
    ("toast_lock_open", layout::toast_lock_open::SPRITE),
    ("toast_spinner", layout::toast_spinner::SPRITE),
];

/// The hand-drawn sprites, whose exports are now their sources under `art/`.
const SOURCES: [(&str, Sprite); 4] = [
    ("base_marker", layout::base_marker::SPRITE),
    ("field", layout::field::SPRITE),
    ("football_ball", layout::football_ball::SPRITE),
    ("football_field", layout::football_field::SPRITE),
];

fn assert_is_sprite(art: &Art, sprite: Sprite) {
    let name = art.name.as_str();
    assert_eq!(
        (art.width, art.height),
        (sprite.width, sprite.height),
        "{name}: size"
    );
    let mut pixels = buffer(art.width, art.height);
    let mut canvas = Canvas::new(&mut pixels, art.width, art.height);
    canvas.blit(&sprite.source(), 0, 0);
    assert_quantized(&canvas, 0, 0, art, name);
}

#[test]
fn every_export_is_its_compiled_sprite() {
    for (name, sprite) in ORACLE {
        assert_is_sprite(&Art::oracle(name), sprite);
    }
}

#[test]
fn every_hand_drawn_source_is_its_compiled_sprite() {
    for (name, sprite) in SOURCES {
        assert_is_sprite(&Art::source(name), sprite);
    }
}

/// The screen positions the art carried — each `__absolute` layer's
/// `spriteSourceSize` and each base slice in the JSON sidecars — are the
/// compiled `POSITION`s and `SLICE`s, so these are the numbers the
/// replacements and the hand-drawn sources' compiled form take over.
#[test]
fn the_art_sits_where_the_exports_put_it() {
    for (art, position, (x, y)) in [
        (Art::source("field"), layout::field::POSITION, (86, 3)),
        (
            Art::source("football_field"),
            layout::football_field::POSITION,
            (3, 52),
        ),
        (
            Art::oracle("inning_bottom"),
            layout::inning_bottom::POSITION,
            (5, 33),
        ),
        (
            Art::oracle("inning_top"),
            layout::inning_top::POSITION,
            (5, 31),
        ),
        (Art::oracle("rink"), layout::rink::POSITION, (2, 47)),
        // The base marker is relative: these three slices place it.
        (
            Art::source("base_marker"),
            layout::first_base::SLICE,
            (111, 16),
        ),
        (
            Art::source("base_marker"),
            layout::second_base::SLICE,
            (103, 8),
        ),
        (
            Art::source("base_marker"),
            layout::third_base::SLICE,
            (95, 16),
        ),
    ] {
        assert_eq!(
            position,
            Slice {
                x,
                y,
                width: art.width,
                height: art.height
            },
            "{}",
            art.name
        );
    }
}

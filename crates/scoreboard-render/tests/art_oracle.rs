//! The true-colour oracle is the truth (BACKLOG 116): every sprite
//! `tools/compile_layout.py` compiled into `generated/layout.rs`, decoded
//! through the same `Source`/`blit` path the renderers use, equals its 8-bit
//! export under `tests/art/` quantized by `rgb565()` — pixel for pixel, with
//! every transparent pixel left untouched. If this holds, a replacement pinned
//! to the export is pinned to what the panel shows today, and stays exact at
//! eight bits once the canvas is (BACKLOG 115).

mod oracle;

use oracle::{Art, assert_quantized, buffer};
use scoreboard_render::blit::{Canvas, Slice, Sprite};
use scoreboard_render::generated::layout;

/// Every compiled sprite, by the name of its layer or PNG.
const SPRITES: [(&str, Sprite); 12] = [
    ("base_marker", layout::base_marker::SPRITE),
    ("dot", layout::dot::SPRITE),
    ("field", layout::field::SPRITE),
    ("football_ball", layout::football_ball::SPRITE),
    ("football_field", layout::football_field::SPRITE),
    ("inning_bottom", layout::inning_bottom::SPRITE),
    ("inning_top", layout::inning_top::SPRITE),
    ("rink", layout::rink::SPRITE),
    ("star", layout::star::SPRITE),
    ("toast_lock_closed", layout::toast_lock_closed::SPRITE),
    ("toast_lock_open", layout::toast_lock_open::SPRITE),
    ("toast_spinner", layout::toast_spinner::SPRITE),
];

#[test]
fn every_export_is_its_compiled_sprite() {
    for (name, sprite) in SPRITES {
        let art = Art::oracle(name);
        assert_eq!(
            (art.width, art.height),
            (sprite.width, sprite.height),
            "{name}: size"
        );
        let mut pixels = buffer(art.width, art.height);
        let mut canvas = Canvas::new(&mut pixels, art.width, art.height);
        canvas.blit(&sprite.source(), 0, 0);
        assert_quantized(&canvas, 0, 0, &art, name);
    }
}

/// The `__absolute` layers' screen positions — each export's
/// `spriteSourceSize` in the JSON sidecar beside it — are the compiled
/// `POSITION`s, so the numbers the replacements take over are these.
#[test]
fn absolute_layers_sit_where_the_exports_put_them() {
    for (name, position, (x, y)) in [
        ("field", layout::field::POSITION, (86, 3)),
        ("football_field", layout::football_field::POSITION, (3, 52)),
        ("inning_bottom", layout::inning_bottom::POSITION, (5, 33)),
        ("inning_top", layout::inning_top::POSITION, (5, 31)),
        ("rink", layout::rink::POSITION, (2, 47)),
    ] {
        let art = Art::oracle(name);
        assert_eq!(
            position,
            Slice {
                x,
                y,
                width: art.width,
                height: art.height
            },
            "{name}"
        );
    }
}

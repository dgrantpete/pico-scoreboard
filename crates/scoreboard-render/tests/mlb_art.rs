//! The procedural count dot and inning arrows (BACKLOG 116), pinned to the
//! compiled sprites they replace: pixel for pixel against the 8-bit exports
//! in `tests/art/` (quantized today, exact constants for after BACKLOG 115),
//! and — for the dot — against `widgets::count_dots` itself through every
//! tint the renderers hand it.

mod oracle;

use oracle::{Art, assert_drawn, assert_same_frame, assert_untouched_outside, buffer, widen};
use scoreboard_model::Rgb888;
use scoreboard_render::blit::{Canvas, Slice};
use scoreboard_render::count_dot::{self, OUTLINE, UNFILLED};
use scoreboard_render::generated::layout;
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::inning_arrow::{self, COLOR, DOWN_Y, UP_Y, X};
use scoreboard_render::widgets::count_dots;
use scoreboard_render::{pack, rgb565};

#[test]
fn the_count_dot_is_its_export() {
    let art = Art::oracle("dot");
    let (x, y) = (71, 7);
    let mut pixels = buffer(WIDTH, HEIGHT);
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    count_dot::draw(&mut canvas, x, y, OUTLINE, UNFILLED);
    assert_drawn(&canvas, x, y, &art, &[OUTLINE, UNFILLED], "count dot");
    let size = count_dot::SIZE;
    assert_untouched_outside(&canvas, (x, y, size, size), "count dot");
    assert_eq!((art.width, art.height), (size, size));
}

/// A row of dots drawn one by one is `count_dots`' row through its tinted
/// palette: untinted, a team's colour, and the MLB pulse's warm reds — each
/// with none, some and all of the dots counting.
#[test]
fn a_row_of_dots_is_the_widgets_row_through_every_tint() {
    let rect = Slice {
        x: 61,
        y: 7,
        width: 19,
        height: 4,
    };
    let pitch = count_dot::SIZE + 1;
    let tints = [
        None,
        Some(pack(Rgb888(0x4e008e))),
        Some(rgb565(255, 40, 40)),
        Some(rgb565(120, 0, 0)),
    ];
    for tint in tints {
        for filled in 0..=4u8 {
            let (mut drawn, mut widget) = (buffer(WIDTH, HEIGHT), buffer(WIDTH, HEIGHT));
            let mut drawn = Canvas::new(&mut drawn, WIDTH, HEIGHT);
            let mut widget = Canvas::new(&mut widget, WIDTH, HEIGHT);
            count_dots(&mut widget, rect, filled, tint);

            let outline = tint.map_or(OUTLINE, widen);
            for index in 0..(rect.width + 1) / pitch {
                let interior = if index < filled as i32 {
                    outline
                } else {
                    UNFILLED
                };
                count_dot::draw(
                    &mut drawn,
                    rect.x + index * pitch,
                    rect.y,
                    outline,
                    interior,
                );
            }
            assert_same_frame(&drawn, &widget, "count dots");
        }
    }
}

#[test]
fn the_arrows_sit_where_the_compiled_ones_did() {
    for (y, position) in [
        (UP_Y, layout::inning_top::POSITION),
        (DOWN_Y, layout::inning_bottom::POSITION),
    ] {
        assert_eq!(
            Slice {
                x: X,
                y,
                width: inning_arrow::WIDTH,
                height: inning_arrow::HEIGHT
            },
            position
        );
    }
}

#[test]
fn the_inning_arrows_are_their_exports() {
    for (name, y, up) in [("inning_top", UP_Y, true), ("inning_bottom", DOWN_Y, false)] {
        let art = Art::oracle(name);
        let mut pixels = buffer(WIDTH, HEIGHT);
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
        inning_arrow::draw(&mut canvas, X, y, up);
        assert_drawn(&canvas, X, y, &art, &[COLOR], name);
        let rect = (X, y, inning_arrow::WIDTH, inning_arrow::HEIGHT);
        assert_untouched_outside(&canvas, rect, name);
    }
}

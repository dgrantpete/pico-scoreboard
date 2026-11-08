//! The procedural rink and star (BACKLOG 116), pinned to the compiled
//! sprites they replace: pixel for pixel against the 8-bit exports in
//! `tests/art/` (quantized today, exact constants for after BACKLOG 115),
//! against the compiled sprite through every palette `game/hockey.rs` tints
//! it with, and against the goal projection `game/hockey.rs` derives from the
//! compiled rectangle.

mod oracle;

use oracle::{
    Art, assert_drawn, assert_quantized, assert_same_frame, assert_untouched_outside, buffer, widen,
};
use scoreboard_model::Rgb888;
use scoreboard_model::snapshot::{GoalMark, Side};
use scoreboard_render::blit::Canvas;
use scoreboard_render::generated::layout;
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::rink::{
    self, AWAY_END, BLUE_LINE, BOARDS, CREASE, HOME_END, ICE, RED_LINE, Rink, STRIP,
};
use scoreboard_render::star::{self, GOLD};
use scoreboard_render::{MAGENTA, pack, scale};

/// Every colour the rink draws, untinted.
const RINK_COLORS: [Rgb888; 7] = [ICE, BOARDS, RED_LINE, BLUE_LINE, CREASE, AWAY_END, HOME_END];

/// The STRIP's rectangle, for the outside-it check.
const STRIP_RECT: (i32, i32, i32, i32) = (STRIP.x, STRIP.y, STRIP.width, STRIP.height);

#[test]
fn the_strip_is_the_compiled_rinks_rectangle() {
    let position = layout::rink::POSITION;
    assert_eq!(
        (STRIP.x, STRIP.y, STRIP.width, STRIP.height),
        (position.x, position.y, position.width, position.height)
    );
}

#[test]
fn the_rink_is_its_export() {
    let art = Art::oracle("rink");
    let mut pixels = buffer(WIDTH, HEIGHT);
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    STRIP.draw(&mut canvas, AWAY_END, HOME_END);
    assert_drawn(&canvas, STRIP.x, STRIP.y, &art, &RINK_COLORS, "rink");
    assert_untouched_outside(&canvas, STRIP_RECT, "rink");
}

/// The glow's second pass: the lines and creases alone, the ice and both ends
/// left as they were.
#[test]
fn the_lines_are_the_export_without_ice_or_ends() {
    let art = Art::oracle("rink").without(&[ICE, AWAY_END, HOME_END]);
    let mut pixels = buffer(WIDTH, HEIGHT);
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    STRIP.draw_lines(&mut canvas);
    assert_drawn(
        &canvas,
        STRIP.x,
        STRIP.y,
        &art,
        &[BOARDS, RED_LINE, BLUE_LINE, CREASE],
        "rink lines",
    );
    assert_untouched_outside(&canvas, STRIP_RECT, "rink lines");
}

/// The palette index of `color` in the compiled rink — found by value, as
/// `game/hockey.rs` finds its placeholders.
fn rink_index(color: Rgb888) -> usize {
    let packed = pack(color);
    layout::rink::PALETTE
        .iter()
        .position(|&entry| entry == packed)
        .expect("a rink colour is in the compiled palette")
}

/// Team colours through `game/hockey.rs`'s end tint, a third of the packed
/// colour — near-black, saturated, pastel, white.
fn end_tints() -> impl Iterator<Item = (u16, u16)> {
    [
        (0x4e008e, 0x007ac1),
        (0xc8102e, 0x041e42),
        (0xffffff, 0x000000),
        (0x6f263d, 0xfcb514),
    ]
    .into_iter()
    .map(|(away, home)| {
        (
            scale(pack(Rgb888(away)), 1, 3),
            scale(pack(Rgb888(home)), 1, 3),
        )
    })
}

/// Whatever the ends are tinted, the drawn rink is the compiled one blitted
/// through the palette with those two entries replaced — and so is the glow
/// pass through the palette with the ice and ends keyed out.
#[test]
fn the_rink_takes_every_tint_the_renderer_gives_its_ends() {
    let (away, home, ice) = (rink_index(AWAY_END), rink_index(HOME_END), rink_index(ICE));
    for (away_tint, home_tint) in end_tints() {
        let mut palette = layout::rink::PALETTE;
        palette[away] = away_tint;
        palette[home] = home_tint;
        let (mut drawn, mut blitted) = (buffer(WIDTH, HEIGHT), buffer(WIDTH, HEIGHT));
        let mut drawn = Canvas::new(&mut drawn, WIDTH, HEIGHT);
        let mut blitted = Canvas::new(&mut blitted, WIDTH, HEIGHT);
        STRIP.draw(&mut drawn, widen(away_tint), widen(home_tint));
        blitted.blit(&layout::rink::SPRITE.tinted(&palette), STRIP.x, STRIP.y);
        assert_same_frame(&drawn, &blitted, "tinted rink");

        let mut lines = palette;
        for index in [ice, away, home] {
            lines[index] = MAGENTA;
        }
        let (mut drawn, mut blitted) = (buffer(WIDTH, HEIGHT), buffer(WIDTH, HEIGHT));
        let mut drawn = Canvas::new(&mut drawn, WIDTH, HEIGHT);
        let mut blitted = Canvas::new(&mut blitted, WIDTH, HEIGHT);
        STRIP.draw_lines(&mut drawn);
        blitted.blit(&layout::rink::SPRITE.tinted(&lines), STRIP.x, STRIP.y);
        assert_same_frame(&drawn, &blitted, "rink lines");
    }
}

/// The shootout's short rink is three three-row bands of the full one, each
/// drawn into its own band with the rink shifted up: the drawing clips like
/// the blit it replaces.
#[test]
fn a_band_of_the_rink_is_the_same_rows_of_the_export() {
    let art = Art::oracle("rink");
    for from in [0, 6, 12] {
        let mut pixels = buffer(WIDTH, 3);
        let mut band = Canvas::new(&mut pixels, WIDTH, 3);
        Rink { y: -from, ..STRIP }.draw(&mut band, AWAY_END, HOME_END);
        let rows = art.crop(0, from, art.width, 3);
        assert_quantized(&band, STRIP.x, 0, &rows, "rink band");
        assert_untouched_outside(&band, (STRIP.x, 0, STRIP.width, 3), "rink band");
    }
}

/// `game/hockey.rs`'s `mark_at`, restated from the numbers: a goal's 2 × 2
/// mark has its bottom-right pixel on the projected point.
fn mark_at(rink: Rink, x: i32, y: i32) -> (i32, i32) {
    (
        rink.x_at(rink::LENGTH_FT / 2 + x) - 1,
        rink.mid() + rink.dy(y) - 1,
    )
}

#[test]
fn the_projection_is_the_one_hockey_derives_from_the_sprite() {
    for x in i8::MIN..=i8::MAX {
        for y in i8::MIN..=i8::MAX {
            let mark = GoalMark {
                side: Side::Away,
                x,
                y,
            };
            assert_eq!(
                mark_at(STRIP, x.into(), y.into()),
                scoreboard_render::game::hockey::mark_at(mark),
                "goal at ({x}, {y}) ft"
            );
        }
    }
}

/// The lines sit where the projection puts their feet.
#[test]
fn the_lines_are_where_their_feet_project() {
    let mut pixels = buffer(WIDTH, HEIGHT);
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    STRIP.draw_lines(&mut canvas);
    let red = pack(RED_LINE);
    let blue = pack(BLUE_LINE);
    let mid = STRIP.mid();
    for (feet, color) in [
        (rink::GOAL_LINE_FT, red),
        (rink::BLUE_LINE_FT, blue),
        (rink::CENTER_FT, red),
        (rink::LENGTH_FT - rink::BLUE_LINE_FT, blue),
        (rink::LENGTH_FT - rink::GOAL_LINE_FT, red),
    ] {
        assert_eq!(
            canvas.pixel_at(STRIP.x_at(feet), mid - 3),
            Some(color),
            "{feet} ft"
        );
    }
    assert_eq!(STRIP.dy(0), 0);
    assert_eq!(STRIP.dy(42), -STRIP.dy(-42));
}

#[test]
fn the_star_is_its_export() {
    let art = Art::oracle("star");
    let (x, y) = (52, 15);
    let mut pixels = buffer(WIDTH, HEIGHT);
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    star::draw(&mut canvas, x, y);
    assert_drawn(&canvas, x, y, &art, &[GOLD], "star");
    assert_untouched_outside(&canvas, (x, y, star::SIZE, star::SIZE), "star");
    assert_eq!((art.width, art.height), (star::SIZE, star::SIZE));
}

/// The star clips at the canvas edge like the blit it replaces.
#[test]
fn the_star_clips_like_the_sprite() {
    for (x, y) in [(-2, -3), (125, 61), (-4, 62)] {
        let (mut drawn, mut blitted) = (buffer(WIDTH, HEIGHT), buffer(WIDTH, HEIGHT));
        let mut drawn = Canvas::new(&mut drawn, WIDTH, HEIGHT);
        let mut blitted = Canvas::new(&mut blitted, WIDTH, HEIGHT);
        star::draw(&mut drawn, x, y);
        blitted.blit(&layout::star::SPRITE.source(), x, y);
        assert_same_frame(&drawn, &blitted, "clipped star");
    }
}

//! The procedural rink and star (BACKLOG 116), pinned to exact pictures: the
//! rink whole, tinted, as the glow's lines-only pass and as the shootout's
//! bands; the star placed and clipped; and the goal projection to the numbers
//! the rink's compiled sprite carried before it was code.

mod picture;

use picture::{assert_picture, assert_untouched_outside, buffer, frame, size};
use scoreboard_model::Rgb888;
use scoreboard_model::snapshot::{GoalMark, Side};
use scoreboard_render::blit::Canvas;
use scoreboard_render::game::hockey::mark_at;
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::rink::{self, AWAY_END, HOME_END, Rink, STRIP};
use scoreboard_render::{scale, star};

/// The live strip, 124 × 15 at (2, 47): `b` boards, `i` ice, `r` the red
/// lines, `u` the blue lines, `c` the creases, `a` and `h` the away and home
/// ends.
const RINK: [&str; 15] = [
    "....bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb....",
    "..bbaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhbb..",
    ".baaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhb.",
    "baaaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhhb",
    "baaaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhhb",
    "baaaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhhb",
    "baaaaarciiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiicrhhhhhhb",
    "baaaaarcciiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiccrhhhhhhb",
    "baaaaarciiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiicrhhhhhhb",
    "baaaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhhb",
    "baaaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhhb",
    "baaaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhhb",
    ".baaaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhhhb.",
    "..bbaariiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiuuiiiiiiiiiiiirriiiiiiiiiiiiiuuiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiirhhhbb..",
    "....bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb....",
];

/// The rink's legend with the ends painted `away` and `home`, or held back.
fn rink_legend(
    away: Option<Rgb888>,
    home: Option<Rgb888>,
    ice: Option<Rgb888>,
) -> [(char, Option<Rgb888>); 7] {
    [
        ('b', Some(Rgb888::new(150, 160, 170))),
        ('i', ice),
        ('r', Some(Rgb888::new(200, 24, 24))),
        ('u', Some(Rgb888::new(30, 80, 240))),
        ('c', Some(Rgb888::new(70, 140, 255))),
        ('a', away),
        ('h', home),
    ]
}

const ICE: Option<Rgb888> = Some(Rgb888::new(14, 22, 34));

const STRIP_RECT: (i32, i32, i32, i32) = (STRIP.x, STRIP.y, STRIP.width, STRIP.height);

#[test]
fn the_strip_sits_under_the_shots_bar() {
    assert_eq!(STRIP_RECT, (2, 47, 124, 15));
    assert_eq!(size(&RINK), (STRIP.width, STRIP.height));
}

/// Untinted, the ends are the art's placeholders: pure red, pure blue.
#[test]
fn the_rink_is_its_picture() {
    let mut pixels = frame();
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    STRIP.draw(&mut canvas, AWAY_END, HOME_END);
    let legend = rink_legend(
        Some(Rgb888::new(255, 0, 0)),
        Some(Rgb888::new(0, 0, 255)),
        ICE,
    );
    assert_picture(&canvas, (STRIP.x, STRIP.y), &RINK, &legend, "rink");
    assert_untouched_outside(&canvas, STRIP_RECT, "rink");
}

/// Each end takes exactly the colour it is given: a third of the team's, as
/// `game/hockey.rs` asks, for colours from near-black to white.
#[test]
fn the_ends_take_any_tint() {
    for (away, home) in [
        (0x4e008e, 0x007ac1),
        (0xc8102e, 0x041e42),
        (0xffffff, 0x000000),
        (0x6f263d, 0xfcb514),
    ] {
        let (away, home) = (scale(Rgb888(away), 1, 3), scale(Rgb888(home), 1, 3));
        let mut pixels = frame();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
        STRIP.draw(&mut canvas, away, home);
        let legend = rink_legend(Some(away), Some(home), ICE);
        assert_picture(&canvas, (STRIP.x, STRIP.y), &RINK, &legend, "tinted rink");
    }
}

/// The glow's second pass: the boards, lines and creases, and not one pixel
/// of ice or either end.
#[test]
fn the_lines_are_the_picture_without_ice_or_ends() {
    let mut pixels = frame();
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    STRIP.draw_lines(&mut canvas);
    let legend = rink_legend(None, None, None);
    assert_picture(&canvas, (STRIP.x, STRIP.y), &RINK, &legend, "rink lines");
    assert_untouched_outside(&canvas, STRIP_RECT, "rink lines");
}

/// The shootout's short rink is three three-row bands of the full one, each
/// drawn into its own band with the rink shifted up: the region clips it to
/// exactly those rows.
#[test]
fn a_band_of_the_rink_is_the_same_rows_of_the_picture() {
    let legend = rink_legend(
        Some(Rgb888::new(255, 0, 0)),
        Some(Rgb888::new(0, 0, 255)),
        ICE,
    );
    for from in [0, 6, 12] {
        let mut pixels = buffer(WIDTH, 3);
        let mut band = Canvas::new(&mut pixels, WIDTH, 3);
        Rink { y: -from, ..STRIP }.draw(&mut band, AWAY_END, HOME_END);
        let rows = &RINK[from as usize..from as usize + 3];
        assert_picture(&band, (STRIP.x, 0), rows, &legend, "rink band");
        assert_untouched_outside(&band, (STRIP.x, 0, STRIP.width, 3), "rink band");
    }
}

/// The goal projection is the one `game/hockey.rs` derived from the rink
/// sprite's 124 × 15 rectangle at (2, 47), with those numbers written out: x
/// along 123 px for 200 ft from the left boards, y from the middle row over
/// 12 rows for 85 ft, the mark's bottom-right pixel on the point. Every goal
/// position the model can carry.
#[test]
fn the_projection_is_the_one_the_rink_sprite_had() {
    for x in i8::MIN..=i8::MAX {
        for y in i8::MIN..=i8::MAX {
            let (fx, fy) = (x as i32, y as i32);
            let expected = (2 + 123 * (100 + fx) / 200 - 1, 47 + 7 - fy * 12 / 85 - 1);
            let mark = GoalMark {
                side: Side::Away,
                x,
                y,
            };
            assert_eq!(mark_at(mark), expected, "goal at ({x}, {y}) ft");
        }
    }
}

/// The lines sit where the projection puts their feet.
#[test]
fn the_lines_are_where_their_feet_project() {
    let mut pixels = frame();
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    STRIP.draw_lines(&mut canvas);
    let (red, blue) = (Rgb888::new(200, 24, 24), Rgb888::new(30, 80, 240));
    for (feet, color) in [
        (rink::GOAL_LINE_FT, red),
        (rink::BLUE_LINE_FT, blue),
        (rink::CENTER_FT, red),
        (rink::LENGTH_FT - rink::BLUE_LINE_FT, blue),
        (rink::LENGTH_FT - rink::GOAL_LINE_FT, red),
    ] {
        let at = (STRIP.x_at(feet), STRIP.mid() - 3);
        assert_eq!(canvas.pixel_at(at.0, at.1), Some(color), "{feet} ft");
    }
    assert_eq!(STRIP.dy(0), 0);
    assert_eq!(STRIP.dy(42), -STRIP.dy(-42));
}

/// The three-stars icon, 5 × 5, gold.
#[rustfmt::skip]
const STAR: [&str; 5] = [
    "..g..",
    ".ggg.",
    "ggggg",
    ".ggg.",
    ".g.g.",
];

const GOLD: [(char, Option<Rgb888>); 1] = [('g', Some(Rgb888::new(255, 200, 40)))];

#[test]
fn the_star_is_its_picture() {
    assert_eq!(size(&STAR), (star::SIZE, star::SIZE));
    let (x, y) = (52, 15);
    let mut pixels = frame();
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    star::draw(&mut canvas, x, y);
    assert_picture(&canvas, (x, y), &STAR, &GOLD, "star");
    assert_untouched_outside(&canvas, (x, y, star::SIZE, star::SIZE), "star");
}

/// Off the frame's edges the star clips: the part that lands is the picture,
/// and nothing wraps.
#[test]
fn the_star_clips_at_the_edges() {
    for (x, y) in [(-2, -3), (125, 61), (-4, 62)] {
        let mut pixels = frame();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
        star::draw(&mut canvas, x, y);
        assert_picture(&canvas, (x, y), &STAR, &GOLD, "clipped star");
        assert_untouched_outside(&canvas, (x, y, star::SIZE, star::SIZE), "clipped star");
    }
}

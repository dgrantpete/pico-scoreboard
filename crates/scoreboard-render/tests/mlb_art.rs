//! The procedural count dot and inning arrows (BACKLOG 116), pinned to exact
//! pictures: the dot alone, a row of them through `widgets::count_dots` at
//! the tints and counts the renderers use, and both arrows where they sit.

mod picture;

use picture::{assert_picture, assert_untouched_outside, frame, size};
use scoreboard_model::Rgb888;
use scoreboard_render::blit::{Canvas, Slice};
use scoreboard_render::count_dot::{self, OUTLINE, UNFILLED};
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::inning_arrow::{self, DOWN_Y, UP_Y, X};
use scoreboard_render::widgets::count_dots;

/// A count dot: `o` its outline, `i` its interior.
#[rustfmt::skip]
const DOT: [&str; 4] = [
    ".oo.",
    "oiio",
    "oiio",
    ".oo.",
];

const WHITE: Rgb888 = Rgb888::new(255, 255, 255);
const GRAY: Rgb888 = Rgb888::new(86, 86, 86);

#[test]
fn the_count_dot_is_its_picture() {
    assert_eq!(size(&DOT), (count_dot::SIZE, count_dot::SIZE));
    let (x, y) = (71, 7);
    let mut pixels = frame();
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    count_dot::draw(&mut canvas, x, y, OUTLINE, UNFILLED);
    let legend = [('o', Some(WHITE)), ('i', Some(GRAY))];
    assert_picture(&canvas, (x, y), &DOT, &legend, "count dot");
    let size = count_dot::SIZE;
    assert_untouched_outside(&canvas, (x, y, size, size), "count dot");
}

/// A 19 px row is four dots a pixel apart. The tint, when there is one,
/// colours every outline and the counted dots' interiors; the rest stay gray.
/// Untinted, outlines are white — the strike row before the pulse, the
/// startup screen's attempts.
#[test]
fn a_row_of_dots_takes_its_tint_and_count() {
    const ROW: [&str; 4] = [
        ".oo.. .oo.. .oo.. .oo.",
        "o11o. o22o. o33o. o44o",
        "o11o. o22o. o33o. o44o",
        ".oo.. .oo.. .oo.. .oo.",
    ];
    // Spaces only separate the dots for reading. A digit `n` is dot `n`'s
    // interior: the outline's colour once `n` dots count, gray before.
    let row: Vec<String> = ROW.iter().map(|line| line.replace(' ', "")).collect();
    let rect = Slice {
        x: 61,
        y: 7,
        width: 19,
        height: 4,
    };
    for tint in [None, Some(Rgb888(0x4e008e)), Some(Rgb888::new(255, 40, 40))] {
        for filled in 0..=4u8 {
            let mut pixels = frame();
            let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
            count_dots(&mut canvas, rect, filled, tint);
            let outline = tint.unwrap_or(WHITE);
            let interior = |dot: u8| Some(if filled >= dot { outline } else { GRAY });
            let legend = [
                ('o', Some(outline)),
                ('1', interior(1)),
                ('2', interior(2)),
                ('3', interior(3)),
                ('4', interior(4)),
            ];
            let picture: Vec<&str> = row.iter().map(String::as_str).collect();
            let what = format!("{filled} counted, tint {tint:?}");
            assert_picture(&canvas, (rect.x, rect.y), &picture, &legend, &what);
            let bounds = (rect.x, rect.y, rect.width, rect.height);
            assert_untouched_outside(&canvas, bounds, &what);
        }
    }
}

/// The inning arrows, up and down, white.
#[rustfmt::skip]
const UP: [&str; 3] = [
    "..w..",
    ".www.",
    "wwwww",
];
#[rustfmt::skip]
const DOWN: [&str; 3] = [
    "wwwww",
    ".www.",
    "..w..",
];

/// Both arrows share a column left of the inning chip and meet on its middle
/// row, 33, where each one's wide row lies.
#[test]
fn the_inning_arrows_are_their_pictures_where_they_sit() {
    assert_eq!((X, UP_Y, DOWN_Y), (5, 31, 33));
    for (picture, y, up) in [(UP, UP_Y, true), (DOWN, DOWN_Y, false)] {
        assert_eq!(size(&picture), (inning_arrow::WIDTH, inning_arrow::HEIGHT));
        let mut pixels = frame();
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
        inning_arrow::draw(&mut canvas, X, y, up);
        let legend = [('w', Some(WHITE))];
        assert_picture(&canvas, (X, y), &picture, &legend, "inning arrow");
        let rect = (X, y, inning_arrow::WIDTH, inning_arrow::HEIGHT);
        assert_untouched_outside(&canvas, rect, "inning arrow");
        let wide_row = if up { y + 2 } else { y };
        assert_eq!(wide_row, 33);
    }
}

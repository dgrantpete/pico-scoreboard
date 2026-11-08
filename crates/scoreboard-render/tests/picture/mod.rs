//! Exact expected pixels, written as pictures: the procedural art's tests
//! (BACKLOG 116) compare a draw against rows of characters, one per pixel,
//! where `.` is a pixel the draw must leave alone and every other character
//! names a colour in the picture's legend.
//!
//! The pictures were transcribed by a script from the art's 8-bit exports on
//! 2026-11-08, after those exports had been proven to be the compiled sprites
//! the golden frames were drawn with; the legends carry the exports' colours
//! as literals, so a picture pins a module's colour constants as well as its
//! shape. A legend entry of `None` says that colour's pixels must be left
//! alone too — how a test asks for a drawing with part of it held back.
//!
//! The frame under every draw is filled with [`SENTINEL`] first, so "left
//! alone" is something a test can see.

#![allow(dead_code)]

use scoreboard_model::Rgb888;
use scoreboard_render::blit::{BYTES_PER_PIXEL, Canvas};
use scoreboard_render::geometry::{HEIGHT, WIDTH};

/// A colour no picture uses.
pub const SENTINEL: Rgb888 = Rgb888::new(1, 2, 3);

/// A `width` × `height` frame filled with [`SENTINEL`].
pub fn buffer(width: i32, height: i32) -> Vec<u8> {
    let pixel: [u8; BYTES_PER_PIXEL] = [SENTINEL.red(), SENTINEL.green(), SENTINEL.blue()];
    pixel.repeat((width * height) as usize)
}

/// A whole panel's frame filled with [`SENTINEL`].
pub fn frame() -> Vec<u8> {
    buffer(WIDTH, HEIGHT)
}

/// The picture's colour for `glyph`; panics on a glyph the legend lacks.
fn color(legend: &[(char, Option<Rgb888>)], glyph: char, what: &str) -> Option<Rgb888> {
    if glyph == '.' {
        return None;
    }
    legend
        .iter()
        .find(|(name, _)| *name == glyph)
        .unwrap_or_else(|| panic!("{what}: {glyph:?} is not in the legend"))
        .1
}

/// The window of `canvas` whose top-left is `(x0, y0)` is `picture`, pixel
/// for pixel: every lettered pixel its legend colour exactly, every `.` (or
/// `None` colour) still [`SENTINEL`]. Pixels of the picture that fall off the
/// canvas are skipped, so a clipped draw is checked by the part that lands.
/// Lists every mismatch, not the first.
pub fn assert_picture(
    canvas: &Canvas<'_>,
    (x0, y0): (i32, i32),
    picture: &[&str],
    legend: &[(char, Option<Rgb888>)],
    what: &str,
) {
    assert!(
        legend.iter().all(|(_, color)| *color != Some(SENTINEL)),
        "{what}: the sentinel is a legend colour"
    );
    let width = picture[0].chars().count();
    let mut wrong = Vec::new();
    for (y, row) in picture.iter().enumerate() {
        assert_eq!(row.chars().count(), width, "{what}: row {y} is ragged");
        for (x, glyph) in row.chars().enumerate() {
            let want = color(legend, glyph, what).unwrap_or(SENTINEL);
            let (px, py) = (x0 + x as i32, y0 + y as i32);
            let Some(got) = canvas.pixel_at(px, py) else {
                continue;
            };
            if got != want {
                wrong.push(format!(
                    "  ({x}, {y}) {glyph:?}: want {want:?}, got {got:?}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{what}: {} pixels differ from the picture\n{}",
        wrong.len(),
        wrong[..wrong.len().min(24)].join("\n")
    );
}

/// Every pixel of `canvas` outside the `width` × `height` window at
/// `(x0, y0)` still holds [`SENTINEL`]: the draw stayed inside its rectangle.
pub fn assert_untouched_outside(
    canvas: &Canvas<'_>,
    (x0, y0, width, height): (i32, i32, i32, i32),
    what: &str,
) {
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            let inside = (x0..x0 + width).contains(&x) && (y0..y0 + height).contains(&y);
            if !inside {
                assert_eq!(
                    canvas.pixel_at(x, y),
                    Some(SENTINEL),
                    "{what}: drew outside its rectangle at ({x}, {y})"
                );
            }
        }
    }
}

/// A picture's size, as `(width, height)`.
pub fn size(picture: &[&str]) -> (i32, i32) {
    (picture[0].chars().count() as i32, picture.len() as i32)
}

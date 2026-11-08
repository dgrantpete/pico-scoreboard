//! The three-stars final's star (NHL.md §6.5; BACKLOG 116): 5 × 5, gold,
//! drawn once per rank beside each star's name.
//!
//! A diamond on its centre column — one pixel at the tip, then rows
//! [`ROWS`] pixels either side of the centre: shoulders, arms, waist — over
//! two legs a column either side of the centre on the last row. At this size
//! the legs are what make it a star rather than a diamond.
//!
//! It was a layer of `hockey_layout.aseprite` drawn by
//! `tools/hockey_layout.lua` from a five-row picture; `tests/hockey_art.rs`
//! pins this to that layer's 8-bit export pixel for pixel. Not tinted: the
//! name beside it carries the team.

use crate::blit::Canvas;
use crate::rgb565;
use scoreboard_model::Rgb888;

/// The star's square edge, in pixels.
pub const SIZE: i32 = 5;
/// The centre column, from the left edge.
pub const CENTER: i32 = SIZE / 2;
/// How far each of the diamond's rows reaches either side of the centre,
/// top down: the tip, the shoulders, the arms, the waist. The legs are the
/// last row.
pub const ROWS: [i32; 4] = [0, 1, 2, 1];
/// The legs' distance either side of the centre.
pub const LEGS: i32 = 1;

pub const GOLD: Rgb888 = Rgb888::new(255, 200, 40);

/// An exact 8-bit colour as the canvas stores it: this module's only contact
/// with the pixel format, so the canvas's widening (BACKLOG 115) is this line.
const fn ink(color: Rgb888) -> u16 {
    rgb565(color.red(), color.green(), color.blue())
}

/// A star with its top-left at `(x, y)`.
pub fn draw(canvas: &mut Canvas<'_>, x: i32, y: i32) {
    let gold = ink(GOLD);
    let center = x + CENTER;
    for (row, reach) in ROWS.into_iter().enumerate() {
        canvas.hline(center - reach, y + row as i32, 2 * reach + 1, gold);
    }
    let legs = y + ROWS.len() as i32;
    canvas.pixel(center - LEGS, legs, gold);
    canvas.pixel(center + LEGS, legs, gold);
}

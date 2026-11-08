//! The inning arrow (BACKLOG 116): a 5 × 3 triangle left of the MLB inning
//! chip, up while the away side bats and down while the home side does.
//!
//! Rows of one, three and five pixels on a centre column, tip first for up
//! and last for down. The two arrows share column [`X`] and sit two rows
//! apart ([`UP_Y`], [`DOWN_Y`]), so their wide rows coincide on row 33 — the
//! inning chip's middle row (`MLB_LIVE.inning`, rows 30–36) — and each tip
//! points away from it.
//!
//! They were the `inning_top__absolute` and `inning_bottom__absolute` layers
//! of `mlb_layout.aseprite`; `tests/mlb_art.rs` pins this to their 8-bit
//! exports pixel for pixel. Not tinted.

use crate::blit::Canvas;
use scoreboard_model::Rgb888;

/// The triangle's base and height, in pixels.
pub const WIDTH: i32 = 5;
pub const HEIGHT: i32 = 3;

/// Where the arrows sit on the panel: their shared left column, and each
/// one's top row.
pub const X: i32 = 5;
pub const UP_Y: i32 = 31;
pub const DOWN_Y: i32 = 33;

pub const COLOR: Rgb888 = Rgb888::new(255, 255, 255);

/// An arrow with its top-left at `(x, y)`, pointing up or down.
pub fn draw(canvas: &mut Canvas<'_>, x: i32, y: i32, points_up: bool) {
    let center = x + WIDTH / 2;
    for step in 0..HEIGHT {
        let row = if points_up { step } else { HEIGHT - 1 - step };
        canvas.hline(center - step, y + row, 2 * step + 1, COLOR);
    }
}

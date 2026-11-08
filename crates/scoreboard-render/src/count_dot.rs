//! The count dot (BACKLOG 116): the 4 × 4 marker `widgets::count_dots` lays
//! out a pixel apart — balls, strikes and outs, a hockey side's skaters, an
//! NBA side's fouls, the startup screen's Wi-Fi attempts.
//!
//! A rounded square: a one-pixel ring with its four corners left out, around
//! a 2 × 2 interior. The renderer tints both parts. The ring is the outline —
//! [`OUTLINE`] untinted, or the whole row in one colour (the critical-count
//! pulse, a team's colour). The interior is the outline's colour when the dot
//! counts and [`UNFILLED`] when it does not, so an empty slot still shows as
//! a ring around something.
//!
//! It was the `dot__relative` layer of `mlb_layout.aseprite` until BACKLOG
//! 116 retired the Aseprite pipeline. `tests/mlb_art.rs` pins it pixel for
//! pixel, alone and as `widgets::count_dots` lays a row of them out.

use crate::blit::Canvas;
use scoreboard_model::Rgb888;

/// The dot's square edge, in pixels.
pub const SIZE: i32 = 4;

/// The untinted outline, and the interior of a dot that does not count.
pub const OUTLINE: Rgb888 = Rgb888::new(255, 255, 255);
pub const UNFILLED: Rgb888 = Rgb888::new(86, 86, 86);

/// A dot with its top-left at `(x, y)`; the four corner pixels are left as
/// they were.
pub fn draw(canvas: &mut Canvas<'_>, x: i32, y: i32, outline: Rgb888, interior: Rgb888) {
    let inner = SIZE - 2;
    canvas.hline(x + 1, y, inner, outline);
    canvas.hline(x + 1, y + SIZE - 1, inner, outline);
    canvas.vline(x, y + 1, inner, outline);
    canvas.vline(x + SIZE - 1, y + 1, inner, outline);
    canvas.fill_rect(x + 1, y + 1, inner, inner, interior);
}

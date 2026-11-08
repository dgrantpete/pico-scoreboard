//! The lock and unlock toasts' padlock (BACKLOG 116): 14 × 22, one colour,
//! centred on the panel over the dimmed frame.
//!
//! A body — the bottom rows from [`BODY_TOP`], full width, with the
//! [`KEYHOLE`] left undrawn as one plain 2 × 6 rectangle (a head-gap-stem
//! keyhole read as a stray horizontal line at panel size) — under a shackle:
//! a [`BAR`]-row bar across columns [`SHACKLE_LEFT`]–[`SHACKLE_RIGHT`] and a
//! [`LEG`]-wide leg down from each end. Closed, both legs run into the body.
//! Open lifts the shackle [`LIFT`] rows: the right leg stretches to stay
//! anchored in the body and the left keeps its [`LEFT_LEG`] rows, leaving a
//! gap under it. Both states share one rectangle with the body in the same
//! rows, so one position serves both and the body does not jump.
//!
//! It was two PNGs `tools/gen_toast_icons.py` wrote; this is that script's
//! geometry, and `tests/toast_art.rs` pins it to those PNGs pixel for pixel.
//! The colour is the toast's to pick — the palette's primary at the toast's
//! brightness — with [`COLOR`] the PNGs' white.

use crate::blit::Canvas;
use crate::rgb565;
use scoreboard_model::Rgb888;

/// The padlock's rectangle, open or closed.
pub const WIDTH: i32 = 14;
pub const HEIGHT: i32 = 22;
/// The body's top row; it runs to the bottom.
pub const BODY_TOP: i32 = 12;
/// The keyhole left undrawn in the body: `(x, y, width, height)`.
pub const KEYHOLE: (i32, i32, i32, i32) = (6, 14, 2, 6);
/// The shackle's outer columns, inclusive, its bar's rows and its legs'
/// width.
pub const SHACKLE_LEFT: i32 = 2;
pub const SHACKLE_RIGHT: i32 = 11;
pub const BAR: i32 = 2;
pub const LEG: i32 = 2;
/// The closed shackle's top row, and how far opening lifts it.
pub const SHACKLE_TOP: i32 = 3;
pub const LIFT: i32 = 3;
/// The left leg's length under the bar, open or closed: closed, exactly
/// down to the body.
pub const LEFT_LEG: i32 = BODY_TOP - SHACKLE_TOP - BAR;

/// The untinted padlock.
pub const COLOR: Rgb888 = Rgb888::new(255, 255, 255);

/// An exact 8-bit colour as the canvas stores it: this module's only contact
/// with the pixel format, so the canvas's widening (BACKLOG 115) is this line.
const fn ink(color: Rgb888) -> u16 {
    rgb565(color.red(), color.green(), color.blue())
}

/// A padlock with its top-left at `(x, y)`, open or closed, in `color`.
pub fn draw(canvas: &mut Canvas<'_>, x: i32, y: i32, open: bool, color: Rgb888) {
    let color = ink(color);

    // The body, as the four rectangles around the keyhole.
    let (hole_x, hole_y, hole_width, hole_height) = KEYHOLE;
    let below = hole_y + hole_height;
    canvas.fill_rect(x, y + BODY_TOP, WIDTH, hole_y - BODY_TOP, color);
    canvas.fill_rect(x, y + hole_y, hole_x, hole_height, color);
    let right = hole_x + hole_width;
    canvas.fill_rect(x + right, y + hole_y, WIDTH - right, hole_height, color);
    canvas.fill_rect(x, y + below, WIDTH, HEIGHT - below, color);

    // The shackle: the bar, the right leg down to the body, the left leg
    // its fixed length.
    let top = if open {
        SHACKLE_TOP - LIFT
    } else {
        SHACKLE_TOP
    };
    let span = SHACKLE_RIGHT - SHACKLE_LEFT + 1;
    canvas.fill_rect(x + SHACKLE_LEFT, y + top, span, BAR, color);
    let legs = top + BAR;
    let right_leg = SHACKLE_RIGHT + 1 - LEG;
    canvas.fill_rect(x + right_leg, y + legs, LEG, BODY_TOP - legs, color);
    canvas.fill_rect(x + SHACKLE_LEFT, y + legs, LEG, LEFT_LEG, color);
}

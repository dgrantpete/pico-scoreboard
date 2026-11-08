//! The skip toast's spinner ring (BACKLOG 116): twelve 3 × 3 dots on a ring
//! of radius about 12, which the toast lights as a comet chasing round once
//! a second.
//!
//! [`RING`] holds each dot's top-left, dot 0 at three o'clock and the rest
//! clockwise in 30° steps. The corners were hand-rounded in `display.py`'s
//! `_SPINNER_DOTS` and are kept verbatim: a trigonometric re-derivation
//! rounds differently and the ring stops looking even. Its bounding box is
//! [`WIDTH`] × [`HEIGHT`].
//!
//! Every colour is the caller's: [`draw`] asks for each dot's in angular
//! order and leaves a dot undrawn when it gets `None` — the dots outside the
//! comet's trail — so there is no untinted spinner and no default colour.
//! That is all the compiled sprite's palette did, minus an inversion: the
//! script that drew the sprite baked dot `k`'s colour as `(0, 0, 8(k + 1))`,
//! whose RGB565 value is `k + 1`, and `toast.rs` inverted the compiled
//! palette to learn which entry was which dot. Here a dot is its index; the
//! sprite and its script went with the Aseprite pipeline (BACKLOG 116).
//! `tests/toast_art.rs` pins the ring pixel for pixel, dot by dot.

use crate::blit::Canvas;
use scoreboard_model::Rgb888;

/// The ring's bounding box.
pub const WIDTH: i32 = 26;
pub const HEIGHT: i32 = 25;
/// Dots on the ring, and each one's square edge.
pub const DOTS: usize = 12;
pub const DOT: i32 = 3;
/// Each dot's top-left in the bounding box, in angular order: dot 0 at three
/// o'clock, then clockwise.
pub const RING: [(i32, i32); DOTS] = [
    (23, 11),
    (21, 17),
    (17, 21),
    (12, 22),
    (6, 21),
    (2, 17),
    (0, 11),
    (2, 5),
    (6, 1),
    (12, 0),
    (17, 1),
    (21, 5),
];

/// The ring with its bounding box's top-left at `(x, y)`; dot `k` in
/// `color(k)`, or not drawn at all where that is `None`.
pub fn draw(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    mut color: impl FnMut(usize) -> Option<Rgb888>,
) {
    for (dot, (dx, dy)) in RING.into_iter().enumerate() {
        if let Some(color) = color(dot) {
            canvas.fill_rect(x + dx, y + dy, DOT, DOT, color);
        }
    }
}

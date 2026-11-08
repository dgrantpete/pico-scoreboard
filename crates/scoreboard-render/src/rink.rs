//! The procedural NHL rink (NHL.md D1, D10; BACKLOG 116): a 200 × 85 ft
//! sheet squashed into the strip under the shots bar, with each team's end
//! tinted and every goal's mark projected onto it (D9).
//!
//! # Projection
//!
//! [`Rink::x_at`] and [`Rink::dy`] are the whole mapping, integer and exact:
//! the strip's columns span the sheet's [`LENGTH_FT`], so the live strip
//! ([`STRIP`], 124 × 15) is 0.615 px/ft along — `x + 123 · feet / 200`,
//! floored — and every line below is placed through `x_at`, so a goal on the
//! goal line lands on the goal line. Across the ice, a point `feet` from the
//! centre line sits `(height − 3) · feet / 85` rows from [`Rink::mid`]: the
//! boards' rows are left out of the sheet's [`WIDTH_FT`]. The model hands a
//! goal over in feet from centre ice with the away side attacking +x
//! (NHL.md §3.5), so its mark's centre is `x_at(LENGTH_FT / 2 + x)` along and
//! `mid() + dy(y)` down — the projection `game/hockey.rs` restated from the
//! compiled sprite's rectangle while the rink was art.
//!
//! # The drawing
//!
//! The boards are a one-pixel ring with rounded corners: on the row `n` in
//! from the top or bottom they start [`CORNER`]`[n]` columns in from each end,
//! and on the row next to the long boards they are [`CURVE`] pixels wide, so
//! the corner reads as a curve rather than a stair. Inside them, ice; each team's end, from the boards to its goal line, in a colour
//! the renderer picks per team (a third of the team's colour today, so the
//! goals plotted there outshine it). Then the lines across the ice: the goal
//! lines [`GOAL_LINE_FT`] in from each end in red, the blue lines
//! [`BLUE_LINE_FT`] in, two pixels wide, and the centre red line two pixels
//! wide at [`CENTER_FT`]. Each crease is a three-pixel bar against its goal
//! line with a pixel in front: a half-disc at this scale. No faceoff dots
//! (D10): single red pixels read as a red team's goals.
//!
//! The surface (ice and ends) and the lines are two passes, because the
//! renderer needs the second alone: a power play's glow fills the attacking
//! zone and then [`Rink::draw_lines`] puts the lines and creases back over it.
//! The passes are disjoint in what survives — nothing in the surface pass
//! shows where a line is drawn — so [`Rink::draw`] is exactly the two in turn.
//!
//! # Why code, not art
//!
//! The rink was drawn in code three times before this: the NHL mockup drew
//! it procedurally, `tools/hockey_layout.lua` re-authored it as Aseprite
//! layers from the same numbers, and `game/hockey.rs` re-derived the goal
//! projection from the compiled sprite's `POSITION`. This module is the Lua
//! script's port, number for number, and the place the projection's numbers
//! live; `tests/hockey_art.rs` pins it to the compiled sprite's 8-bit export
//! pixel for pixel. Integer arithmetic only: it is drawn every frame on
//! core 1.

use crate::blit::Canvas;
use scoreboard_model::Rgb888;

/// The rink's rectangle on the panel — the boards are its outer ring — and
/// the projection into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rink {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The live screen's rink: the full-width strip under the shots bar, columns
/// 2–125 and rows 47–61 (NHL.md §6.5).
pub const STRIP: Rink = Rink {
    x: 2,
    y: 47,
    width: 124,
    height: 15,
};

/// The sheet, in feet: boards to boards along, and across.
pub const LENGTH_FT: i32 = 200;
pub const WIDTH_FT: i32 = 85;
/// Each goal line's distance from its end boards, each blue line's, and the
/// centre red line's.
pub const GOAL_LINE_FT: i32 = 11;
pub const BLUE_LINE_FT: i32 = 75;
pub const CENTER_FT: i32 = LENGTH_FT / 2;

/// The boards' inset from each end on the rows 0, 1 and 2 in from the top or
/// bottom; every row further in starts at the end itself.
pub const CORNER: [i32; 3] = [4, 2, 1];
/// The end boards' width on the row next to the long boards, where the
/// corner steps furthest; one pixel on every other row.
pub const CURVE: i32 = 2;

pub const ICE: Rgb888 = Rgb888::new(14, 22, 34);
pub const BOARDS: Rgb888 = Rgb888::new(150, 160, 170);
pub const RED_LINE: Rgb888 = Rgb888::new(200, 24, 24);
pub const BLUE_LINE: Rgb888 = Rgb888::new(30, 80, 240);
pub const CREASE: Rgb888 = Rgb888::new(70, 140, 255);
/// The ends' untinted colours: the art's placeholders, pure red and pure
/// blue. The renderer always passes each team's tint instead.
pub const AWAY_END: Rgb888 = Rgb888::new(255, 0, 0);
pub const HOME_END: Rgb888 = Rgb888::new(0, 0, 255);

impl Rink {
    /// x of a point `feet` along the sheet from the left boards, `0..=200`.
    pub const fn x_at(&self, feet: i32) -> i32 {
        self.x + (self.width - 1) * feet / LENGTH_FT
    }

    /// The middle row: centre ice across.
    pub const fn mid(&self) -> i32 {
        self.y + self.height / 2
    }

    /// Row offset from [`Rink::mid`] of a point `feet` across from the centre
    /// line; positive feet are toward the top boards. Truncates toward zero,
    /// so the two halves mirror.
    pub const fn dy(&self, feet: i32) -> i32 {
        -(feet * (self.height - 3) / WIDTH_FT)
    }

    /// The whole rink: ice, each end in its tint, then [`Rink::draw_lines`].
    ///
    /// The surface goes down in bands of rows that share a boards inset: one
    /// row at a time through the rounded corners, then the straight middle in
    /// one go.
    pub fn draw(&self, canvas: &mut Canvas<'_>, away_end: Rgb888, home_end: Rgb888) {
        let corner = CORNER.len() as i32;
        for edge in 1..corner {
            for top in [self.y + edge, self.y + self.height - 1 - edge] {
                let inset = CORNER[edge as usize];
                self.surface(canvas, top, 1, inset, away_end, home_end);
            }
        }
        let middle = self.height - 2 * corner;
        self.surface(canvas, self.y + corner, middle, 0, away_end, home_end);
        self.draw_lines(canvas);
    }

    /// `rows` rows of the surface from `top`, inside boards `inset` columns
    /// in from each end: ice, with each team's end from the boards to its goal
    /// line over it.
    fn surface(
        &self,
        canvas: &mut Canvas<'_>,
        top: i32,
        rows: i32,
        inset: i32,
        away_end: Rgb888,
        home_end: Rgb888,
    ) {
        let (left, right) = (self.x + inset, self.x + self.width - 1 - inset);
        let away_goal = self.x_at(GOAL_LINE_FT);
        let home_goal = self.x_at(LENGTH_FT - GOAL_LINE_FT);
        fill(canvas, left + 1, top, right - left - 1, rows, ICE);
        fill(canvas, left + 1, top, away_goal - left - 1, rows, away_end);
        fill(
            canvas,
            home_goal + 1,
            top,
            right - home_goal - 1,
            rows,
            home_end,
        );
    }

    /// The boards, the lines across the ice and the creases, and nothing
    /// under them: what a fill over the ice needs put back on top.
    pub fn draw_lines(&self, canvas: &mut Canvas<'_>) {
        let (left, right) = (self.x, self.x + self.width - 1);
        let bottom = self.y + self.height - 1;
        let corner = CORNER.len() as i32;
        // The long boards, top and bottom, between the corners.
        for y in [self.y, bottom] {
            fill(
                canvas,
                left + CORNER[0],
                y,
                self.width - 2 * CORNER[0],
                1,
                BOARDS,
            );
        }
        // The end boards, straight between the corners...
        for x in [left, right] {
            fill(
                canvas,
                x,
                self.y + corner,
                1,
                self.height - 2 * corner,
                BOARDS,
            );
        }
        // ...and through each corner, a row at a time, inward from its inset.
        for edge in 1..corner {
            let inset = CORNER[edge as usize];
            let run = if edge == 1 { CURVE } else { 1 };
            for y in [self.y + edge, bottom - edge] {
                fill(canvas, left + inset, y, run, 1, BOARDS);
                fill(canvas, right - inset - (run - 1), y, run, 1, BOARDS);
            }
        }

        let away_goal = self.x_at(GOAL_LINE_FT);
        let home_goal = self.x_at(LENGTH_FT - GOAL_LINE_FT);
        let (away_blue, home_blue) = (self.x_at(BLUE_LINE_FT), self.x_at(LENGTH_FT - BLUE_LINE_FT));
        let center = self.x_at(CENTER_FT);
        for (x, width, color) in [
            (away_goal, 1, RED_LINE),
            (home_goal, 1, RED_LINE),
            // Each blue line widens toward centre ice, and the centre line
            // toward the away end.
            (away_blue, 2, BLUE_LINE),
            (home_blue - 1, 2, BLUE_LINE),
            (center - 1, 2, RED_LINE),
        ] {
            self.across(canvas, x, width, color);
        }

        // Each crease: a bar against the goal line, toward centre ice, and a
        // pixel in front of the bar's middle.
        let mid = self.mid();
        for (goal_line, toward_center) in [(away_goal, 1), (home_goal, -1)] {
            fill(canvas, goal_line + toward_center, mid - 1, 1, 3, CREASE);
            fill(canvas, goal_line + 2 * toward_center, mid, 1, 1, CREASE);
        }
    }

    /// A line `width` columns wide across the ice from column `x`, inside
    /// the boards on every row: through a corner whose inset reaches it, the
    /// line stops short.
    fn across(&self, canvas: &mut Canvas<'_>, x: i32, width: i32, color: Rgb888) {
        let reach = (x - self.x).min(self.x + self.width - 1 - (x + width - 1));
        if reach <= 0 {
            return;
        }
        let mut short = 0;
        for edge in 1..CORNER.len() as i32 {
            if CORNER[edge as usize] >= reach {
                short = edge;
            }
        }
        let rows = self.height - 2 - 2 * short;
        fill(canvas, x, self.y + 1 + short, width, rows, color);
    }
}

/// Every rectangle the rink draws goes through this one call. Inlined at each
/// of its call sites, the canvas's clipped fill cost kilobytes of flash for a
/// strip drawn once a frame; out of line it is one copy, and the rink stays a
/// list of rectangles.
#[inline(never)]
fn fill(canvas: &mut Canvas<'_>, x: i32, y: i32, width: i32, height: i32, color: Rgb888) {
    canvas.fill_rect(x, y, width, height, color);
}

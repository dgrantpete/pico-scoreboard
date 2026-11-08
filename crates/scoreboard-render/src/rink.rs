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
//! and where the curve meets the straight end boards ([`JOINS`]) they run a
//! pixel or two further in so the corner reads as a curve, not a stair. Inside
//! them, ice; each team's end, from the boards to its goal line, in a colour
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
use crate::rgb565;
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
/// Where the corner meets the straight boards: on the row `.0` in from the
/// top or bottom, the boards also fill columns `.1..=.2` in from each end.
pub const JOINS: [(i32, i32, i32); 2] = [(1, 2, 3), (2, 1, 1)];

pub const ICE: Rgb888 = Rgb888::new(14, 22, 34);
pub const BOARDS: Rgb888 = Rgb888::new(150, 160, 170);
pub const RED_LINE: Rgb888 = Rgb888::new(200, 24, 24);
pub const BLUE_LINE: Rgb888 = Rgb888::new(30, 80, 240);
pub const CREASE: Rgb888 = Rgb888::new(70, 140, 255);
/// The ends' untinted colours: the art's placeholders, pure red and pure
/// blue. The renderer always passes each team's tint instead.
pub const AWAY_END: Rgb888 = Rgb888::new(255, 0, 0);
pub const HOME_END: Rgb888 = Rgb888::new(0, 0, 255);

/// An exact 8-bit colour as the canvas stores it: this module's only contact
/// with the pixel format, so the canvas's widening (BACKLOG 115) is this line.
const fn ink(color: Rgb888) -> u16 {
    rgb565(color.red(), color.green(), color.blue())
}

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

    /// The boards' inset from each end on `row` of the strip.
    const fn inset(&self, row: i32) -> i32 {
        let edge = if row < self.height - 1 - row {
            row
        } else {
            self.height - 1 - row
        };
        if edge < CORNER.len() as i32 {
            CORNER[edge as usize]
        } else {
            0
        }
    }

    /// The whole rink: ice, each end in its tint, then [`Rink::draw_lines`].
    pub fn draw(&self, canvas: &mut Canvas<'_>, away_end: Rgb888, home_end: Rgb888) {
        let (ice, away_end, home_end) = (ink(ICE), ink(away_end), ink(home_end));
        let away_goal = self.x_at(GOAL_LINE_FT);
        let home_goal = self.x_at(LENGTH_FT - GOAL_LINE_FT);
        for row in 1..self.height - 1 {
            let y = self.y + row;
            let left = self.x + self.inset(row);
            let right = self.x + self.width - 1 - self.inset(row);
            canvas.hline(left + 1, y, right - left - 1, ice);
            canvas.hline(left + 1, y, away_goal - left - 1, away_end);
            canvas.hline(home_goal + 1, y, right - home_goal - 1, home_end);
        }
        self.draw_lines(canvas);
    }

    /// The boards, the lines across the ice and the creases, and nothing
    /// under them: what a fill over the ice needs put back on top.
    pub fn draw_lines(&self, canvas: &mut Canvas<'_>) {
        let boards = ink(BOARDS);
        let (left, right) = (self.x, self.x + self.width - 1);
        let bottom = self.y + self.height - 1;
        // The long boards, top and bottom, between the corners.
        for y in [self.y, bottom] {
            canvas.hline(left + CORNER[0], y, self.width - 2 * CORNER[0], boards);
        }
        // The end boards and the corners' curve, a pixel per row a side.
        for row in 1..self.height - 1 {
            let inset = self.inset(row);
            canvas.pixel(left + inset, self.y + row, boards);
            canvas.pixel(right - inset, self.y + row, boards);
        }
        for (edge, from, to) in JOINS {
            for y in [self.y + edge, bottom - edge] {
                for k in from..=to {
                    canvas.pixel(left + k, y, boards);
                    canvas.pixel(right - k, y, boards);
                }
            }
        }

        let away_goal = self.x_at(GOAL_LINE_FT);
        let home_goal = self.x_at(LENGTH_FT - GOAL_LINE_FT);
        let (away_blue, home_blue) = (self.x_at(BLUE_LINE_FT), self.x_at(LENGTH_FT - BLUE_LINE_FT));
        let center = self.x_at(CENTER_FT);
        for (x, color) in [
            (away_goal, RED_LINE),
            (home_goal, RED_LINE),
            // Each blue line widens toward centre ice.
            (away_blue, BLUE_LINE),
            (away_blue + 1, BLUE_LINE),
            (home_blue - 1, BLUE_LINE),
            (home_blue, BLUE_LINE),
            (center - 1, RED_LINE),
            (center, RED_LINE),
        ] {
            self.across(canvas, x, color);
        }

        // Each crease: a bar against the goal line, toward centre ice, and a
        // pixel in front of the bar's middle.
        let crease = ink(CREASE);
        let mid = self.mid();
        for (goal_line, toward_center) in [(away_goal, 1), (home_goal, -1)] {
            canvas.vline(goal_line + toward_center, mid - 1, 3, crease);
            canvas.pixel(goal_line + 2 * toward_center, mid, crease);
        }
    }

    /// A one-pixel line across the ice at column `x`, inside the boards on
    /// every row — so a line near an end would stop short of the corners.
    fn across(&self, canvas: &mut Canvas<'_>, x: i32, color: Rgb888) {
        let color = ink(color);
        for row in 1..self.height - 1 {
            let inset = self.inset(row);
            if x > self.x + inset && x < self.x + self.width - 1 - inset {
                canvas.pixel(x, self.y + row, color);
            }
        }
    }
}

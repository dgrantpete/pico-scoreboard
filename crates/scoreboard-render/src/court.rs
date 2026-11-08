//! The procedural NBA court (NBA.md D14): a 94 × 50 ft floor drawn into
//! whatever rectangle the geometry table hands it, carrying every made field
//! goal where it was made (D5).
//!
//! # Projection
//!
//! The model carries a make in feet — semantics: `along` the court from the
//! baseline the team attacks, `across` from the centre line — and this module
//! owns the pixels. [`Court::fx`] and [`Court::dy`] are the whole mapping:
//! the court's outer rectangle spans the floor's length and width, so the B5
//! court (124 × 37) is 1.31 px/ft along and 0.72 across, 1.8× anisotropic.
//! Every line below goes through the same two functions as the marks, so a
//! make at the rim lands on the rim at any ratio. ESPN's shot frame starts
//! [`ESPN_ORIGIN`] feet in from the baseline (its `y = 1` is the rim, §3.4),
//! so the screen hands a make in as `along = y + ESPN_ORIGIN`, `across = x`
//! — home's `x` already negated upstream, the rink's §3.5 rule.
//!
//! # The lines
//!
//! The rule book's numbers, in feet ([`LENGTH`] through [`PAINT_DEPTH`]): the
//! sidelines, baselines and half-court line in [`LINES`]; each end's lane
//! lines and free-throw line in the attacking team's colour at two thirds,
//! the floor between them left as [`WOOD`] so the marks that land in the
//! paint keep their contrast; the three-point line as a straight corner
//! segment and then an arc; the backboard as a three-row vertical and the rim
//! as one [`IRON`] pixel. No centre circle: at every ratio tried it squashed
//! into a dash, and the half-court line alone says "middle".
//!
//! Two rules keep the drawing honest at a strip's ratios. The arc is a
//! staircase of row runs, each joining the previous row's so the line is
//! continuous — but a run never steps past its own row's x, because at the
//! taller ratios several rows round to one column and overshooting put a nub
//! beside each. And below about 0.8 px/ft the backboard and the rim round to
//! the same column, so the rim takes the next column toward the court rather
//! than vanish into the board.
//!
//! # Why code, not art
//!
//! The rink is Aseprite art compiled to bytes, with its projection restated
//! in `hockey.rs` against the art's rectangle. The court is BACKLOG 116's
//! first step (NBA.md C4): the number that places a line is the number that
//! places a mark, in one file, and the rink's procedural rewrite takes this
//! shape next. The mockup's `court.rs` (`data/nba-design/mockup-harness`) is
//! the reference implementation this ports; `tests/court.rs` pins the two to
//! the byte. The mockup's study knobs — the filled paint, the detail toggle,
//! the optional wood — belonged to designs that were not chosen and are not
//! here (owner, 2026-11-08: one design, no knobs).

use crate::blit::Canvas;
use crate::{BLACK, mix, rgb565, scale};
use scoreboard_model::snapshot::Side;

/// The court's rectangle on the panel — the lines are its outer ring — and
/// the projection into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Court {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Which basket. The away side attacks the left one and the home side the
/// right, so a team's makes pile up under its own crest (D5). The rink tints
/// the end a team *defends*, which puts its goals under the other crest; a
/// court has no defended-end marker, so the lane follows the attacker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Left,
    Right,
}

impl From<Side> for End {
    fn from(side: Side) -> Self {
        match side {
            Side::Away => End::Left,
            Side::Home => End::Right,
        }
    }
}

/// The floor, in feet.
pub const LENGTH: f32 = 94.0;
pub const WIDTH: f32 = 50.0;
/// The rim's centre from the baseline, and the backboard's face.
pub const RIM: f32 = 5.25;
pub const BACKBOARD: f32 = 4.0;
/// The three-point arc's radius about the rim, and the corner three's
/// distance from the centre line, where the straight segment runs until the
/// arc reaches it.
pub const ARC: f32 = 23.75;
pub const CORNER: f32 = 22.0;
/// The paint's half-width, and its depth: the free-throw line.
pub const PAINT_HALF: f32 = 8.0;
pub const PAINT_DEPTH: f32 = 19.0;
/// Where ESPN's shot frame starts, in feet from the baseline: its `y = 1` is
/// the rim (NBA.md §3.4), so a make sits `y + ESPN_ORIGIN` feet along.
pub const ESPN_ORIGIN: f32 = RIM - 1.0;

/// The floor, the lines, the rim.
pub const WOOD: u16 = rgb565(30, 20, 10);
pub const LINES: u16 = rgb565(150, 150, 150);
pub const IRON: u16 = rgb565(255, 110, 0);

/// A mark's edge, in pixels: every make is 2 × 2, threes the same as twos
/// (D5; a 1 px mark was invisible on wood).
pub const MARK: i32 = 2;

impl Court {
    /// x of a point `feet` along the court from the left baseline.
    pub fn fx(&self, feet: f32) -> i32 {
        self.x + libm::roundf((self.width - 1) as f32 * feet / LENGTH) as i32
    }

    /// Row offset from the middle row of a point `feet` across the court from
    /// the centre line; positive is toward the top sideline.
    pub fn dy(&self, feet: f32) -> i32 {
        -(libm::roundf((self.height - 1) as f32 * feet / WIDTH) as i32)
    }

    /// The middle row: the centre line's.
    pub fn mid(&self) -> i32 {
        self.y + self.height / 2
    }

    /// x of a point `along` feet from the baseline at `end`.
    pub fn at(&self, end: End, along: f32) -> i32 {
        match end {
            End::Left => self.fx(along),
            End::Right => self.fx(LENGTH - along),
        }
    }

    /// The bare floor: wood, lines, each end's lane in its attacking team's
    /// colour, the arcs, the boards and the rims.
    pub fn draw(&self, canvas: &mut Canvas<'_>, away: u16, home: u16) {
        let (x0, x1) = (self.x, self.x + self.width - 1);
        let y0 = self.y;
        let mid = self.mid();
        canvas.fill_rect(x0 + 1, y0 + 1, self.width - 2, self.height - 2, WOOD);
        // Sidelines, baselines, the half-court line.
        canvas.rect(x0, y0, self.width, self.height, LINES);
        canvas.vline(self.fx(LENGTH / 2.0), y0 + 1, self.height - 2, LINES);

        let half = -self.dy(PAINT_HALF);
        for (end, team) in [(End::Left, away), (End::Right, home)] {
            // The lane lines and the free-throw line, from the baseline to
            // the paint's depth.
            let lane = scale(team, 2, 3);
            let (a, b) = (self.at(end, 0.0), self.at(end, PAINT_DEPTH));
            let (left, right) = (a.min(b), a.max(b));
            canvas.hline(left, mid - half, right - left + 1, lane);
            canvas.hline(left, mid + half, right - left + 1, lane);
            canvas.vline(b, mid - half, 2 * half + 1, lane);

            // The three-point line, from the corner rows in to the middle:
            // the corner segment is the first row's run from the baseline,
            // and every later row's run joins the previous row's.
            let corner_rows = -self.dy(CORNER);
            let mut prev: Option<i32> = None;
            for row in (0..=corner_rows).rev() {
                let across = (row as f32 * WIDTH / (self.height - 1) as f32).min(CORNER);
                let along = RIM + libm::sqrtf((ARC * ARC - across * across).max(0.0));
                let x = self.at(end, along);
                let from = match (prev, end) {
                    (None, End::Left) => x0 + 1,
                    (None, End::Right) => x1 - 1,
                    (Some(p), End::Left) => (p + 1).min(x),
                    (Some(p), End::Right) => (p - 1).max(x),
                };
                let (left, right) = (from.min(x), from.max(x));
                for offset in [-row, row] {
                    canvas.hline(left, mid + offset, right - left + 1, LINES);
                }
                prev = Some(x);
            }

            let board = self.at(end, BACKBOARD);
            canvas.vline(board, mid - 1, 3, LINES);
            let mut rim = self.at(end, RIM);
            if rim == board {
                rim += match end {
                    End::Left => 1,
                    End::Right => -1,
                };
            }
            canvas.pixel(rim, mid, IRON);
        }
    }

    /// The top-left pixel of a mark whose bottom-right pixel is the projected
    /// point, pulled inside the lines when the point is on or past them: a
    /// baseline make sits against the baseline, a heave from anywhere stays
    /// on the floor.
    fn origin(&self, end: End, along: f32, across: f32) -> (i32, i32) {
        let x = self.at(end, along) - (MARK - 1);
        let y = self.mid() + self.dy(across) - (MARK - 1);
        (
            x.clamp(self.x + 1, self.x + self.width - 1 - MARK),
            y.clamp(self.y + 1, self.y + self.height - 1 - MARK),
        )
    }

    /// A translucent make, `alpha`/256 of `color` over whatever is lit — the
    /// rink's D9 heat map at D5's lower alpha, so forty makes a side still
    /// grade. `along` feet from the attacked baseline, `across` feet from the
    /// centre line; the read-back is from the canvas, in the one draw that
    /// puts the marks there.
    pub fn plot_over(
        &self,
        canvas: &mut Canvas<'_>,
        end: End,
        along: f32,
        across: f32,
        color: u16,
        alpha: u32,
    ) {
        let (x, y) = self.origin(end, along, across);
        for py in y..y + MARK {
            for px in x..x + MARK {
                let under = canvas.pixel_at(px, py).unwrap_or(BLACK);
                canvas.pixel(px, py, mix(under, color, alpha));
            }
        }
    }

    /// An opaque mark in `color`: the newest make's blink.
    pub fn plot(&self, canvas: &mut Canvas<'_>, end: End, along: f32, across: f32, color: u16) {
        let (x, y) = self.origin(end, along, across);
        canvas.fill_rect(x, y, MARK, MARK, color);
    }
}

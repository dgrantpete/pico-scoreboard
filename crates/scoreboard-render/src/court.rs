//! The arena court (NCAAB.md D5, §3.11): a 94 × 50 ft floor seen from the
//! near side, carrying every made field goal where it was made (NBA.md D5),
//! on the lines of the league's rule book.
//!
//! # One camera
//!
//! [`Camera`] is the whole mapping from the floor to the panel: a mild
//! perspective from 200 ft back and 39° up, fitted to the geometry table's
//! court rectangle (B5's, 124 × 37), so the near sideline is 118 px, the far
//! one 97, the near half 20 rows and the far half 16. A point is `x` feet
//! along from the left baseline, `y` across from the centre line toward the
//! far sideline, `z` up. The marks go through it every frame; the lines,
//! keys, oval and hoops went through it once, on the host, into the tables
//! below — so a make and a line in the same place share a pixel.
//!
//! The wire's shot frame is feet from the backboard plane with the rim at
//! `y = 1` for every league (the extractor normalises each — NCAAB.md D4)
//! and feet from the centre line, home's already negated, so a make lands
//! `along = y + ORIGIN` feet from the baseline its side attacks, `across = x`.
//!
//! # Tables, not region tests (E1)
//!
//! The court's static pixels are a function of the camera and the rule book
//! alone, so `tests/court_compile.rs` draws them on the host by the owner's
//! pixel-art rules — shapes as regions, pixel-perfect edges, the boundary
//! last, the hoops at 1.5× width with a one-pixel knockout — and commits
//! them as [`generated::court`](crate::generated::court), failing whenever
//! the tables are stale. Per frame the device fills spans in the frame's
//! team colours, projects the marks and checks the knockout's pixels:
//! constant data, not memoisation, and nothing remembered between frames.
//!
//! # Draw order (§3.11)
//!
//! [`CourtRaster::draw_floor`] — wood, keys, lines, oval, arcs and boundary,
//! already composited in that order, so a lane's colour never crosses the
//! boundary — then the marks ([`Camera::plot_over`], the newest blinking
//! through [`Camera::plot`]), then the caption on
//! [`CourtRaster::clear_caption`]'s floor, then [`draw_hoops`].

use crate::blit::Canvas;
use crate::generated::court::{COURTS, FLOOR, HOOPS, KNOCKOUT};
use crate::{BLACK, Rgb888, mix, scale};
use scoreboard_model::basketball::CourtLines;
use scoreboard_model::snapshot::Side;

/// The court's rectangle on the panel, from the geometry table: the camera
/// is fitted to it — centre court on its middle column, the far sideline on
/// its top row, the near sideline on its bottom row.
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
/// court has no defended-end marker, so the key follows the attacker.
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

impl End {
    /// A point `along` feet from this end's baseline, in feet from the left
    /// baseline.
    pub fn feet_from_left(self, along: f32) -> f32 {
        match self {
            End::Left => along,
            End::Right => LENGTH - along,
        }
    }
}

/// The floor, in feet.
pub const LENGTH: f32 = 94.0;
pub const HALF_WIDTH: f32 = 25.0;
/// The rim's centre from the baseline, and the backboard's face.
pub const RIM: f32 = 5.25;
pub const BACKBOARD: f32 = 4.0;
/// The free-throw line from the baseline, and the radius of the free-throw
/// circle and the centre circle: every court's.
pub const PAINT_DEPTH: f32 = 19.0;
pub const CIRCLE: f32 = 6.0;
/// Where the wire's shot frame starts, in feet from the baseline: its
/// `y = 1` is the rim, so a make sits `y + ORIGIN` feet along.
pub const ORIGIN: f32 = RIM - 1.0;

/// The camera: its distance from centre court along its line of sight, and
/// its elevation above the floor.
pub const CAM_DISTANCE: f32 = 200.0;
pub const CAM_ELEVATION_DEG: f32 = 39.0;

/// The floor and its lines: plain wood (the planks and the far side's
/// darkening were invisible on the panel).
pub const WOOD: Rgb888 = Rgb888::new(30, 20, 10);
pub const LINES: Rgb888 = Rgb888::new(150, 150, 150);
/// The hoop: the rim, the net, the board's frame and face, the stanchion's
/// post and arm, and its padded base at half the post's brightness.
pub const IRON: Rgb888 = Rgb888::new(255, 110, 0);
pub const NET: Rgb888 = Rgb888::new(225, 225, 225);
pub const BOARD: Rgb888 = Rgb888::new(205, 205, 210);
pub const BOARD_FACE: Rgb888 = Rgb888::new(120, 125, 135);
pub const PAD: Rgb888 = Rgb888::new(78, 78, 84);
pub const PAD_BASE: Rgb888 = scale(PAD, 1, 2);

/// A mark's edge, in pixels: every make is 2 × 2, threes the same as twos
/// (D5; a 1 px mark was invisible on wood).
pub const MARK: i32 = 2;

/// A run of `len` pixels rightward from (`x`, `y`): the tables' one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub x: u8,
    pub y: u8,
    pub len: u8,
}

impl Run {
    pub const fn new(x: u8, y: u8, len: u8) -> Run {
        Run { x, y, len }
    }

    /// The run's first and last columns.
    pub const fn ends(self) -> (i32, i32) {
        (self.x as i32, self.x as i32 + self.len as i32 - 1)
    }
}

/// Runs drawn in one colour.
#[derive(Debug)]
pub struct Paint {
    pub color: Rgb888,
    pub runs: &'static [Run],
}

/// The pixels beside the board, the net and the rim, by what each returns
/// to: the floor where it is on the court (baselines included), black off it.
#[derive(Debug)]
pub struct Knockout {
    pub floor: &'static [Run],
    pub off: &'static [Run],
}

/// One rule book's static court.
#[derive(Debug)]
pub struct CourtRaster {
    pub lines: CourtLines,
    /// Every pixel in [`LINES`]: the boundary, the half-court line, the
    /// centre oval and both three-point lines.
    pub white: &'static [Run],
    /// Each key — the lane and the free-throw circle's outer half, one shape,
    /// and the free-throw line — left (away) then right (home), drawn in the
    /// attacking team's colour at two thirds.
    pub keys: [&'static [Run]; 2],
    /// The caption box's floor, clipped between the three-point arcs.
    pub caption: &'static [Run],
}

/// The static court for `lines`: one of the rule books the tables were
/// compiled for, or the NBA's for any other.
pub fn raster(lines: CourtLines) -> &'static CourtRaster {
    COURTS
        .iter()
        .copied()
        .find(|raster| raster.lines == lines)
        .unwrap_or(COURTS[0])
}

fn fill(canvas: &mut Canvas<'_>, runs: &[Run], color: Rgb888) {
    for run in runs {
        canvas.hline(run.x as i32, run.y as i32, run.len as i32, color);
    }
}

impl CourtRaster {
    /// The bare floor: wood baseline to baseline, the lines, and each key in
    /// its attacking team's colour at two thirds, so the marks that land in
    /// the paint keep their contrast.
    pub fn draw_floor(&self, canvas: &mut Canvas<'_>, away: Rgb888, home: Rgb888) {
        fill(canvas, &FLOOR, WOOD);
        fill(canvas, self.white, LINES);
        fill(canvas, self.keys[0], scale(away, 2, 3));
        fill(canvas, self.keys[1], scale(home, 2, 3));
    }

    /// Bare wood under the caption, erasing the marks and the half-court line
    /// there but stopping at the three-point arcs, which stay whole.
    pub fn clear_caption(&self, canvas: &mut Canvas<'_>) {
        fill(canvas, self.caption, WOOD);
    }
}

/// Both hoops — stanchion, board, net, rim — then the knockout: every pixel
/// beside the board, net or rim that still shows [`LINES`] returns to what
/// the floor shows there, so a baseline reads as passing behind the board
/// instead of fusing with it. Line pixels only: a mark or the stanchion
/// keeps its own.
pub fn draw_hoops(canvas: &mut Canvas<'_>) {
    for paint in &HOOPS {
        fill(canvas, paint.runs, paint.color);
    }
    for (runs, under) in [(KNOCKOUT.floor, WOOD), (KNOCKOUT.off, BLACK)] {
        for run in runs {
            let (first, last) = run.ends();
            let y = run.y as i32;
            for x in first..=last {
                if canvas.pixel_at(x, y) == Some(LINES) {
                    canvas.pixel(x, y, under);
                }
            }
        }
    }
}

/// The perspective, fitted to a [`Court`]: centre court on the rectangle's
/// middle column, the far sideline on its top row and the near sideline on
/// its bottom row, so the floor's height sets the scale and the near
/// sideline spans 118 of B5's 124 columns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    court: Court,
    /// The screen column of the half-court line and the row of the centre
    /// line, fractional.
    cx: f32,
    cy: f32,
    /// The focal length, in pixels.
    f: f32,
    /// The elevation's sine and cosine.
    sin: f32,
    cos: f32,
}

impl Camera {
    pub fn new(court: &Court) -> Camera {
        let (sin, cos) = libm::sincosf(CAM_ELEVATION_DEG.to_radians());
        let (near, far) = (
            CAM_DISTANCE - HALF_WIDTH * cos,
            CAM_DISTANCE + HALF_WIDTH * cos,
        );
        let f = (court.height - 1) as f32 / (HALF_WIDTH * sin * (1.0 / near + 1.0 / far));
        Camera {
            court: *court,
            cx: court.x as f32 + (court.width - 1) as f32 / 2.0,
            cy: court.y as f32 + f * HALF_WIDTH * sin / far,
            f,
            sin,
            cos,
        }
    }

    /// The rectangle the camera is fitted to.
    pub fn court(&self) -> Court {
        self.court
    }

    /// The screen position of a point `x` feet along from the left baseline,
    /// `y` feet across from the centre line (positive toward the far
    /// sideline) and `z` feet up.
    pub fn project(&self, x: f32, y: f32, z: f32) -> (f32, f32) {
        let depth = CAM_DISTANCE + y * self.cos - z * self.sin;
        (
            self.cx + self.f * (x - LENGTH / 2.0) / depth,
            self.cy - self.f * (y * self.sin + z * self.cos) / depth,
        )
    }

    /// [`Camera::project`], to the nearest pixel.
    pub fn px(&self, x: f32, y: f32, z: f32) -> (i32, i32) {
        let (column, row) = self.project(x, y, z);
        (libm::roundf(column) as i32, libm::roundf(row) as i32)
    }

    /// The floor's `y` under screen row `row`: the inverse of
    /// [`Camera::project`] at `z = 0`.
    pub fn floor_y(&self, row: i32) -> f32 {
        let k = (self.cy - row as f32) / self.f;
        k * CAM_DISTANCE / (self.sin - k * self.cos)
    }

    /// The floor's `x` under pixel column `column` on screen row `row`.
    pub fn floor_x(&self, column: i32, row: i32) -> f32 {
        let y = self.floor_y(row);
        LENGTH / 2.0 + (column as f32 - self.cx) * (CAM_DISTANCE + y * self.cos) / self.f
    }

    /// The left and right baselines' columns on `row`.
    pub fn edges(&self, row: i32) -> (i32, i32) {
        let y = self.floor_y(row);
        (self.px(0.0, y, 0.0).0, self.px(LENGTH, y, 0.0).0)
    }

    /// The top-left pixel of a mark whose bottom-right pixel is the projected
    /// point, pulled inside the boundary when the point is on or past it: a
    /// baseline make sits against the baseline, a heave from anywhere stays
    /// on the floor. The row is held off the sidelines, then the column off
    /// the baselines on the mark's top row — the narrower of its two.
    fn origin(&self, end: End, along: f32, across: f32) -> (i32, i32) {
        let (x, y) = self.px(end.feet_from_left(along), across, 0.0);
        let court = self.court;
        let y = (y - (MARK - 1)).clamp(court.y + 1, court.y + court.height - 1 - MARK);
        let (left, right) = floor_span(y);
        ((x - (MARK - 1)).clamp(left + 1, right - MARK), y)
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
        color: Rgb888,
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
    pub fn plot(&self, canvas: &mut Canvas<'_>, end: End, along: f32, across: f32, color: Rgb888) {
        let (x, y) = self.origin(end, along, across);
        canvas.fill_rect(x, y, MARK, MARK, color);
    }
}

/// The floor's first and last columns on `row`, from the table the floor is
/// drawn by — so the marks keep to the floor the panel shows.
fn floor_span(row: i32) -> (i32, i32) {
    let top = FLOOR[0].y as i32;
    let index = (row - top).clamp(0, FLOOR.len() as i32 - 1) as usize;
    FLOOR[index].ends()
}

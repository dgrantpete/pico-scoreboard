//! The arena court's compile step (NCAAB.md E1): the court's static pixels,
//! drawn here on the host by §3.11's rules, become the committed
//! `src/generated/court.rs`, and this test is the compiler.
//!
//! Everything the device would otherwise test per pixel per frame — about
//! four thousand inside tests for the keys and the oval, the hoop's quads and
//! lines, the knockout's neighbourhoods — depends only on the camera, the
//! court's rectangle and the rule book, so it is computed once, here, and the
//! device fills the tables (`court.rs`). One source of truth stays the code:
//! this file holds the drawing, and
//!
//! ```text
//! SCOREBOARD_BLESS_COURT=1 cargo test -p scoreboard-render --test court_compile
//! ```
//!
//! rewrites the module from it; without the variable the test fails whenever
//! the committed module is not exactly what the code draws — `art_compile.rs`'s
//! pattern. The comparison ignores CRLF, as that test's does.
//!
//! # The rules (the owner's, from the panel review)
//!
//! - **Shapes are regions.** The keys — the lane box and the free-throw
//!   circle's outer half, one shape — and the centre oval are digitised by
//!   pixel centre (a pixel is in when its centre is) and drawn as the
//!   region's edge: inside pixels with an outside side-neighbour that is on
//!   the court. Flats land at every extreme and tangent lines meet by
//!   construction. The free-throw line is the last pixel short of 19 ft on
//!   each lane row.
//! - **Pixel-perfect:** an edge pixel that is the corner of an L — exactly
//!   two edge neighbours, one beside and one above or below — is dropped.
//! - **The three-point line** is a staircase of row runs, each joining the
//!   previous row's, from the corner rows in to the middle.
//! - **The boundary draws last**, so the lanes' colours end against it.
//! - **The hoops** — stanchion, board, net, rim at 1.5× width and 1.06×
//!   height — then a one-pixel knockout: a line pixel beside a pixel the
//!   board, net or rim drew returns to the floor (black off it). The
//!   stanchion is drawn before the knockout looks, so it neither knocks nor
//!   is knocked.
//!
//! The reviewed prototype these port is `data/ncaab-design/render-ncaa/src/
//! arena.rs`, view 4; the port matched it byte for byte (B7's oracle).

use std::collections::BTreeSet;
use std::path::PathBuf;

use scoreboard_model::Rgb888;
use scoreboard_model::basketball::CourtLines;
use scoreboard_render::blit::{BYTES_PER_PIXEL, Canvas, Slice};
use scoreboard_render::court::{
    self, BOARD, BOARD_FACE, CIRCLE, Camera, End, IRON, LENGTH, LINES, NET, PAD, PAD_BASE,
    PAINT_DEPTH, RIM, Run, WOOD,
};
use scoreboard_render::geometry::{BASKETBALL_LIVE, HEIGHT, WIDTH};
use scoreboard_render::{BLACK, scale};

/// Set to rewrite the generated module instead of comparing against it.
const BLESS_VAR: &str = "SCOREBOARD_BLESS_COURT";

/// The rule books compiled, by the name of their static: the NBA's first,
/// the court an unknown rule book falls back to.
const RULE_BOOKS: [(&str, CourtLines); 2] = [("NBA", CourtLines::NBA), ("NCAA", CourtLines::NCAA)];

/// The hoop's proportions on the panel: wider than true so the rim and net
/// read, a little taller with them; the band above caps the height.
const HOOP_WIDTH: f32 = 1.5;
const HOOP_HEIGHT: f32 = 1.06;

/// The square around the board's middle in which the knockout looks.
const KNOCK_HALF: i32 = 12;

/// The hoops' colours in drawing order, by the name the module uses.
const HOOP_PAINTS: [(&str, Rgb888); 6] = [
    ("PAD_BASE", PAD_BASE),
    ("PAD", PAD),
    ("BOARD_FACE", BOARD_FACE),
    ("BOARD", BOARD),
    ("NET", NET),
    ("IRON", IRON),
];

fn generated_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("generated")
        .join("court.rs")
}

fn camera() -> Camera {
    Camera::new(&BASKETBALL_LIVE.court)
}

// -- A panel-sized grid ----------------------------------------------------------

#[derive(Clone)]
struct Grid<T> {
    cells: Vec<T>,
}

impl<T: Copy> Grid<T> {
    fn new(fill: T) -> Self {
        Grid {
            cells: vec![fill; (WIDTH * HEIGHT) as usize],
        }
    }

    fn contains(x: i32, y: i32) -> bool {
        (0..WIDTH).contains(&x) && (0..HEIGHT).contains(&y)
    }

    fn get(&self, x: i32, y: i32) -> Option<T> {
        Self::contains(x, y).then(|| self.cells[(y * WIDTH + x) as usize])
    }

    /// One pixel, dropped off the panel — `Canvas::pixel`.
    fn set(&mut self, x: i32, y: i32, value: T) {
        if Self::contains(x, y) {
            self.cells[(y * WIDTH + x) as usize] = value;
        }
    }

    /// A row of `width` pixels, clipped to the panel — `Canvas::hline`.
    fn hline(&mut self, x: i32, y: i32, width: i32, value: T) {
        for column in x.max(0)..(x + width).min(WIDTH) {
            self.set(column, y, value);
        }
    }

    /// One pixel, kept inside the panel's safe ring and below the band.
    fn put(&mut self, x: i32, y: i32, value: T) {
        if in_ring(x, y) {
            self.set(x, y, value);
        }
    }

    /// Every maximal run of pixels `pick` keeps, row-major.
    fn runs(&self, pick: impl Fn(T) -> bool) -> Vec<Run> {
        let mut runs = Vec::new();
        for y in 0..HEIGHT {
            let mut x = 0;
            while x < WIDTH {
                if !pick(self.get(x, y).unwrap()) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < WIDTH && pick(self.get(x, y).unwrap()) {
                    x += 1;
                }
                runs.push(Run::new(start as u8, y as u8, (x - start) as u8));
            }
        }
        runs
    }
}

/// The court's ink stays in rows 25–62 and columns 1–126: under the band and
/// off the panel's edge ring.
fn in_ring(x: i32, y: i32) -> bool {
    (1..=WIDTH - 2).contains(&x) && (25..=HEIGHT - 2).contains(&y)
}

fn runs_from(set: &BTreeSet<(i32, i32)>) -> Vec<Run> {
    let mut grid = Grid::new(false);
    for &(x, y) in set {
        grid.set(x, y, true);
    }
    grid.runs(|on| on)
}

// -- The floor -------------------------------------------------------------------

/// What a pixel of the bare court shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    Off,
    Wood,
    Line,
    Key(End),
}

/// The edge of a floor region: every pixel whose centre is `inside` (floor
/// feet) with a side-neighbour that is not but `counts` — on the court for a
/// key, whose baseline side is the court's own line — less the corner pixel
/// of every L.
fn region_edge(
    grid: &mut Grid<Ink>,
    cam: &Camera,
    ink: Ink,
    inside: impl Fn(f32, f32) -> bool,
    counts: impl Fn(f32, f32) -> bool,
) {
    let court = cam.court();
    let (x0, y0) = (1, court.y);
    let (columns, rows) = (WIDTH - 2, court.height);
    let at = |x: i32, y: i32| inside(cam.floor_x(x, y), cam.floor_y(y));
    let mut edge = Grid::new(false);
    for y in y0..y0 + rows {
        for x in x0..x0 + columns {
            if !at(x, y) {
                continue;
            }
            let outlined = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| {
                let (nx, ny) = (x + dx, y + dy);
                !at(nx, ny) && counts(cam.floor_x(nx, ny), cam.floor_y(ny))
            });
            edge.set(x, y, outlined);
        }
    }
    let on = |x: i32, y: i32| {
        (x0..x0 + columns).contains(&x)
            && (y0..y0 + rows).contains(&y)
            && edge.get(x, y) == Some(true)
    };
    for y in y0..y0 + rows {
        for x in x0..x0 + columns {
            if !on(x, y) {
                continue;
            }
            let near: Vec<(i32, i32)> = (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| (dx, dy) != (0, 0) && on(x + dx, y + dy))
                .collect();
            let corner = near.len() == 2 && {
                let (a, b) = (near[0], near[1]);
                (a.1 == 0 && b.0 == 0) || (a.0 == 0 && b.1 == 0)
            };
            if !corner {
                grid.put(x, y, ink);
            }
        }
    }
}

/// The bare court for one rule book, in §3.11's order: wood and the half-court
/// line; the centre oval; each key, its free-throw line and its three-point
/// line; the boundary last.
fn floor(cam: &Camera, lines: CourtLines) -> Grid<Ink> {
    let (arc, corner, lane_half) = (
        lines.arc.feet(),
        lines.corner.feet(),
        lines.lane_half.feet(),
    );
    let court = cam.court();
    let (top, bottom) = (court.y, court.y + court.height - 1);
    let mut grid = Grid::new(Ink::Off);

    for row in top..=bottom {
        let (l, r) = cam.edges(row);
        if row == top || row == bottom {
            grid.hline(l, row, r - l + 1, Ink::Line);
            continue;
        }
        grid.hline(l + 1, row, r - l - 1, Ink::Wood);
        grid.set(l, row, Ink::Line);
        grid.set(r, row, Ink::Line);
        let half = cam.px(LENGTH / 2.0, cam.floor_y(row), 0.0).0;
        grid.set(half, row, Ink::Line);
    }

    region_edge(
        &mut grid,
        cam,
        Ink::Line,
        |x, y| {
            let dx = x - LENGTH / 2.0;
            dx * dx + y * y <= CIRCLE * CIRCLE
        },
        |_, _| true,
    );

    for end in [End::Left, End::Right] {
        let ink = Ink::Key(end);
        let key = move |x: f32, y: f32| {
            let along = end.feet_from_left(x);
            let lane = (0.0..=PAINT_DEPTH).contains(&along) && y.abs() <= lane_half;
            let beyond = along - PAINT_DEPTH;
            let circle = along >= PAINT_DEPTH && beyond * beyond + y * y <= CIRCLE * CIRCLE;
            lane || circle
        };
        region_edge(&mut grid, cam, ink, key, move |x, _| {
            end.feet_from_left(x) >= 0.0
        });

        // The free-throw line: on each lane row, the last pixel whose centre
        // is short of 19 ft — the rule the region's edge used.
        let toward = match end {
            End::Left => 1,
            End::Right => -1,
        };
        for row in top..=bottom {
            let y = cam.floor_y(row);
            if y.abs() > lane_half {
                continue;
            }
            let mut x = cam.px(end.feet_from_left(PAINT_DEPTH), y, 0.0).0;
            while end.feet_from_left(cam.floor_x(x, row)) > PAINT_DEPTH {
                x -= toward;
            }
            while end.feet_from_left(cam.floor_x(x + toward, row)) <= PAINT_DEPTH {
                x += toward;
            }
            grid.set(x, row, ink);
        }

        // The three-point line: each half from its corner row in to the
        // middle, the first row's run from the baseline, every later row's
        // joining the previous row's.
        let arc_x = |y: f32| RIM + libm::sqrtf((arc * arc - y * y).max(0.0));
        let mid_row = cam.px(LENGTH / 2.0, 0.0, 0.0).1;
        for sign in [1.0f32, -1.0] {
            let corner_row = cam.px(LENGTH / 2.0, sign * corner, 0.0).1;
            let step = if sign > 0.0 { 1 } else { -1 };
            let mut prev: Option<i32> = None;
            let mut row = corner_row;
            loop {
                let y = match prev {
                    None => sign * corner,
                    Some(_) => cam.floor_y(row).clamp(-corner, corner),
                };
                let x = cam.px(end.feet_from_left(arc_x(y)), y, 0.0).0;
                let start = match (prev, end) {
                    (None, _) => cam.px(end.feet_from_left(0.0), y, 0.0).0 + toward,
                    (Some(p), End::Left) => (p + 1).min(x),
                    (Some(p), End::Right) => (p - 1).max(x),
                };
                grid.hline(start.min(x), row, (start - x).abs() + 1, Ink::Line);
                prev = Some(x);
                if row == mid_row {
                    break;
                }
                row += step;
            }
        }
    }

    // The sidelines and baselines last: one unbroken edge the lanes' lines
    // end against rather than cross.
    for row in top..=bottom {
        let (l, r) = cam.edges(row);
        if row == top || row == bottom {
            grid.hline(l, row, r - l + 1, Ink::Line);
        } else {
            grid.set(l, row, Ink::Line);
            grid.set(r, row, Ink::Line);
        }
    }
    grid
}

/// The caption box's floor: each of its rows between the baselines, and
/// between the three-point arcs where a row crosses them — the perspective
/// brings the arcs in across the box's corners. It still clears the
/// half-court line under the text.
fn caption(cam: &Camera, lines: CourtLines, area: Slice) -> Vec<Run> {
    let (arc, corner) = (lines.arc.feet(), lines.corner.feet());
    let mut runs = Vec::new();
    for row in area.y..area.y + area.height {
        let (mut l, mut r) = cam.edges(row);
        let y = cam.floor_y(row);
        if y.abs() <= corner {
            let along = RIM + libm::sqrtf((arc * arc - y * y).max(0.0));
            l = l.max(cam.px(End::Left.feet_from_left(along), y, 0.0).0);
            r = r.min(cam.px(End::Right.feet_from_left(along), y, 0.0).0);
        }
        let (a, b) = (area.x.max(l + 1), (area.x + area.width - 1).min(r - 1));
        if b >= a {
            runs.push(Run::new(a as u8, row as u8, (b - a + 1) as u8));
        }
    }
    runs
}

// -- The hoops -------------------------------------------------------------------

/// Bresenham's pixels from `a` to `b`, both ends included.
fn line(grid: &mut Grid<Option<Rgb888>>, (x0, y0): (i32, i32), (x1, y1): (i32, i32), c: Rgb888) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    loop {
        grid.put(x, y, Some(c));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// A convex quad, filled: every pixel whose centre is inside it or within
/// half a pixel of an edge, so a quad seen edge-on still draws its sliver.
fn quad(grid: &mut Grid<Option<Rgb888>>, pts: [(f32, f32); 4], c: Rgb888) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    for py in libm::roundf(y0) as i32..=libm::roundf(y1) as i32 {
        for px in libm::roundf(x0) as i32..=libm::roundf(x1) as i32 {
            let (x, y) = (px as f32, py as f32);
            let mut sign = 0i32;
            let mut inside = true;
            let mut near = false;
            for i in 0..4 {
                let (ax, ay) = pts[i];
                let (bx, by) = pts[(i + 1) % 4];
                let (ex, ey) = (bx - ax, by - ay);
                let cross = ex * (y - ay) - ey * (x - ax);
                let s = if cross > 0.0 {
                    1
                } else if cross < 0.0 {
                    -1
                } else {
                    0
                };
                if s != 0 {
                    if sign == 0 {
                        sign = s;
                    } else if s != sign {
                        inside = false;
                    }
                }
                let len2 = ex * ex + ey * ey;
                let t = if len2 > 0.0 {
                    (((x - ax) * ex + (y - ay) * ey) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (qx, qy) = (ax + t * ex - x, ay + t * ey - y);
                if qx * qx + qy * qy <= 0.25 {
                    near = true;
                }
            }
            if inside || near {
                grid.put(px, py, Some(c));
            }
        }
    }
}

/// One end's hoop, drawn in two layers: the stanchion, which is down before
/// the knockout looks, and the board, net and rim, whose pixels knock out the
/// line pixels beside them. And the square the knockout looks in.
struct Hoop {
    stanchion: Grid<Option<Rgb888>>,
    front: Grid<Option<Rgb888>>,
    square: (i32, i32),
}

/// One end's hoop, back to front as the camera at centre court sees it.
fn hoop(cam: &Camera, end: End) -> Hoop {
    let (w, h) = (HOOP_WIDTH, HOOP_HEIGHT);
    let p = |along: f32, across: f32, z: f32| cam.project(end.feet_from_left(along), across, z);
    let q = |along: f32, across: f32, z: f32| cam.px(end.feet_from_left(along), across, z);

    // The stanchion: a padded base on the floor behind the baseline, a post,
    // and the arm up to the back of the board.
    let mut stanchion = Grid::new(None);
    let base = [
        p(-9.0, 0.0, 0.0),
        p(-3.0, 0.0, 0.0),
        p(-3.0, 0.0, 2.5),
        p(-9.0, 0.0, 2.5),
    ];
    quad(&mut stanchion, base, PAD_BASE);
    line(
        &mut stanchion,
        q(-5.0, 0.0, 2.5),
        q(-5.0, 0.0, 8.5 * h),
        PAD,
    );
    line(
        &mut stanchion,
        q(-5.0, 0.0, 8.5 * h),
        q(3.8, 0.0, 11.0 * h),
        PAD,
    );

    // The board: 6 ft across, 9.5 to 13 ft up, its face 4 ft in, outlined.
    let mut front = Grid::new(None);
    let half = 3.0 * w.min(1.5);
    let (lo, hi) = (9.5 * h, 13.0 * h);
    let face = [
        p(4.0, -half, lo),
        p(4.0, half, lo),
        p(4.0, half, hi),
        p(4.0, -half, hi),
    ];
    quad(&mut front, face, BOARD_FACE);
    for i in 0..4 {
        let (a, b) = (face[i], face[(i + 1) % 4]);
        let at = |(x, y): (f32, f32)| (libm::roundf(x) as i32, libm::roundf(y) as i32);
        line(&mut front, at(a), at(b), BOARD);
    }

    // The net, then the rim over it: 18 in across, 10 ft up, its centre
    // 15 in off the board.
    let (rim_r, rim_x, rim_z) = (0.75 * w, 4.0 + 1.25 * w, 10.0 * h);
    let (net_r, net_z) = (0.45 * w, rim_z - 1.5 * w * h);
    for k in 0..8 {
        let a = k as f32 * core::f32::consts::PI / 4.0;
        let (ca, sa) = (libm::cosf(a), libm::sinf(a));
        line(
            &mut front,
            q(rim_x + rim_r * ca, rim_r * sa, rim_z),
            q(rim_x + net_r * ca, net_r * sa, net_z),
            NET,
        );
    }
    for k in 0..16 {
        let a = k as f32 * core::f32::consts::PI / 8.0;
        let (x, y) = q(rim_x + rim_r * libm::cosf(a), rim_r * libm::sinf(a), rim_z);
        front.put(x, y, Some(IRON));
    }

    let (cx, cy) = q(4.0, 0.0, 11.0 * h);
    Hoop {
        stanchion,
        front,
        square: (cx - KNOCK_HALF, cy - KNOCK_HALF),
    }
}

/// Inside a hoop's knockout square.
fn in_square((sx, sy): (i32, i32), x: i32, y: i32) -> bool {
    (sx..=sx + 2 * KNOCK_HALF).contains(&x) && (sy..=sy + 2 * KNOCK_HALF).contains(&y)
}

/// Whether a knocked-out pixel returns to the floor (wood, baselines
/// included) or to black.
fn on_floor(cam: &Camera, x: i32, y: i32) -> bool {
    let court = cam.court();
    let (l, r) = cam.edges(y);
    (court.y..court.y + court.height).contains(&y) && (l..=r).contains(&x)
}

/// Both hoops' pixels in their final colours, and the knockout's: the pixels
/// in each square beside one the board, net or rim drew, that no hoop pixel
/// covers (the stanchion's never show a line), by what they return to.
struct Hoops {
    drawn: Grid<Option<Rgb888>>,
    knock_floor: BTreeSet<(i32, i32)>,
    knock_off: BTreeSet<(i32, i32)>,
}

fn hoops(cam: &Camera) -> Hoops {
    let ends = [hoop(cam, End::Left), hoop(cam, End::Right)];
    let mut drawn = Grid::new(None);
    for end in &ends {
        for layer in [&end.stanchion, &end.front] {
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    if let Some(Some(c)) = layer.get(x, y) {
                        drawn.set(x, y, Some(c));
                    }
                }
            }
        }
    }
    let (mut floor, mut off) = (BTreeSet::new(), BTreeSet::new());
    for end in &ends {
        let source =
            |x: i32, y: i32| in_square(end.square, x, y) && end.front.get(x, y).flatten().is_some();
        let (sx, sy) = end.square;
        for y in sy..=sy + 2 * KNOCK_HALF {
            for x in sx..=sx + 2 * KNOCK_HALF {
                let beside = (-1..=1)
                    .any(|dy| (-1..=1).any(|dx| (dx, dy) != (0, 0) && source(x + dx, y + dy)));
                if !beside || source(x, y) || !in_ring(x, y) || drawn.get(x, y).flatten().is_some()
                {
                    continue;
                }
                if on_floor(cam, x, y) {
                    floor.insert((x, y));
                } else {
                    off.insert((x, y));
                }
            }
        }
    }
    Hoops {
        drawn,
        knock_floor: floor,
        knock_off: off,
    }
}

// -- The module ------------------------------------------------------------------

const HEADER: &str = "\
// GENERATED by crates/scoreboard-render/tests/court_compile.rs — do not edit.
//
// After changing the court's drawing, its camera, its rectangle or a rule
// book, regenerate this file and commit the two together:
//
//     SCOREBOARD_BLESS_COURT=1 cargo test -p scoreboard-render --test court_compile
//
";

fn runs_text(runs: &[Run], indent: &str) -> String {
    let mut out = String::new();
    for chunk in runs.chunks(4) {
        let items: Vec<String> = chunk
            .iter()
            .map(|run| format!("Run::new({}, {}, {}),", run.x, run.y, run.len))
            .collect();
        out += &format!("{indent}{}\n", items.join(" "));
    }
    out
}

fn slice_text(runs: &[Run], indent: &str) -> String {
    if runs.is_empty() {
        return "&[]".into();
    }
    format!(
        "&[\n{}{}]",
        runs_text(runs, &format!("{indent}    ")),
        indent
    )
}

fn pixels(runs: &[Run]) -> usize {
    runs.iter().map(|run| run.len as usize).sum()
}

/// The whole generated module, from the code as it is now.
fn generate() -> String {
    let cam = camera();
    let court = cam.court();
    let mut out = String::from(HEADER);
    out += &format!(
        "//! The arena court's static pixels (NCAAB.md E1), compiled for the\n\
         //! geometry table's court at ({}, {}), {} × {}: each row's floor, the\n\
         //! hoops and their knockout, which every rule book shares, then each\n\
         //! rule book's lines, keys and caption floor. [`crate::court`] says what\n\
         //! each table holds and draws them.\n\
         \n\
         use crate::court::{{\n    \
         BOARD, BOARD_FACE, CourtRaster, IRON, Knockout, NET, PAD, PAD_BASE, Paint, Run,\n\
         }};\n\
         use scoreboard_model::basketball::CourtLines;\n\n",
        court.x, court.y, court.width, court.height
    );

    let floor_runs: Vec<Run> = (court.y..court.y + court.height)
        .map(|row| {
            let (l, r) = cam.edges(row);
            Run::new(l as u8, row as u8, (r - l + 1) as u8)
        })
        .collect();
    out += &format!(
        "/// Each court row's floor, baseline to baseline.\n\
         #[rustfmt::skip]\n\
         pub static FLOOR: [Run; {}] = [\n{}];\n\n",
        floor_runs.len(),
        runs_text(&floor_runs, "    ")
    );

    let Hoops {
        drawn,
        knock_floor,
        knock_off,
    } = hoops(&cam);
    out += &format!(
        "/// Both hoops in their final colours, back to front: the stanchion's\n\
         /// base, post and arm, the board's face and frame, the net, the rim.\n\
         #[rustfmt::skip]\n\
         pub static HOOPS: [Paint; {}] = [\n",
        HOOP_PAINTS.len()
    );
    for (name, color) in HOOP_PAINTS {
        let runs = drawn.runs(|c| c == Some(color));
        out += &format!(
            "    Paint {{\n        color: {name},\n        runs: {},\n    }},\n",
            slice_text(&runs, "        ")
        );
    }
    out += "];\n\n";
    let (knock_floor, knock_off) = (runs_from(&knock_floor), runs_from(&knock_off));
    out += &format!(
        "/// The pixels beside the board, net and rim that a line pixel may be on\n\
         /// ({} on the floor, {} off it).\n\
         #[rustfmt::skip]\n\
         pub static KNOCKOUT: Knockout = Knockout {{\n    floor: {},\n    off: {},\n}};\n",
        pixels(&knock_floor),
        pixels(&knock_off),
        slice_text(&knock_floor, "    "),
        slice_text(&knock_off, "    ")
    );

    for (name, lines) in RULE_BOOKS {
        let grid = floor(&cam, lines);
        let white = grid.runs(|ink| ink == Ink::Line);
        let keys = [End::Left, End::Right].map(|end| grid.runs(|ink| ink == Ink::Key(end)));
        let caption = caption(&cam, lines, BASKETBALL_LIVE.caption_box);
        out += &format!(
            "\n/// `CourtLines::{name}`: {} line pixels, {} and {} in the keys.\n\
             #[rustfmt::skip]\n\
             pub static {name}: CourtRaster = CourtRaster {{\n    \
             lines: CourtLines::{name},\n    \
             white: {},\n    \
             keys: [\n        {},\n        {},\n    ],\n    \
             caption: {},\n}};\n",
            pixels(&white),
            pixels(&keys[0]),
            pixels(&keys[1]),
            slice_text(&white, "    "),
            slice_text(&keys[0], "        "),
            slice_text(&keys[1], "        "),
            slice_text(&caption, "    "),
        );
    }
    let names: Vec<String> = RULE_BOOKS
        .iter()
        .map(|(name, _)| format!("&{name}"))
        .collect();
    out += &format!(
        "\n/// Every compiled rule book, the NBA's first: the court an unknown rule\n\
         /// book draws.\n\
         pub static COURTS: [&CourtRaster; {}] = [{}];\n",
        RULE_BOOKS.len(),
        names.join(", ")
    );
    out
}

#[test]
fn the_generated_module_is_what_the_court_code_draws() {
    let path = generated_path();
    let fresh = generate();
    if std::env::var_os(BLESS_VAR).is_some() {
        std::fs::write(&path, &fresh).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n");
    if committed == fresh {
        return;
    }
    let (line, ours, theirs) = fresh
        .lines()
        .zip(committed.lines())
        .enumerate()
        .find(|(_, (ours, theirs))| ours != theirs)
        .map(|(index, (ours, theirs))| (index + 1, ours, theirs))
        .unwrap_or((
            fresh.lines().count().min(committed.lines().count()) + 1,
            "",
            "",
        ));
    panic!(
        "{} is stale: the court's code draws something else.\n\
         first difference, line {line}:\n  committed: {theirs}\n  drawn:     {ours}\n\
         If the change is intended, regenerate and commit both: {BLESS_VAR}=1 \
         cargo test -p scoreboard-render --test court_compile",
        path.display()
    );
}

// -- Round trips: the device's drawing of the tables is the code's -----------------

struct Frame(Vec<u8>);

impl Frame {
    fn new() -> Self {
        Frame(vec![0; (WIDTH * HEIGHT) as usize * BYTES_PER_PIXEL])
    }

    fn canvas(&mut self) -> Canvas<'_> {
        Canvas::new(&mut self.0, WIDTH, HEIGHT)
    }

    fn pixel(&self, x: i32, y: i32) -> Rgb888 {
        let i = (y * WIDTH + x) as usize * BYTES_PER_PIXEL;
        Rgb888::new(self.0[i], self.0[i + 1], self.0[i + 2])
    }
}

const AWAY: Rgb888 = Rgb888::new(200, 40, 40);
const HOME: Rgb888 = Rgb888::new(40, 90, 220);

fn paint(ink: Ink) -> Rgb888 {
    match ink {
        Ink::Off => BLACK,
        Ink::Wood => WOOD,
        Ink::Line => LINES,
        Ink::Key(End::Left) => scale(AWAY, 2, 3),
        Ink::Key(End::Right) => scale(HOME, 2, 3),
    }
}

/// Each rule book's floor, as the device fills it from the tables, is the
/// floor the code draws, pixel for pixel.
#[test]
fn the_tables_draw_the_court_the_code_draws() {
    let cam = camera();
    for (name, lines) in RULE_BOOKS {
        let mut frame = Frame::new();
        court::raster(lines).draw_floor(&mut frame.canvas(), AWAY, HOME);
        let grid = floor(&cam, lines);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                assert_eq!(
                    frame.pixel(x, y),
                    paint(grid.get(x, y).unwrap()),
                    "{name} at ({x}, {y})"
                );
            }
        }
    }
}

/// The hoops and their knockout, as the device draws them from the tables,
/// are the prototype's procedure on each bare court: the stanchion drawn, the
/// canvas remembered, the board, net and rim drawn over it, then every pixel
/// in the square that was a line before and is beside one the hoop changed
/// put back to the floor.
#[test]
fn the_hoops_knock_out_what_the_code_knocks_out() {
    let cam = camera();
    for (name, lines) in RULE_BOOKS {
        let mut device = Frame::new();
        let raster = court::raster(lines);
        raster.draw_floor(&mut device.canvas(), AWAY, HOME);
        court::draw_hoops(&mut device.canvas());

        let mut expected = Frame::new();
        raster.draw_floor(&mut expected.canvas(), AWAY, HOME);
        for end in [End::Left, End::Right] {
            let hoop = hoop(&cam, end);
            let mut canvas = expected.canvas();
            let lay = |canvas: &mut Canvas<'_>, layer: &Grid<Option<Rgb888>>| {
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        if let Some(Some(c)) = layer.get(x, y) {
                            canvas.pixel(x, y, c);
                        }
                    }
                }
            };
            lay(&mut canvas, &hoop.stanchion);
            let before = expected.0.clone();
            let mut canvas = expected.canvas();
            lay(&mut canvas, &hoop.front);
            let at = |pixels: &[u8], x: i32, y: i32| {
                let i = (y * WIDTH + x) as usize * BYTES_PER_PIXEL;
                Rgb888::new(pixels[i], pixels[i + 1], pixels[i + 2])
            };
            let changed = |x: i32, y: i32| {
                in_square(hoop.square, x, y) && at(&expected.0, x, y) != at(&before, x, y)
            };
            let (sx, sy) = hoop.square;
            let mut gap = Vec::new();
            for y in sy..=sy + 2 * KNOCK_HALF {
                for x in sx..=sx + 2 * KNOCK_HALF {
                    if changed(x, y) || at(&before, x, y) != LINES {
                        continue;
                    }
                    if (-1..=1)
                        .any(|dy| (-1..=1).any(|dx| (dx, dy) != (0, 0) && changed(x + dx, y + dy)))
                    {
                        gap.push((x, y));
                    }
                }
            }
            let mut canvas = expected.canvas();
            for (x, y) in gap {
                if in_ring(x, y) {
                    canvas.pixel(x, y, if on_floor(&cam, x, y) { WOOD } else { BLACK });
                }
            }
        }
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                assert_eq!(
                    device.pixel(x, y),
                    expected.pixel(x, y),
                    "{name} at ({x}, {y})"
                );
            }
        }
    }
}

/// The rule books the tables hold, and the fallback.
#[test]
fn each_rule_book_draws_its_own_court_and_any_other_the_nbas() {
    for (_, lines) in RULE_BOOKS {
        assert_eq!(court::raster(lines).lines, lines);
    }
    let unknown = CourtLines {
        lane_half: CourtLines::NCAA.lane_half,
        ..CourtLines::NBA
    };
    assert_eq!(court::raster(unknown).lines, CourtLines::NBA);
}

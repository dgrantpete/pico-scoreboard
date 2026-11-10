//! The arena court (NCAAB.md §3.11), held to its own rules whatever the
//! tables hold: nothing outside rows 25–62 and columns 1–126, mirrored ends,
//! an unbroken boundary that only the hoops stand in front of, a caption
//! floor that leaves the arcs whole, a dark pixel between every hoop and
//! every line, makes that land on the floor and share pixels with the lines
//! they were made on, D5's alpha climb, and each rule book's own lines.
//! `court_compile.rs` holds the drawing itself; B7's oracle matched it to the
//! reviewed prototype byte for byte.

use scoreboard_model::Rgb888;
use scoreboard_model::basketball::CourtLines;
use scoreboard_model::snapshot::Side;
use scoreboard_render::blit::Canvas;
use scoreboard_render::court::{
    self, BOARD, BOARD_FACE, Camera, End, IRON, LENGTH, LINES, NET, ORIGIN, PAD, PAD_BASE, RIM,
    WOOD,
};
use scoreboard_render::geometry::{BASKETBALL_LIVE, HEIGHT, WIDTH};
use scoreboard_render::{BLACK, WHITE, scale};

/// Utah purple and Oklahoma City blue, brightened as the model stores a team
/// colour.
const AWAY: u32 = 0x4e008e;
const HOME: u32 = 0x007ac1;
/// D5's mark opacity.
const ALPHA: u32 = 96;
const RULE_BOOKS: [CourtLines; 2] = [CourtLines::NBA, CourtLines::NCAA];

/// `(home, along, across)`: a dunk, a long two, a corner three, the baseline
/// corner, a half-court heave, five stacked at the other rim, the other
/// corner, and points past the far baseline and both sidelines.
const MARKS: [(bool, f32, f32); 14] = [
    (false, 5.25, 0.0),
    (false, 27.0, 0.0),
    (false, 10.0, 22.5),
    (false, 0.0, -25.0),
    (false, 51.25, 3.0),
    (true, 5.25, 0.0),
    (true, 5.25, 0.0),
    (true, 5.25, 0.0),
    (true, 5.25, 0.0),
    (true, 5.25, 0.0),
    (true, 14.0, -22.0),
    (true, 94.0, 10.0),
    (true, -3.0, 30.0),
    (false, 40.0, -31.0),
];
const BLINK: (bool, f32, f32) = (true, 20.0, 5.0);

struct Frame(Vec<u8>);

impl Frame {
    fn filled(color: Rgb888) -> Self {
        let mut pixels = vec![0; (WIDTH * HEIGHT * 3) as usize];
        for pixel in pixels.as_chunks_mut::<3>().0 {
            *pixel = [color.red(), color.green(), color.blue()];
        }
        Frame(pixels)
    }

    fn new() -> Self {
        Self::filled(BLACK)
    }

    fn canvas(&mut self) -> Canvas<'_> {
        Canvas::new(&mut self.0, WIDTH, HEIGHT)
    }

    fn pixel(&self, x: i32, y: i32) -> Rgb888 {
        let index = ((y * WIDTH + x) * 3) as usize;
        Rgb888::new(self.0[index], self.0[index + 1], self.0[index + 2])
    }
}

fn teams() -> (Rgb888, Rgb888) {
    (Rgb888(AWAY).brightened(), Rgb888(HOME).brightened())
}

fn end(home: bool) -> End {
    if home { End::Right } else { End::Left }
}

fn camera() -> Camera {
    Camera::new(&BASKETBALL_LIVE.court)
}

fn bare(lines: CourtLines, away: Rgb888, home: Rgb888) -> Frame {
    let mut frame = Frame::new();
    court::raster(lines).draw_floor(&mut frame.canvas(), away, home);
    frame
}

/// §3.11's whole draw over `background`: the floor, `MARKS` and the `BLINK`,
/// the caption's floor, the hoops — or all but the hoops.
fn full(lines: CourtLines, background: Rgb888, hoops: bool) -> Frame {
    let (away, home) = teams();
    let cam = camera();
    let raster = court::raster(lines);
    let mut frame = Frame::filled(background);
    let mut canvas = frame.canvas();
    raster.draw_floor(&mut canvas, away, home);
    for (home_side, along, across) in MARKS {
        let color = if home_side { home } else { away };
        cam.plot_over(&mut canvas, end(home_side), along, across, color, ALPHA);
    }
    cam.plot(&mut canvas, end(BLINK.0), BLINK.1, BLINK.2, WHITE);
    raster.clear_caption(&mut canvas);
    if hoops {
        court::draw_hoops(&mut canvas);
    }
    frame
}

/// The boundary's pixels: the far and near sidelines whole, each baseline's
/// pixel on every row between.
fn boundary() -> Vec<(i32, i32)> {
    let cam = camera();
    let court = cam.court();
    let (top, bottom) = (court.y, court.y + court.height - 1);
    let mut pixels = Vec::new();
    for y in top..=bottom {
        let (l, r) = cam.edges(y);
        if y == top || y == bottom {
            pixels.extend((l..=r).map(|x| (x, y)));
        } else {
            pixels.extend([(l, y), (r, y)]);
        }
    }
    pixels
}

/// The board, net and rim's colours: what the knockout keeps lines off.
const FRONT: [Rgb888; 4] = [BOARD_FACE, BOARD, NET, IRON];

fn beside(frame: &Frame, x: i32, y: i32, colors: &[Rgb888]) -> bool {
    (-1..=1).any(|dy| {
        (-1..=1).any(|dx| {
            let (nx, ny) = (x + dx, y + dy);
            (dx, dy) != (0, 0)
                && (0..WIDTH).contains(&nx)
                && (0..HEIGHT).contains(&ny)
                && colors.contains(&frame.pixel(nx, ny))
        })
    })
}

#[test]
fn nothing_is_drawn_outside_rows_25_to_62_and_columns_1_to_126() {
    let sentinel = Rgb888::new(8, 248, 8);
    for lines in RULE_BOOKS {
        let frame = full(lines, sentinel, true);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let inside = (1..=WIDTH - 2).contains(&x) && (25..=HEIGHT - 2).contains(&y);
                if !inside {
                    assert_eq!(
                        frame.pixel(x, y),
                        sentinel,
                        "{lines:?} ({x}, {y}) was touched"
                    );
                }
            }
        }
    }
}

/// The camera sits on the half-court line, so the ends mirror about column
/// 63.5 — except on columns 63 and 64: the one-pixel half-court line rounds
/// onto 64 and its mirror is wood.
#[test]
fn the_ends_mirror() {
    let color = Rgb888::new(200, 40, 40);
    let cam = camera();
    assert_eq!(cam.px(LENGTH / 2.0, 0.0, 0.0).0, 64, "the half-court line");
    for lines in RULE_BOOKS {
        let mut frame = bare(lines, color, color);
        court::draw_hoops(&mut frame.canvas());
        for y in 0..HEIGHT {
            for x in (0..WIDTH).filter(|x| !(63..=64).contains(x)) {
                assert_eq!(
                    frame.pixel(x, y),
                    frame.pixel(WIDTH - 1 - x, y),
                    "{lines:?} ({x}, {y})"
                );
            }
        }
    }
}

/// §3.11's numbers, as the camera fits them to B5's rectangle: the near
/// sideline 118 px, the far 97 (98 between its rounded ends), the far half
/// 16 rows and the near half 20.
#[test]
fn the_camera_fits_the_b5_rectangle() {
    let cam = camera();
    let court = cam.court();
    let (top, bottom) = (court.y, court.y + court.height - 1);
    assert_eq!((top, bottom), (26, 62));
    let (l, r) = cam.edges(bottom);
    assert_eq!(r - l + 1, 118, "near sideline");
    let (l, r) = cam.edges(top);
    assert_eq!(r - l + 1, 98, "far sideline");
    let centre = cam.px(LENGTH / 2.0, 0.0, 0.0).1;
    assert_eq!((centre - top, bottom - centre), (16, 20));
    // The floor's inverse lands back on the pixel it came from.
    for row in top..=bottom {
        for column in [20, 64, 100] {
            let (x, y) = (cam.floor_x(column, row), cam.floor_y(row));
            assert_eq!(cam.px(x, y, 0.0), (column, row));
        }
    }
}

/// The sidelines and baselines draw last and nothing on the floor crosses
/// them — no lane colour, no mark, no caption. Only the hoops stand in front
/// of the far end of each baseline: there a boundary pixel is the hoop's, or
/// knocked back to wood beside it.
#[test]
fn the_boundary_is_unbroken_and_only_the_hoops_stand_in_front_of_it() {
    for lines in RULE_BOOKS {
        let floor = full(lines, BLACK, false);
        for &(x, y) in &boundary() {
            assert_eq!(floor.pixel(x, y), LINES, "{lines:?} ({x}, {y})");
        }
        let frame = full(lines, BLACK, true);
        let hoop = [PAD_BASE, PAD, BOARD_FACE, BOARD, NET, IRON];
        let mut behind = 0;
        for &(x, y) in &boundary() {
            let pixel = frame.pixel(x, y);
            if pixel == LINES {
                continue;
            }
            behind += 1;
            assert!(
                hoop.contains(&pixel) || (pixel == WOOD && beside(&frame, x, y, &FRONT)),
                "{lines:?} ({x}, {y}) is {pixel:?}"
            );
            assert!(
                y < camera().px(0.0, 0.0, 0.0).1,
                "only the far half: ({x}, {y})"
            );
        }
        assert!(behind > 0, "the boards stand in front of the baselines");
    }
}

/// The caption's floor erases marks and the half-court line under the text
/// but stops at the three-point arcs: of the lines, it covers only the
/// half-court line's column.
#[test]
fn the_caption_floor_leaves_every_line_but_half_court_whole() {
    let (away, home) = teams();
    let half_court = camera().px(LENGTH / 2.0, 0.0, 0.0).0;
    let slot = BASKETBALL_LIVE.caption_box;
    for lines in RULE_BOOKS {
        let floor = bare(lines, away, home);
        let mut cleared = 0;
        for run in court::raster(lines).caption {
            let (first, last) = run.ends();
            let y = run.y as i32;
            assert!((slot.y..slot.y + slot.height).contains(&y));
            assert!(
                first >= slot.x && last < slot.x + slot.width,
                "{lines:?} row {y}"
            );
            for x in first..=last {
                let under = floor.pixel(x, y);
                assert!(
                    under == WOOD || (under == LINES && x == half_court),
                    "{lines:?}: the caption covers ({x}, {y}), {under:?}"
                );
                cleared += usize::from(under == LINES);
            }
        }
        assert_eq!(
            cleared, slot.height as usize,
            "{lines:?}: the half-court line under the box"
        );
    }
}

/// The knockout: after the whole draw no line pixel touches the board, the
/// net or the rim, even where a mark sits beside them.
#[test]
fn the_knockout_leaves_a_dark_pixel_between_hoop_and_line() {
    for lines in RULE_BOOKS {
        let frame = full(lines, BLACK, true);
        let mut front = 0;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if FRONT.contains(&frame.pixel(x, y)) {
                    front += 1;
                }
                if frame.pixel(x, y) == LINES {
                    assert!(
                        !beside(&frame, x, y, &FRONT),
                        "{lines:?}: the line at ({x}, {y}) touches the hoop"
                    );
                }
            }
        }
        assert!(front > 40, "{lines:?}: both hoops drew");
    }
}

/// Every make lands on the floor, held off the boundary: in the corners, on
/// the baselines, past them and past the sidelines, from both ends.
#[test]
fn a_make_lands_inside_the_court() {
    let cam = camera();
    let color = Rgb888::new(200, 40, 40);
    let base = bare(CourtLines::NBA, color, color);
    let mut along = -10.0;
    while along <= LENGTH + 10.0 {
        let mut across = -30.0;
        while across <= 30.0 {
            for side in [End::Left, End::Right] {
                let mut frame = bare(CourtLines::NBA, color, color);
                cam.plot(&mut frame.canvas(), side, along, across, WHITE);
                let lit: Vec<(i32, i32)> = (0..HEIGHT)
                    .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
                    .filter(|&(x, y)| frame.pixel(x, y) == WHITE)
                    .collect();
                assert_eq!(lit.len(), 4, "{side:?} {along} {across}");
                for (x, y) in lit {
                    assert_ne!(base.pixel(x, y), BLACK, "off the floor at ({x}, {y})");
                    assert!(
                        !boundary().contains(&(x, y)),
                        "on the boundary at ({x}, {y})"
                    );
                }
            }
            across += 2.5;
        }
        along += 2.5;
    }
}

/// One projection draws the lines and places the marks, so a make where a
/// projected line is shares its pixel: the mark's bottom-right pixel is the
/// point, and at the top of the arc, in both corners and at half court that
/// pixel is the line's. (The keys are regions digitised by pixel centre, so
/// their edges sit up to half a pixel inside the true shape — NCAAB.md §8.)
#[test]
fn a_make_and_a_line_in_the_same_place_share_a_pixel() {
    let cam = camera();
    let (away, home) = teams();
    for lines in RULE_BOOKS {
        let floor = bare(lines, away, home);
        let (arc, corner) = (RIM + lines.arc.feet(), lines.corner.feet());
        for side in [End::Left, End::Right] {
            for (along, across) in [
                (arc, 0.0),
                (10.0, corner),
                (10.0, -corner),
                (LENGTH / 2.0, 0.0),
            ] {
                let (x, y) = cam.px(side.feet_from_left(along), across, 0.0);
                let what = format!("{lines:?} {side:?} ({along}, {across})");
                assert_eq!(floor.pixel(x, y), LINES, "{what}: the line");
                let mut frame = bare(lines, away, home);
                cam.plot(&mut frame.canvas(), side, along, across, WHITE);
                assert_eq!(frame.pixel(x, y), WHITE, "{what}: the point");
                assert_eq!(
                    frame.pixel(x - 1, y - 1),
                    WHITE,
                    "{what}: up and left of it"
                );
            }
        }
    }
}

/// The wire's `y = 1` is the rim: through `ORIGIN` a make lands on the rim's
/// floor point at either end, the away side's on the left (D5).
#[test]
fn a_make_at_the_wires_rim_lands_under_the_rim() {
    let cam = camera();
    assert_eq!(1.0 + ORIGIN, RIM);
    for side in [Side::Away, Side::Home] {
        let end = End::from(side);
        let (x, y) = cam.px(end.feet_from_left(RIM), 0.0, 0.0);
        let mut frame = Frame::new();
        cam.plot(&mut frame.canvas(), end, 1.0 + ORIGIN, 0.0, WHITE);
        for (px, py) in [(x, y), (x - 1, y), (x, y - 1), (x - 1, y - 1)] {
            assert_eq!(frame.pixel(px, py), WHITE, "{side:?} ({px}, {py})");
        }
    }
    assert_eq!(End::from(Side::Away), End::Left);
    assert!(cam.px(End::Left.feet_from_left(RIM), 0.0, 0.0).0 < 64);
    assert!(cam.px(End::Right.feet_from_left(RIM), 0.0, 0.0).0 > 64);
}

/// The compositing restated — per channel, at 256ths — so the test does not
/// borrow what it checks.
fn over(under: Rgb888, color: Rgb888, alpha: u32) -> Rgb888 {
    let channel = |u: u8, o: u8| ((u as u32 * (256 - alpha) + o as u32 * alpha) / 256) as u8;
    Rgb888::new(
        channel(under.red(), color.red()),
        channel(under.green(), color.green()),
        channel(under.blue(), color.blue()),
    )
}

/// D5: one make 37 % of the way from the wood to the team colour, two
/// stacked 61 %, five 90 % — here as the exact values of white over wood at
/// 96/256, one mark reading back what the last one wrote.
#[test]
fn marks_climb_toward_the_team_colour_at_d5s_alpha() {
    let cam = camera();
    let (away, home) = teams();
    // A spot on bare wood, past the arc.
    let (end, along, across) = (End::Left, 30.0, 10.0);
    let (x, y) = cam.px(end.feet_from_left(along), across, 0.0);
    let spot = [(x - 1, y - 1), (x, y - 1), (x - 1, y), (x, y)];
    let base = bare(CourtLines::NBA, away, home);
    for (px, py) in spot {
        assert_eq!(base.pixel(px, py), WOOD, "({px}, {py}) is wood");
    }

    let mut climb = Vec::new();
    let mut value = WOOD;
    for _ in 0..5 {
        value = over(value, WHITE, ALPHA);
        climb.push(value);
    }
    assert_eq!(
        [climb[0], climb[1], climb[4]],
        [
            Rgb888::new(114, 108, 101),
            Rgb888::new(166, 163, 158),
            Rgb888::new(233, 231, 230),
        ],
        "the hand-worked values of one, two and five marks"
    );
    for count in 1..=5 {
        let mut frame = bare(CourtLines::NBA, away, home);
        let mut canvas = frame.canvas();
        for _ in 0..count {
            cam.plot_over(&mut canvas, end, along, across, WHITE, ALPHA);
        }
        for (px, py) in spot {
            assert_eq!(
                frame.pixel(px, py),
                climb[count - 1],
                "{count} marks at ({px}, {py})"
            );
        }
    }
}

/// Where the rule books differ the courts do (§3.11): the NBA's lane is
/// 16 ft and the NCAA's 12, so its key spans more rows; the NBA's arc is
/// 23.75 ft about the rim and the NCAA's 22.146, so at the middle row it
/// reaches further from the baseline. Everything else — the floor, the
/// half-court line, the oval, the boundary — is the same court.
#[test]
fn each_rule_book_draws_its_own_lines() {
    let cam = camera();
    let color = Rgb888::new(200, 40, 40);
    let lane = scale(color, 2, 3);
    let mid = cam.px(LENGTH / 2.0, 0.0, 0.0).1;
    let key_rows = |frame: &Frame| -> Vec<i32> {
        (0..HEIGHT)
            .filter(|&y| (0..64).any(|x| frame.pixel(x, y) == lane))
            .collect()
    };
    let arc_column = |frame: &Frame| -> i32 {
        // The middle row's lines left of half court: the baseline, the arc's
        // top, the oval's left edge.
        let lines: Vec<i32> = (0..64).filter(|&x| frame.pixel(x, mid) == LINES).collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        lines[1]
    };
    let (nba, ncaa) = (
        bare(CourtLines::NBA, color, color),
        bare(CourtLines::NCAA, color, color),
    );
    let (nba_rows, ncaa_rows) = (key_rows(&nba), key_rows(&ncaa));
    assert_eq!((nba_rows[0], nba_rows[nba_rows.len() - 1]), (37, 48));
    assert_eq!((ncaa_rows[0], ncaa_rows[ncaa_rows.len() - 1]), (39, 46));
    assert_eq!((arc_column(&nba), arc_column(&ncaa)), (43, 41));
    // Away from the keys and the arcs the courts are one.
    for y in 0..HEIGHT {
        for x in (54..=73).chain([0, 127]) {
            assert_eq!(nba.pixel(x, y), ncaa.pixel(x, y), "({x}, {y})");
        }
    }
}

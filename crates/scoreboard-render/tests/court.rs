//! The procedural court (NBA.md D14): pinned to the mockup harness it ports,
//! and to its own rules — nothing outside its rectangle, mirrored ends, an
//! arc that is one staircase, the rim beside the board at a coarse ratio,
//! D5's alpha climb, and marks that stay inside the lines.

use scoreboard_model::Rgb888;
use scoreboard_model::snapshot::Side;
use scoreboard_render::blit::Canvas;
use scoreboard_render::court::{
    BACKBOARD, CORNER, Court, ESPN_ORIGIN, End, IRON, LENGTH, LINES, RIM, WOOD,
};
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::{WHITE, pack, rgb565, scale};

/// NBA.md §6.5's B5 court: rows 26–62, columns 2–125.
const B5: Court = Court {
    x: 2,
    y: 26,
    width: 124,
    height: 37,
};
/// Utah purple and Oklahoma City blue, brightened as the model stores a team
/// colour: the inputs the reference bitmaps were drawn with.
const AWAY: u32 = 0x4e008e;
const HOME: u32 = 0x007ac1;
/// D5's mark opacity.
const ALPHA: u32 = 96;

/// The reference bitmaps: the B5 court's 124 × 37 rectangle, RGB565 LE,
/// row-major, drawn on 2026-11-08 by the mockup harness's own `Court::draw`,
/// `plot_over` and `plot` (`data/nba-design/mockup-harness/src/court.rs`,
/// included by path into a throwaway binary against this checkout's crates)
/// with `AWAY` and `HOME` above, then `MARKS` at `ALPHA` and the `BLINK`.
const BARE: &[u8] = include_bytes!("court/b5_bare.bin");
const MARKED: &[u8] = include_bytes!("court/b5_marks.bin");

/// `(home, along, across)`, as the reference drew them: a dunk, a long two,
/// a corner three, the baseline corner, a half-court heave, five stacked at
/// the other rim, the other corner, and a point past the far baseline.
const MARKS: [(bool, f32, f32); 12] = [
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
];
const BLINK: (bool, f32, f32) = (true, 20.0, 5.0);

/// The other strips the mockups drew the court into, for the rules that have
/// to hold at every ratio: B (124 × 17), B2 (124 × 29), B4 (62 × 33, true
/// proportion) and the rink's 15-row strip.
const STRIPS: [Court; 4] = [
    Court {
        x: 2,
        y: 46,
        width: 124,
        height: 17,
    },
    Court {
        x: 2,
        y: 34,
        width: 124,
        height: 29,
    },
    Court {
        x: 33,
        y: 14,
        width: 62,
        height: 33,
    },
    Court {
        x: 2,
        y: 47,
        width: 124,
        height: 15,
    },
];

struct Frame(Vec<u8>);

impl Frame {
    fn filled(color: u16) -> Self {
        let mut pixels = vec![0; (WIDTH * HEIGHT * 2) as usize];
        for pixel in pixels.as_chunks_mut::<2>().0 {
            *pixel = color.to_le_bytes();
        }
        Frame(pixels)
    }

    fn new() -> Self {
        Self::filled(0)
    }

    fn canvas(&mut self) -> Canvas<'_> {
        Canvas::new(&mut self.0, WIDTH, HEIGHT)
    }

    fn pixel(&self, x: i32, y: i32) -> u16 {
        let index = ((y * WIDTH + x) * 2) as usize;
        u16::from_le_bytes([self.0[index], self.0[index + 1]])
    }

    /// The court's rectangle, row-major, as the reference bitmaps are laid out.
    fn rect_bytes(&self, court: Court) -> Vec<u8> {
        let mut out = Vec::new();
        for y in court.y..court.y + court.height {
            for x in court.x..court.x + court.width {
                out.extend_from_slice(&self.pixel(x, y).to_le_bytes());
            }
        }
        out
    }
}

fn teams() -> (u16, u16) {
    (
        pack(Rgb888(AWAY).brightened()),
        pack(Rgb888(HOME).brightened()),
    )
}

fn end(home: bool) -> End {
    if home { End::Right } else { End::Left }
}

fn bare(court: Court, away: u16, home: u16) -> Frame {
    let mut frame = Frame::new();
    court.draw(&mut frame.canvas(), away, home);
    frame
}

/// The B5 court with `MARKS` and the `BLINK`, over `background`.
fn marked(background: u16) -> Frame {
    let (away, home) = teams();
    let mut frame = Frame::filled(background);
    let mut canvas = frame.canvas();
    B5.draw(&mut canvas, away, home);
    for (home_side, along, across) in MARKS {
        let color = if home_side { home } else { away };
        B5.plot_over(&mut canvas, end(home_side), along, across, color, ALPHA);
    }
    B5.plot(&mut canvas, end(BLINK.0), BLINK.1, BLINK.2, WHITE);
    frame
}

/// Where a court bitmap first disagrees with its reference.
fn assert_same_bytes(court: Court, got: &[u8], want: &[u8], what: &str) {
    assert_eq!(
        got.len(),
        want.len(),
        "{what}: the reference is not a {court:?}"
    );
    let differing = got
        .as_chunks::<2>()
        .0
        .iter()
        .zip(want.as_chunks::<2>().0)
        .enumerate()
        .filter(|(_, (have, need))| have != need)
        .collect::<Vec<_>>();
    if let Some((index, (have, need))) = differing.first() {
        let index = *index as i32;
        let (x, y) = (court.x + index % court.width, court.y + index / court.width);
        panic!(
            "{what}: {} px differ from the mockup's, first at ({x}, {y}): drew {:#06x}, mockup {:#06x}",
            differing.len(),
            u16::from_le_bytes(**have),
            u16::from_le_bytes(**need),
        );
    }
}

#[test]
fn the_bare_b5_court_is_the_mockups_byte_for_byte() {
    let (away, home) = teams();
    let frame = bare(B5, away, home);
    assert_same_bytes(B5, &frame.rect_bytes(B5), BARE, "bare court");
}

#[test]
fn the_marks_are_the_mockups_byte_for_byte() {
    let frame = marked(0);
    assert_same_bytes(B5, &frame.rect_bytes(B5), MARKED, "marked court");
}

#[test]
fn nothing_is_drawn_outside_the_court() {
    let sentinel = rgb565(8, 248, 8);
    let frame = marked(sentinel);
    let (away, home) = teams();
    let floor = [WOOD, LINES, IRON, scale(away, 2, 3), scale(home, 2, 3)];
    let bare = bare(B5, away, home);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let inside =
                (B5.x..B5.x + B5.width).contains(&x) && (B5.y..B5.y + B5.height).contains(&y);
            if inside {
                assert!(floor.contains(&bare.pixel(x, y)), "({x}, {y}) is not floor");
            } else {
                assert_eq!(frame.pixel(x, y), sentinel, "({x}, {y}) was touched");
            }
        }
    }
}

#[test]
fn the_ends_mirror() {
    // The same colour at both ends, so the mirror is exact — except on the
    // two middle columns: the court is an even width, so the one-pixel
    // half-court line falls on one of them and its mirror is wood.
    let color = rgb565(200, 40, 40);
    let (x0, x1) = (B5.x, B5.x + B5.width - 1);
    let centre = B5.fx(LENGTH / 2.0);
    let frame = bare(B5, color, color);
    for y in B5.y..B5.y + B5.height {
        for x in (x0..=x1).filter(|&x| x != centre && x0 + x1 - x != centre) {
            assert_eq!(frame.pixel(x, y), frame.pixel(x0 + x1 - x, y), "({x}, {y})");
        }
    }
    // §6.5's numbers: the half-court line, the middle row, the two rims.
    assert_eq!(B5.fx(LENGTH / 2.0), 64);
    assert_eq!(B5.mid(), 44);
    assert_eq!(B5.at(End::Left, RIM), 9);
    assert_eq!(B5.at(End::Right, RIM), 118);
}

/// The arc's run in one row at one end: the line-coloured pixels between the
/// baseline and the half-court line, the backboard's three left out. Asserts
/// the run is unbroken.
fn arc_run(frame: &Frame, court: Court, end: End, y: i32) -> (i32, i32) {
    let centre = court.fx(LENGTH / 2.0);
    let (lo, hi) = match end {
        End::Left => (court.x + 1, centre),
        End::Right => (centre + 1, court.x + court.width - 1),
    };
    let board = court.at(end, BACKBOARD);
    let is_board = |x: i32| x == board && (court.mid() - 1..=court.mid() + 1).contains(&y);
    let xs: Vec<i32> = (lo..hi)
        .filter(|&x| !is_board(x) && frame.pixel(x, y) == LINES)
        .collect();
    let (first, last) = (xs[0], xs[xs.len() - 1]);
    assert_eq!(
        xs.len() as i32,
        last - first + 1,
        "{court:?} row {y}: a broken run"
    );
    (first, last)
}

/// Each row's run joins the previous row's and never steps past its own x:
/// from the corner rows in, the run begins where the last one ended (or one
/// past it) and ends no sooner.
#[test]
fn the_arc_is_one_staircase_without_nubs() {
    let (away, home) = teams();
    for court in STRIPS.into_iter().chain([B5]) {
        let frame = bare(court, away, home);
        let corner_rows = -court.dy(CORNER);
        for end in [End::Left, End::Right] {
            let toward_centre = match end {
                End::Left => 1,
                End::Right => -1,
            };
            for half in [-1, 1] {
                let mut prev: Option<(i32, i32)> = None;
                for row in (0..=corner_rows).rev() {
                    let y = court.mid() + half * row;
                    let (first, last) = arc_run(&frame, court, end, y);
                    // The run's near end (toward the baseline) and far end.
                    let (near, far) = match end {
                        End::Left => (first, last),
                        End::Right => (last, first),
                    };
                    match prev {
                        None => assert_eq!(
                            near,
                            court.at(end, 0.0) + toward_centre,
                            "{court:?} {end:?}: the corner segment starts at the baseline"
                        ),
                        Some((_, prev_far)) => assert!(
                            near == prev_far || near == prev_far + toward_centre,
                            "{court:?} {end:?} row {y}: run {first}..={last} does not join the row before, which ended at {prev_far}"
                        ),
                    }
                    prev = Some((near, far));
                }
            }
        }
    }
}

#[test]
fn the_rim_takes_the_next_column_when_it_rounds_onto_the_board() {
    let (away, home) = teams();
    // B4's 0.65 px/ft: the premise is that the board and the rim share a column.
    let coarse = STRIPS[2];
    assert_eq!(coarse.at(End::Left, BACKBOARD), coarse.at(End::Left, RIM));
    let frame = bare(coarse, away, home);
    for (end, toward_centre) in [(End::Left, 1), (End::Right, -1)] {
        let (board, mid) = (coarse.at(end, BACKBOARD), coarse.mid());
        for y in mid - 1..=mid + 1 {
            assert_eq!(
                frame.pixel(board, y),
                LINES,
                "{end:?}: the board stays whole"
            );
        }
        assert_eq!(
            frame.pixel(board + toward_centre, mid),
            IRON,
            "{end:?}: the rim steps in"
        );
    }
    // B5 has the room: the rim sits on its own column.
    assert_ne!(B5.at(End::Left, BACKBOARD), B5.at(End::Left, RIM));
    let frame = bare(B5, away, home);
    for end in [End::Left, End::Right] {
        assert_eq!(frame.pixel(end_rim(end).0, end_rim(end).1), IRON, "{end:?}");
        assert_eq!(
            frame.pixel(B5.at(end, BACKBOARD), B5.mid()),
            LINES,
            "{end:?}"
        );
    }
}

fn end_rim(end: End) -> (i32, i32) {
    (B5.at(end, RIM), B5.mid())
}

/// The compositing restated — widen to 8 bits, blend, pack — so the test
/// does not borrow what it checks.
fn over(under: u16, color: u16, alpha: u32) -> u16 {
    let widen = |c: u16| {
        [
            ((c >> 11) & 31) as u32 * 255 / 31,
            ((c >> 5) & 63) as u32 * 255 / 63,
            (c & 31) as u32 * 255 / 31,
        ]
    };
    let (u, o) = (widen(under), widen(color));
    let channel = |i: usize| ((u[i] * (256 - alpha) + o[i] * alpha) / 256) as u8;
    rgb565(channel(0), channel(1), channel(2))
}

/// D5: one make 37 % of the way from the wood to the team colour, two
/// stacked 61 %, five 90 % — here as the exact 565 values of white over wood
/// at 96/256, one mark reading back what the last one wrote.
#[test]
fn marks_climb_toward_the_team_colour_at_d5s_alpha() {
    let (away, home) = teams();
    // A spot on bare wood, past the arc.
    let (end, along, across) = (End::Left, 30.0, 10.0);
    let (x, y) = (B5.at(end, along), B5.mid() + B5.dy(across));
    let spot = [(x - 1, y - 1), (x, y - 1), (x - 1, y), (x, y)];
    let base = bare(B5, away, home);
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
        [0x6B6C, 0xA513, 0xEF3D],
        "the hand-worked values of one, two and five marks"
    );
    for count in 1..=5 {
        let mut frame = bare(B5, away, home);
        let mut canvas = frame.canvas();
        for _ in 0..count {
            B5.plot_over(&mut canvas, end, along, across, WHITE, ALPHA);
        }
        for (px, py) in spot {
            assert_eq!(
                frame.pixel(px, py),
                climb[count - 1],
                "{count} marks at ({px}, {py})"
            );
        }
    }
    // The prose, within 565's stair: the red channel's climb from wood to white.
    let red = |c: u16| ((c >> 11) & 31) as f32 * 255.0 / 31.0;
    for (value, nominal) in [(climb[0], 37.5), (climb[1], 60.9), (climb[4], 90.5)] {
        let climbed = (red(value) - red(WOOD)) / (255.0 - red(WOOD)) * 100.0;
        assert!(
            (climbed - nominal).abs() < 4.0,
            "{climbed:.1} % against {nominal} %"
        );
    }
}

#[test]
fn marks_stay_inside_the_lines() {
    let (away, home) = teams();
    let (x0, x1) = (B5.x, B5.x + B5.width - 1);
    let (y0, y1) = (B5.y, B5.y + B5.height - 1);
    let base = bare(B5, away, home);
    let mut frame = bare(B5, away, home);
    let mut canvas = frame.canvas();
    // On the baseline, in both its corners, a heave to the far baseline, and
    // the heave the model clamps to half court (§3.4), from both ends.
    for (along, across) in [
        (0.0, 0.0),
        (0.0, 25.0),
        (0.0, -25.0),
        (LENGTH, 0.0),
        (47.0 + ESPN_ORIGIN, 3.0),
    ] {
        B5.plot(&mut canvas, End::Left, along, across, WHITE);
        B5.plot(&mut canvas, End::Right, along, across, WHITE);
    }
    // The lines are untouched.
    for x in x0..=x1 {
        for y in [y0, y1] {
            assert_eq!(frame.pixel(x, y), base.pixel(x, y), "({x}, {y})");
        }
    }
    for y in y0..=y1 {
        for x in [x0, x1] {
            assert_eq!(frame.pixel(x, y), base.pixel(x, y), "({x}, {y})");
        }
    }
    // The marks landed, pressed against them: the baseline make, the two
    // corners and the far heave at each end, the half-court heave on the
    // floor just past the line.
    for (x, y) in [
        (x0 + 1, B5.mid()),
        (x0 + 1, y0 + 1),
        (x0 + 1, y1 - 1),
        (x1 - 1, B5.mid()),
        (x1 - 1, y0 + 1),
        (x1 - 1, y1 - 1),
        (69, 42),
        (58, 42),
    ] {
        assert_eq!(frame.pixel(x, y), WHITE, "({x}, {y})");
    }
}

/// ESPN's `y = 1` is the rim (§3.4): through `ESPN_ORIGIN` it lands on the
/// rim pixel at either end, the mark's bottom-right pixel being the point.
#[test]
fn a_make_at_espns_rim_sits_on_the_rim() {
    let (away, home) = teams();
    let along = 1.0 + ESPN_ORIGIN;
    assert_eq!(B5.at(End::Left, along), B5.at(End::Left, RIM));
    for side in [Side::Away, Side::Home] {
        let end = End::from(side);
        let mut frame = bare(B5, away, home);
        B5.plot(&mut frame.canvas(), end, along, 0.0, WHITE);
        let (x, y) = end_rim(end);
        for (px, py) in [(x, y), (x - 1, y), (x, y - 1), (x - 1, y - 1)] {
            assert_eq!(frame.pixel(px, py), WHITE, "{side:?} ({px}, {py})");
        }
        assert_ne!(frame.pixel(x + 1, y), WHITE);
        assert_ne!(frame.pixel(x - 2, y), WHITE);
    }
    // Away attacks the left basket, home the right (D5).
    assert_eq!(End::from(Side::Away), End::Left);
    assert!(B5.at(End::Left, RIM) < B5.fx(LENGTH / 2.0));
    assert!(B5.at(End::Right, RIM) > B5.fx(LENGTH / 2.0));
}

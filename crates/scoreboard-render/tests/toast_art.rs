//! The procedural padlock and spinner ring (BACKLOG 116), pinned to exact
//! pictures where `toast.rs` centres them: the padlock closed and open in any
//! colour, the ring dot by dot in angular order, and a comet's dark dots left
//! undrawn.

mod picture;

use picture::{assert_picture, assert_untouched_outside, frame, size};
use scoreboard_model::Rgb888;
use scoreboard_render::blit::Canvas;
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::padlock::{self, COLOR};
use scoreboard_render::{scale, spinner};

/// Where `toast.rs` centres each icon on the panel.
const LOCK_AT: (i32, i32) = (WIDTH / 2 - padlock::WIDTH / 2, 19);
const SPINNER_AT: (i32, i32) = (
    WIDTH / 2 - spinner::WIDTH / 2,
    HEIGHT / 2 - spinner::HEIGHT / 2,
);

/// The padlock, closed: `w` is its colour, the keyhole and the space under
/// the shackle left alone.
const CLOSED: [&str; 22] = [
    "..............",
    "..............",
    "..............",
    "..wwwwwwwwww..",
    "..wwwwwwwwww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "wwwwwwwwwwwwww",
    "wwwwwwwwwwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwwwwwwwwwww",
    "wwwwwwwwwwwwww",
];

/// Open: the shackle three rows higher, its left leg the same length and
/// clear of the body.
const OPEN: [&str; 22] = [
    "..wwwwwwwwww..",
    "..wwwwwwwwww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..ww......ww..",
    "..........ww..",
    "..........ww..",
    "..........ww..",
    "wwwwwwwwwwwwww",
    "wwwwwwwwwwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwww..wwwwww",
    "wwwwwwwwwwwwww",
    "wwwwwwwwwwwwww",
];

/// White, then primaries at toast brightnesses, as `toast.rs` scales them.
fn lock_colors() -> impl Iterator<Item = Rgb888> {
    let primaries = [0xffffff, 0x4e008e, 0xffb000, 0x10c060];
    let scaled = primaries.into_iter().flat_map(|primary| {
        [255, 200, 128, 33].map(move |brightness| scale(Rgb888(primary), brightness, 255))
    });
    std::iter::once(Rgb888::new(255, 255, 255)).chain(scaled)
}

#[test]
fn the_padlock_is_its_picture_in_any_colour() {
    assert_eq!(COLOR, Rgb888::new(255, 255, 255));
    for (picture, open) in [(CLOSED, false), (OPEN, true)] {
        assert_eq!(size(&picture), (padlock::WIDTH, padlock::HEIGHT));
        for color in lock_colors() {
            let mut pixels = frame();
            let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
            padlock::draw(&mut canvas, LOCK_AT.0, LOCK_AT.1, open, color);
            let what = format!("padlock open={open} in {color:?}");
            assert_picture(&canvas, LOCK_AT, &picture, &[('w', Some(color))], &what);
            let rect = (LOCK_AT.0, LOCK_AT.1, padlock::WIDTH, padlock::HEIGHT);
            assert_untouched_outside(&canvas, rect, &what);
        }
    }
}

/// The ring: dot `k` is the `k`th letter, `a` at three o'clock and clockwise
/// from there.
const RING: [&str; 25] = [
    "............jjj...........",
    "......iii...jjj..kkk......",
    "......iii...jjj..kkk......",
    "......iii........kkk......",
    "..........................",
    "..hhh................lll..",
    "..hhh................lll..",
    "..hhh................lll..",
    "..........................",
    "..........................",
    "..........................",
    "ggg....................aaa",
    "ggg....................aaa",
    "ggg....................aaa",
    "..........................",
    "..........................",
    "..........................",
    "..fff................bbb..",
    "..fff................bbb..",
    "..fff................bbb..",
    "..........................",
    "......eee........ccc......",
    "......eee...ddd..ccc......",
    "......eee...ddd..ccc......",
    "............ddd...........",
];

/// The ring's legend: dot `k`'s letter in `color(k)`.
fn ring_legend(color: impl Fn(usize) -> Option<Rgb888>) -> Vec<(char, Option<Rgb888>)> {
    (0..spinner::DOTS)
        .map(|dot| ((b'a' + dot as u8) as char, color(dot)))
        .collect()
}

/// Every dot in a colour of its own, so a dot drawn in another's place shows.
fn own(dot: usize) -> Rgb888 {
    Rgb888::new(0, 0, 8 * (dot as u8 + 1))
}

#[test]
fn the_ring_is_its_picture_dot_by_dot() {
    assert_eq!(size(&RING), (spinner::WIDTH, spinner::HEIGHT));
    let mut pixels = frame();
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    spinner::draw(&mut canvas, SPINNER_AT.0, SPINNER_AT.1, |dot| {
        Some(own(dot))
    });
    let legend = ring_legend(|dot| Some(own(dot)));
    assert_picture(&canvas, SPINNER_AT, &RING, &legend, "spinner");
    let rect = (SPINNER_AT.0, SPINNER_AT.1, spinner::WIDTH, spinner::HEIGHT);
    assert_untouched_outside(&canvas, rect, "spinner");
}

/// A comet: the dots in its trail take its fading colours and the dots
/// outside it are not drawn at all, wherever the head is and however long
/// the trail.
#[test]
fn the_ring_draws_only_the_comet() {
    for head in 0..spinner::DOTS {
        for trail in [0, 1, 10, spinner::DOTS] {
            let comet = |dot: usize| -> Option<Rgb888> {
                let lag = (head + spinner::DOTS - dot) % spinner::DOTS;
                (lag < trail).then(|| {
                    let value = 255 - (lag as u32 * 255) / trail as u32;
                    scale(Rgb888(0xffb000), value, 255)
                })
            };
            let mut pixels = frame();
            let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
            spinner::draw(&mut canvas, SPINNER_AT.0, SPINNER_AT.1, comet);
            let what = format!("comet head {head} trail {trail}");
            assert_picture(&canvas, SPINNER_AT, &RING, &ring_legend(comet), &what);
        }
    }
}

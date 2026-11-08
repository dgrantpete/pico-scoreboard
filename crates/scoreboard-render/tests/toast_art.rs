//! The procedural padlock and spinner ring (BACKLOG 116), pinned to the
//! toast PNGs they replace: pixel for pixel against the PNGs in `tests/art/`
//! (quantized today, exact for after BACKLOG 115), and against the compiled
//! sprites through every palette `toast.rs` builds for them — the padlock's
//! one colour, and the spinner's per-dot comet with keyed-out dots.

mod oracle;

use oracle::{Art, assert_drawn, assert_same_frame, assert_untouched_outside, buffer, widen};
use scoreboard_model::Rgb888;
use scoreboard_render::blit::Canvas;
use scoreboard_render::generated::layout;
use scoreboard_render::geometry::{HEIGHT, WIDTH};
use scoreboard_render::padlock::{self, COLOR};
use scoreboard_render::spinner::{self, DOTS};
use scoreboard_render::{MAGENTA, pack, pack_scaled};

/// Where `toast.rs` centres each icon on the panel.
const LOCK_AT: (i32, i32) = (WIDTH / 2 - padlock::WIDTH / 2, 19);
const SPINNER_AT: (i32, i32) = (
    WIDTH / 2 - spinner::WIDTH / 2,
    HEIGHT / 2 - spinner::HEIGHT / 2,
);

/// Primaries at toast brightnesses, as `toast.rs` packs them.
fn toast_colors() -> impl Iterator<Item = u16> {
    [0xffffff, 0x4e008e, 0xffb000, 0x10c060]
        .into_iter()
        .flat_map(|primary| {
            [255, 200, 128, 33].map(move |brightness| pack_scaled(Rgb888(primary), brightness, 255))
        })
}

#[test]
fn the_padlocks_are_their_pngs() {
    for (name, open) in [("toast_lock_closed", false), ("toast_lock_open", true)] {
        let art = Art::oracle(name);
        assert_eq!((art.width, art.height), (padlock::WIDTH, padlock::HEIGHT));
        let (x, y) = LOCK_AT;
        let mut pixels = buffer(WIDTH, HEIGHT);
        let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
        padlock::draw(&mut canvas, x, y, open, COLOR);
        assert_drawn(&canvas, x, y, &art, &[COLOR], name);
        assert_untouched_outside(&canvas, (x, y, padlock::WIDTH, padlock::HEIGHT), name);
    }
}

#[test]
fn the_padlock_takes_every_colour_the_toast_gives_it() {
    for color in toast_colors() {
        for (sprite, defaults, open) in [
            (
                layout::toast_lock_closed::SPRITE,
                layout::toast_lock_closed::PALETTE,
                false,
            ),
            (
                layout::toast_lock_open::SPRITE,
                layout::toast_lock_open::PALETTE,
                true,
            ),
        ] {
            let mut palette = defaults;
            palette[1] = color;
            let (x, y) = LOCK_AT;
            let (mut drawn, mut blitted) = (buffer(WIDTH, HEIGHT), buffer(WIDTH, HEIGHT));
            let mut drawn = Canvas::new(&mut drawn, WIDTH, HEIGHT);
            let mut blitted = Canvas::new(&mut blitted, WIDTH, HEIGHT);
            padlock::draw(&mut drawn, x, y, open, widen(color));
            blitted.blit(&sprite.tinted(&palette), x, y);
            assert_same_frame(&drawn, &blitted, "tinted padlock");
        }
    }
}

/// Dot `k`'s colour in the PNG: `(0, 0, 8(k + 1))`, RGB565 `k + 1`.
fn baked(dot: usize) -> Rgb888 {
    Rgb888::new(0, 0, 8 * (dot as u8 + 1))
}

#[test]
fn the_ring_is_its_png() {
    let art = Art::oracle("toast_spinner");
    assert_eq!((art.width, art.height), (spinner::WIDTH, spinner::HEIGHT));
    let (x, y) = SPINNER_AT;
    let mut pixels = buffer(WIDTH, HEIGHT);
    let mut canvas = Canvas::new(&mut pixels, WIDTH, HEIGHT);
    spinner::draw(&mut canvas, x, y, |dot| Some(baked(dot)));
    let palette: Vec<Rgb888> = (0..DOTS).map(baked).collect();
    assert_drawn(&canvas, x, y, &art, &palette, "spinner");
    let rect = (x, y, spinner::WIDTH, spinner::HEIGHT);
    assert_untouched_outside(&canvas, rect, "spinner");
}

/// A comet drawn dot by dot is the compiled ring blitted through the palette
/// `toast.rs` builds — each dot's entry, found by its baked value, set to the
/// dot's colour or to the key — wherever the head is and however long the
/// trail.
#[test]
fn the_ring_takes_every_comet_the_toast_gives_it() {
    let entry_of = |dot: usize| {
        layout::toast_spinner::PALETTE
            .iter()
            .position(|&entry| entry == pack(baked(dot)))
            .expect("every dot has a palette entry")
    };
    for primary in [0xffffff, 0x4e008e, 0xffb000] {
        for head in 0..DOTS {
            for trail in [0, 1, 10, DOTS] {
                let comet = |dot: usize| -> Option<u16> {
                    let lag = (head + DOTS - dot) % DOTS;
                    (lag < trail).then(|| {
                        let value = 255 - (lag as u32 * 255) / trail as u32;
                        pack_scaled(Rgb888(primary), value, 255)
                    })
                };
                let mut palette = layout::toast_spinner::PALETTE;
                for dot in 0..DOTS {
                    palette[entry_of(dot)] = comet(dot).unwrap_or(MAGENTA);
                }
                let (x, y) = SPINNER_AT;
                let (mut drawn, mut blitted) = (buffer(WIDTH, HEIGHT), buffer(WIDTH, HEIGHT));
                let mut drawn = Canvas::new(&mut drawn, WIDTH, HEIGHT);
                let mut blitted = Canvas::new(&mut blitted, WIDTH, HEIGHT);
                spinner::draw(&mut drawn, x, y, |dot| comet(dot).map(widen));
                blitted.blit(&layout::toast_spinner::SPRITE.tinted(&palette), x, y);
                assert_same_frame(&drawn, &blitted, "comet");
            }
        }
    }
}

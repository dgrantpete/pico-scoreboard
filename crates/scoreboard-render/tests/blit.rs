//! Pinning tests for the blitter, ported from
//! `tools/preview/tests/test_framebuf_shim.py`.
//!
//! The shim those tests cover is itself pinned against `compile_layout.py`'s
//! packers, and the goldens here come from running those same packers — so the
//! chain from "what the compiler wrote" to "what the panel shows" is checked
//! end to end without either side transcribing the other.

mod goldens;

use goldens::{PACKED_CASES, RGB565_COLORS, RGB565_PACKED};
use scoreboard_render::blit::{
    BYTES_PER_PIXEL, Canvas, FADE_TERMS, PixelFormat, Source, expand_rgb565,
};
use scoreboard_render::{BLACK, Rgb888, WHITE, generated::art};

const TEAL: Rgb888 = Rgb888::new(0x12, 0x34, 0x56);
const AMBER: Rgb888 = Rgb888::new(0xFF, 0xBF, 0x00);

/// A zeroed buffer for a `width` × `height` canvas.
fn buffer(width: usize, height: usize) -> Vec<u8> {
    vec![0u8; width * height * BYTES_PER_PIXEL]
}

/// Every raw source value mapped to a distinct color — the value in the blue
/// channel — so a blit through it reports exactly which index each pixel held.
fn identity_palette() -> [Option<Rgb888>; 256] {
    core::array::from_fn(|index| Some(Rgb888::new(0x80, 0, index as u8)))
}

fn format_for(name: &str) -> PixelFormat {
    match name.split('_').next().unwrap() {
        "mono" => PixelFormat::MonoHlsb,
        "gs2" => PixelFormat::Gs2Hmsb,
        "gs4" => PixelFormat::Gs4Hmsb,
        "gs8" => PixelFormat::Gs8,
        other => panic!("unknown golden format {other}"),
    }
}

/// RGB565 truncation, restated: the inverse of [`expand_rgb565`].
fn truncate(color: Rgb888) -> u16 {
    ((color.red() as u16 & 0xF8) << 8)
        | ((color.green() as u16 & 0xFC) << 3)
        | (color.blue() as u16 >> 3)
}

#[test]
fn packed_sources_read_back_the_indices_that_were_packed() {
    let palette = identity_palette();
    for case in PACKED_CASES {
        let mut pixels = buffer(case.width, case.height);
        let mut canvas = Canvas::new(&mut pixels, case.width as i32, case.height as i32);
        let source = Source::new(
            case.packed,
            case.width as i32,
            case.height as i32,
            format_for(case.name),
            Some(&palette),
        );
        canvas.blit(&source, 0, 0);

        let read: Vec<u8> = (0..case.height)
            .flat_map(|y| (0..case.width).map(move |x| (x as i32, y as i32)))
            .map(|(x, y)| canvas.pixel_at(x, y).unwrap().blue())
            .collect();
        assert_eq!(read, case.indices, "format round trip: {}", case.name);
    }
}

#[test]
fn gs2_puts_the_leftmost_pixel_in_the_low_bits() {
    // The formats disagree, and the compiled sprites are packed to match each
    // one exactly — GS2 low, GS4 high. Getting either backwards mirrors sprites
    // within every byte.
    let case = PACKED_CASES
        .iter()
        .find(|c| c.name == "gs2_leftmost")
        .unwrap();
    assert_eq!(case.packed[0] & 0x03, 1);
}

#[test]
fn gs4_puts_the_leftmost_pixel_in_the_high_nibble() {
    let case = PACKED_CASES
        .iter()
        .find(|c| c.name == "gs4_leftmost")
        .unwrap();
    assert_eq!(case.packed[0] >> 4, 0xA);
}

#[test]
fn rgb565_sources_are_little_endian_and_widen_by_bit_replication() {
    let mut pixels = buffer(3, 2);
    let mut canvas = Canvas::new(&mut pixels, 3, 2);
    let source = Source::new(RGB565_PACKED, 3, 2, PixelFormat::Rgb565, None);
    canvas.blit(&source, 0, 0);

    let read: Vec<Rgb888> = (0..2)
        .flat_map(|y| (0..3).map(move |x| (x, y)))
        .map(|(x, y)| canvas.pixel_at(x, y).unwrap())
        .collect();
    let widened: Vec<Rgb888> = RGB565_COLORS.iter().map(|&c| expand_rgb565(c)).collect();
    assert_eq!(read, widened);
    assert_eq!(
        read.iter().map(|&c| truncate(c)).collect::<Vec<_>>(),
        RGB565_COLORS
    );
    assert_eq!(RGB565_PACKED[0], (RGB565_COLORS[0] & 0xFF) as u8);
    assert_eq!(RGB565_PACKED[1], (RGB565_COLORS[0] >> 8) as u8);
}

#[test]
fn bit_replication_is_the_exact_inverse_of_truncation() {
    // What keeps a crest lighting the LEDs it lit on the RGB565 surface: the
    // widened colour truncates back to the value the backend sent, for all of
    // them, and the extremes land on black and full white.
    for value in 0..=u16::MAX {
        assert_eq!(truncate(expand_rgb565(value)), value, "{value:#06x}");
    }
    assert_eq!(expand_rgb565(0x0000), BLACK);
    assert_eq!(expand_rgb565(0xFFFF), WHITE);
    assert_eq!(expand_rgb565(0x8410), Rgb888::new(0x84, 0x82, 0x84));
}

#[test]
fn odd_widths_keep_rows_isolated() {
    // A 5-wide GS4 source rounds to 3 bytes per row; row 1 must not read row 0's
    // tail nibble.
    let palette = identity_palette();
    let case = PACKED_CASES
        .iter()
        .find(|c| c.name == "gs4_rows_5x2")
        .unwrap();
    let mut pixels = buffer(5, 2);
    let mut canvas = Canvas::new(&mut pixels, 5, 2);
    canvas.blit(
        &Source::new(case.packed, 5, 2, PixelFormat::Gs4Hmsb, Some(&palette)),
        0,
        0,
    );
    let row: Vec<u8> = (0..5)
        .map(|x| canvas.pixel_at(x, 1).unwrap().blue())
        .collect();
    assert_eq!(row, [6, 7, 8, 9, 10]);
}

#[test]
fn a_none_palette_entry_is_see_through() {
    let case = PACKED_CASES
        .iter()
        .find(|c| c.name == "gs2_leftmost")
        .unwrap();
    let palette = [None, Some(WHITE), Some(WHITE), Some(WHITE)];
    let mut pixels = buffer(4, 1);
    let mut canvas = Canvas::new(&mut pixels, 4, 1);
    canvas.fill(TEAL);
    canvas.blit(
        &Source::new(case.packed, 4, 1, PixelFormat::Gs2Hmsb, Some(&palette)),
        0,
        0,
    );
    // Index 1 drew; the three index-0 pixels have no colour and were skipped.
    assert_eq!(canvas.pixel_at(0, 0), Some(WHITE));
    assert_eq!(canvas.pixel_at(1, 0), Some(TEAL));
    assert_eq!(canvas.pixel_at(2, 0), Some(TEAL));
    assert_eq!(canvas.pixel_at(3, 0), Some(TEAL));
}

#[test]
fn every_colour_a_palette_can_hold_draws() {
    // BACKLOG 112, gone: MicroPython keyed on the mapped colour RGB565 0xF81F,
    // so magenta — and anything that packed to it, red and blue at 248 or
    // above with green under 4 — drew nothing. Transparency is a `None` entry
    // now, and those colours are colours.
    let case = PACKED_CASES
        .iter()
        .find(|c| c.name == "gs2_leftmost")
        .unwrap();
    for ink in [
        Rgb888::new(255, 0, 255),
        Rgb888::new(248, 0, 248),
        Rgb888::new(255, 3, 250),
    ] {
        let palette = [Some(ink); 4];
        let mut pixels = buffer(4, 1);
        let mut canvas = Canvas::new(&mut pixels, 4, 1);
        canvas.fill(TEAL);
        canvas.blit(
            &Source::new(case.packed, 4, 1, PixelFormat::Gs2Hmsb, Some(&palette)),
            0,
            0,
        );
        assert!(
            (0..4).all(|x| canvas.pixel_at(x, 0) == Some(ink)),
            "{ink:?}"
        );
    }
}

#[test]
fn every_transparent_sprite_reserves_a_see_through_index_0() {
    for (name, sprite) in [
        ("base_marker", art::base_marker::SPRITE),
        ("field", art::field::SPRITE),
        ("football_ball", art::football_ball::SPRITE),
        ("football_field", art::football_field::SPRITE),
    ] {
        let palette = sprite.palette.expect("paletted sprite");
        assert_eq!(palette[0], None, "{name} reserves index 0 as see-through");
        assert!(
            palette[1..].iter().all(Option::is_some),
            "{name}: only index 0 is see-through"
        );
    }
}

#[test]
fn negative_offsets_clip_instead_of_wrapping() {
    let palette = identity_palette();
    let case = PACKED_CASES.iter().find(|c| c.name == "gs8_3x3").unwrap();
    let mut pixels = buffer(3, 3);
    let mut canvas = Canvas::new(&mut pixels, 3, 3);
    canvas.blit(
        &Source::new(case.packed, 3, 3, PixelFormat::Gs8, Some(&palette)),
        -2,
        -1,
    );
    // Source (2, 1) lands at (0, 0); the two columns and one row before it are
    // dropped, not wrapped onto the previous row.
    let expected = case.indices[3 + 2];
    assert_eq!(canvas.pixel_at(0, 0).unwrap().blue(), expected);
    assert_eq!(canvas.pixel_at(1, 0), Some(BLACK));
}

#[test]
fn a_region_writes_only_inside_itself() {
    // A gray fills as a byte run and any other colour a pixel at a time; both
    // have to stop at the region's edge.
    for color in [Rgb888::new(0xAB, 0xAB, 0xAB), AMBER] {
        let mut pixels = buffer(4, 4);
        let mut canvas = Canvas::new(&mut pixels, 4, 4);
        canvas.region(1, 1, 2, 2).fill(color);
        for y in 0..4 {
            for x in 0..4 {
                let inside = (1..=2).contains(&x) && (1..=2).contains(&y);
                let expected = if inside { color } else { BLACK };
                assert_eq!(canvas.pixel_at(x, y), Some(expected), "at ({x}, {y})");
            }
        }
    }
}

#[test]
fn a_region_clips_an_overlong_write_instead_of_spilling() {
    // The property that lets every renderer draw without masking: writing past a
    // region's width must not land on the next parent row.
    let mut pixels = buffer(4, 2);
    let mut canvas = Canvas::new(&mut pixels, 4, 2);
    canvas.region(0, 0, 2, 2).fill_rect(0, 0, 10, 1, TEAL);
    assert_eq!(canvas.pixel_at(0, 0), Some(TEAL));
    assert_eq!(canvas.pixel_at(1, 0), Some(TEAL));
    assert_eq!(canvas.pixel_at(2, 0), Some(BLACK));
    assert_eq!(canvas.pixel_at(3, 0), Some(BLACK));
}

#[test]
fn regions_nest() {
    let mut pixels = buffer(8, 8);
    let mut canvas = Canvas::new(&mut pixels, 8, 8);
    let mut outer = canvas.region(2, 2, 4, 4);
    outer.region(1, 1, 2, 2).fill(TEAL);
    assert_eq!(canvas.pixel_at(3, 3), Some(TEAL));
    assert_eq!(canvas.pixel_at(4, 4), Some(TEAL));
    assert_eq!(canvas.pixel_at(2, 2), Some(BLACK));
    assert_eq!(canvas.pixel_at(5, 5), Some(BLACK));
}

#[test]
fn pixels_are_three_bytes_in_rgb_order() {
    let mut pixels = buffer(2, 1);
    let mut canvas = Canvas::new(&mut pixels, 2, 1);
    canvas.pixel(1, 0, TEAL);
    assert_eq!(pixels, [0, 0, 0, 0x12, 0x34, 0x56]);
}

#[test]
fn lines_and_rules_clip() {
    let (one, two) = (Rgb888::new(0x11, 0x11, 0x11), Rgb888::new(0x22, 0x22, 0x22));
    let mut pixels = buffer(8, 8);
    let mut canvas = Canvas::new(&mut pixels, 8, 8);
    canvas.hline(-3, 2, 20, one);
    assert!((0..8).all(|x| canvas.pixel_at(x, 2) == Some(one)));
    assert_eq!(canvas.pixel_at(0, 1), Some(BLACK));
    canvas.vline(5, -2, 20, two);
    assert!((0..8).all(|y| canvas.pixel_at(5, y) == Some(two)));
}

#[test]
fn line_matches_micropythons_bresenham() {
    // Pinned against modframebuf.c: the loop draws dx pixels stepping the minor
    // axis whenever the error term goes non-negative, then sets the second
    // endpoint unconditionally, so both endpoints are always lit.
    let mut pixels = buffer(16, 16);
    let mut canvas = Canvas::new(&mut pixels, 16, 16);
    canvas.line(2, 10, 10, 0, TEAL);
    assert_eq!(canvas.pixel_at(2, 10), Some(TEAL));
    assert_eq!(canvas.pixel_at(10, 0), Some(TEAL));
    let lit = (0..16)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .filter(|(x, y)| canvas.pixel_at(*x, *y) != Some(BLACK))
        .count();
    assert_eq!(lit, 11, "steep: one pixel per y step, both endpoints");

    let mut pixels = buffer(16, 16);
    let mut canvas = Canvas::new(&mut pixels, 16, 16);
    canvas.line(0, 0, 15, 5, AMBER);
    assert_eq!(canvas.pixel_at(0, 0), Some(AMBER));
    assert_eq!(canvas.pixel_at(15, 5), Some(AMBER));
    let lit = (0..16)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .filter(|(x, y)| canvas.pixel_at(*x, *y) != Some(BLACK))
        .count();
    assert_eq!(lit, 16, "shallow: one pixel per x step");
}

#[test]
fn dim_scales_every_channel_by_its_rung() {
    // The closed form, per channel byte: c/2, plus c/4 and c/8 as the rung
    // asks, each term truncated on its own. Over pseudorandom bytes, so a mask
    // that let one channel bleed into its neighbour shows up; and at two
    // sizes, one a whole number of words (a frame) and one not (the tail).
    let mut state = 0x5C0DE_u32;
    for (width, height) in [(8, 4), (3, 1)] {
        let source: Vec<u8> = (0..width * height * BYTES_PER_PIXEL)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        for (step, terms) in FADE_TERMS.iter().enumerate() {
            let mut pixels = source.clone();
            Canvas::new(&mut pixels, width as i32, height as i32).dim(*terms);
            let expected: Vec<u8> = source
                .iter()
                .map(|&c| {
                    (c >> 1)
                        + if terms.half_step { c >> 2 } else { 0 }
                        + if terms.quarter_step { c >> 3 } else { 0 }
                })
                .collect();
            assert_eq!(
                pixels, expected,
                "{width}x{height}, fade ladder step {step}"
            );
        }
    }
}

#[test]
fn the_fade_ladder_darkens_monotonically() {
    let mut previous = WHITE;
    for terms in FADE_TERMS {
        let mut pixels = buffer(2, 1);
        let mut canvas = Canvas::new(&mut pixels, 2, 1);
        canvas.fill(WHITE);
        canvas.dim(terms);
        let dimmed = canvas.pixel_at(0, 0).unwrap();
        assert_eq!(canvas.pixel_at(1, 0), Some(dimmed), "every pixel alike");
        assert!(dimmed.red() < previous.red(), "{dimmed:?} !< {previous:?}");
        previous = dimmed;
    }
    // The last rung is the held level: half brightness in every channel.
    assert_eq!(previous, Rgb888::new(127, 127, 127));
}

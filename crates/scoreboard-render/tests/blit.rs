//! Pinning tests for the blitter, ported from
//! `tools/preview/tests/test_framebuf_shim.py`.
//!
//! The shim those tests cover is itself pinned against `compile_layout.py`'s
//! packers, and the goldens here come from running those same packers — so the
//! chain from "what the compiler wrote" to "what the panel shows" is checked
//! end to end without either side transcribing the other.

mod goldens;

use goldens::{DIM_CASES, DIM_SOURCE, PACKED_CASES, RGB565_COLORS, RGB565_PACKED};
use scoreboard_render::blit::{Canvas, FADE_TERMS, PixelFormat, Source};
use scoreboard_render::{MAGENTA, generated::layout};

/// Every raw source value mapped to a distinct color, so a blit through it
/// reports exactly which index each pixel held.
fn identity_palette() -> [u16; 256] {
    core::array::from_fn(|index| 0x8000 | index as u16)
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

#[test]
fn packed_sources_read_back_the_indices_that_were_packed() {
    let palette = identity_palette();
    for case in PACKED_CASES {
        let mut pixels = vec![0u8; case.width * case.height * 2];
        let mut canvas = Canvas::new(&mut pixels, case.width as i32, case.height as i32);
        let source = Source::new(
            case.packed,
            case.width as i32,
            case.height as i32,
            format_for(case.name),
            Some(&palette),
            None,
        );
        canvas.blit(&source, 0, 0);

        let read: Vec<u8> = (0..case.height)
            .flat_map(|y| (0..case.width).map(move |x| (x as i32, y as i32)))
            .map(|(x, y)| (canvas.pixel_at(x, y).unwrap() & 0xFF) as u8)
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
fn rgb565_sources_are_little_endian() {
    let mut pixels = vec![0u8; 3 * 2 * 2];
    let mut canvas = Canvas::new(&mut pixels, 3, 2);
    let source = Source::new(RGB565_PACKED, 3, 2, PixelFormat::Rgb565, None, None);
    canvas.blit(&source, 0, 0);

    let read: Vec<u16> = (0..2)
        .flat_map(|y| (0..3).map(move |x| (x, y)))
        .map(|(x, y)| canvas.pixel_at(x, y).unwrap())
        .collect();
    assert_eq!(read, RGB565_COLORS);
    assert_eq!(RGB565_PACKED[0], (RGB565_COLORS[0] & 0xFF) as u8);
    assert_eq!(RGB565_PACKED[1], (RGB565_COLORS[0] >> 8) as u8);
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
    let mut pixels = vec![0u8; 5 * 2 * 2];
    let mut canvas = Canvas::new(&mut pixels, 5, 2);
    canvas.blit(
        &Source::new(
            case.packed,
            5,
            2,
            PixelFormat::Gs4Hmsb,
            Some(&palette),
            None,
        ),
        0,
        0,
    );
    let row: Vec<u8> = (0..5)
        .map(|x| (canvas.pixel_at(x, 1).unwrap() & 0xFF) as u8)
        .collect();
    assert_eq!(row, [6, 7, 8, 9, 10]);
}

#[test]
fn the_palette_is_applied_before_the_colorkey() {
    // The detail every sprite's KEY depends on: index 0 maps to magenta, and it
    // is the mapped color that is compared, so one key value serves paletted and
    // RGB565 sprites alike.
    let case = PACKED_CASES
        .iter()
        .find(|c| c.name == "gs2_leftmost")
        .unwrap();
    let palette = [MAGENTA, 0xFFFF, 0xFFFF, 0xFFFF];
    let mut pixels = vec![0u8; 4 * 2];
    let mut canvas = Canvas::new(&mut pixels, 4, 1);
    canvas.fill(0x1234);
    canvas.blit(
        &Source::new(
            case.packed,
            4,
            1,
            PixelFormat::Gs2Hmsb,
            Some(&palette),
            Some(MAGENTA),
        ),
        0,
        0,
    );
    // Index 1 drew; the three index-0 pixels mapped to the key and were skipped.
    assert_eq!(canvas.pixel_at(0, 0), Some(0xFFFF));
    assert_eq!(canvas.pixel_at(1, 0), Some(0x1234));
    assert_eq!(canvas.pixel_at(2, 0), Some(0x1234));
    assert_eq!(canvas.pixel_at(3, 0), Some(0x1234));
}

#[test]
fn every_transparent_sprite_keys_on_mapped_magenta() {
    for (name, sprite) in [
        ("dot", layout::dot::SPRITE),
        ("base_marker", layout::base_marker::SPRITE),
        ("field", layout::field::SPRITE),
        ("football_field", layout::football_field::SPRITE),
        ("toast_spinner", layout::toast_spinner::SPRITE),
        ("toast_lock_closed", layout::toast_lock_closed::SPRITE),
    ] {
        assert_eq!(sprite.key, Some(MAGENTA), "{name} key");
        let palette = sprite.palette.expect("paletted sprite");
        assert_eq!(palette[0], MAGENTA, "{name} reserves index 0 for magenta");
    }
}

#[test]
fn negative_offsets_clip_instead_of_wrapping() {
    let palette = identity_palette();
    let case = PACKED_CASES.iter().find(|c| c.name == "gs8_3x3").unwrap();
    let mut pixels = vec![0u8; 3 * 3 * 2];
    let mut canvas = Canvas::new(&mut pixels, 3, 3);
    canvas.blit(
        &Source::new(case.packed, 3, 3, PixelFormat::Gs8, Some(&palette), None),
        -2,
        -1,
    );
    // Source (2, 1) lands at (0, 0); the two columns and one row before it are
    // dropped, not wrapped onto the previous row.
    let expected = case.indices[3 + 2];
    assert_eq!(canvas.pixel_at(0, 0).unwrap() & 0xFF, expected as u16);
    assert_eq!(canvas.pixel_at(1, 0), Some(0));
}

#[test]
fn a_region_writes_only_inside_itself() {
    let mut pixels = vec![0u8; 4 * 4 * 2];
    let mut canvas = Canvas::new(&mut pixels, 4, 4);
    canvas.region(1, 1, 2, 2).fill(0xABCD);
    for y in 0..4 {
        for x in 0..4 {
            let inside = (1..=2).contains(&x) && (1..=2).contains(&y);
            let expected = if inside { 0xABCD } else { 0x0000 };
            assert_eq!(canvas.pixel_at(x, y), Some(expected), "at ({x}, {y})");
        }
    }
}

#[test]
fn a_region_clips_an_overlong_write_instead_of_spilling() {
    // The property that lets every renderer draw without masking: writing past a
    // region's width must not land on the next parent row.
    let mut pixels = vec![0u8; 4 * 2 * 2];
    let mut canvas = Canvas::new(&mut pixels, 4, 2);
    canvas.region(0, 0, 2, 2).fill_rect(0, 0, 10, 1, 0x7777);
    assert_eq!(canvas.pixel_at(0, 0), Some(0x7777));
    assert_eq!(canvas.pixel_at(1, 0), Some(0x7777));
    assert_eq!(canvas.pixel_at(2, 0), Some(0x0000));
    assert_eq!(canvas.pixel_at(3, 0), Some(0x0000));
}

#[test]
fn regions_nest() {
    let mut pixels = vec![0u8; 8 * 8 * 2];
    let mut canvas = Canvas::new(&mut pixels, 8, 8);
    let mut outer = canvas.region(2, 2, 4, 4);
    outer.region(1, 1, 2, 2).fill(0x0F0F);
    assert_eq!(canvas.pixel_at(3, 3), Some(0x0F0F));
    assert_eq!(canvas.pixel_at(4, 4), Some(0x0F0F));
    assert_eq!(canvas.pixel_at(2, 2), Some(0x0000));
    assert_eq!(canvas.pixel_at(5, 5), Some(0x0000));
}

#[test]
fn lines_and_rules_clip() {
    let mut pixels = vec![0u8; 8 * 8 * 2];
    let mut canvas = Canvas::new(&mut pixels, 8, 8);
    canvas.hline(-3, 2, 20, 0x1111);
    assert!((0..8).all(|x| canvas.pixel_at(x, 2) == Some(0x1111)));
    assert_eq!(canvas.pixel_at(0, 1), Some(0x0000));
    canvas.vline(5, -2, 20, 0x2222);
    assert!((0..8).all(|y| canvas.pixel_at(5, y) == Some(0x2222)));
}

#[test]
fn line_matches_micropythons_bresenham() {
    // Pinned against modframebuf.c: the loop draws dx pixels stepping the minor
    // axis whenever the error term goes non-negative, then sets the second
    // endpoint unconditionally, so both endpoints are always lit.
    let mut pixels = vec![0u8; 16 * 16 * 2];
    let mut canvas = Canvas::new(&mut pixels, 16, 16);
    canvas.line(2, 10, 10, 0, 0x1111);
    assert_eq!(canvas.pixel_at(2, 10), Some(0x1111));
    assert_eq!(canvas.pixel_at(10, 0), Some(0x1111));
    let lit = (0..16)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .filter(|(x, y)| canvas.pixel_at(*x, *y) != Some(0))
        .count();
    assert_eq!(lit, 11, "steep: one pixel per y step, both endpoints");

    let mut pixels = vec![0u8; 16 * 16 * 2];
    let mut canvas = Canvas::new(&mut pixels, 16, 16);
    canvas.line(0, 0, 15, 5, 0x2222);
    assert_eq!(canvas.pixel_at(0, 0), Some(0x2222));
    assert_eq!(canvas.pixel_at(15, 5), Some(0x2222));
    let lit = (0..16)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .filter(|(x, y)| canvas.pixel_at(*x, *y) != Some(0))
        .count();
    assert_eq!(lit, 16, "shallow: one pixel per x step");
}

#[test]
fn dim_matches_the_python_mask_math() {
    // The goldens come from executing display.py's own CPython _dim_frame, which
    // is required to stay mask-identical to the viper one that runs on the
    // device.
    for case in DIM_CASES {
        let mut pixels = DIM_SOURCE.to_vec();
        let mut canvas = Canvas::new(&mut pixels, 8, 4);
        canvas.dim(FADE_TERMS[case.step]);
        assert_eq!(pixels, case.dimmed, "fade ladder step {}", case.step);
    }
}

#[test]
fn the_fade_ladder_darkens_monotonically() {
    let white = 0xFFFFu16;
    let mut previous = white as u32;
    for terms in FADE_TERMS {
        let mut pixels = vec![0u8; 2 * 2];
        let mut canvas = Canvas::new(&mut pixels, 2, 1);
        canvas.fill(white);
        canvas.dim(terms);
        let dimmed = canvas.pixel_at(0, 0).unwrap() as u32;
        assert!(dimmed < previous, "{dimmed:#06x} !< {previous:#06x}");
        previous = dimmed;
    }
    // The last rung is the held level: half brightness in every channel.
    assert_eq!(previous, 0x7BEF);
}

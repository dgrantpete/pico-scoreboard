//! The RGB565 bit layout, pinned to literals.
//!
//! Every other sprite test compares `finish`'s output against
//! `common::pack565`, whose own doc says it is "same math as the crate's pack"
//! — so the packing is currently pinned only against a copy of itself. A
//! coordinated edit that swapped red and blue in both places, or that moved
//! green's six bits, would pass the whole suite and put every crest on a panel
//! in the wrong colors.
//!
//! That matters beyond tidiness because the firmware's crest pool stores these
//! `u16`s as little-endian byte pairs and hands them to a renderer that never
//! re-checks them, alongside sprites the backend packed independently
//! (`backend/src/logo.rs`, `encode_rgb565_raw`). Three encoders have to agree
//! on where red lives, and nothing at compile time makes them.
//!
//! So: decode a real PNG of one known color and assert the exact `u16`, with
//! the constant written out rather than computed.

mod common;

use common::*;
use png_stream::{Rgb8, SPRITE_PIXELS, Scratch};

/// Any background at all — every test here is fully opaque, so it must not
/// reach the output. Deliberately not black: a blend bug that leaked the
/// background would then be invisible.
const BG: Rgb8 = Rgb8::new(90, 90, 90);

/// Decode a uniform 48×48 opaque RGB image and return the one color every
/// cell must hold. 48 is a whole multiple of 24, so every cell covers the same
/// four source pixels and the box filter is exact.
fn uniform(r: u8, g: u8, b: u8) -> u16 {
    let (w, h) = (48, 48);
    let pixels: Vec<u8> = (0..w * h).flat_map(|_| [r, g, b]).collect();
    let data = encode_png(w, h, png::ColorType::Rgb, png::Filter::NoFilter, &pixels);
    let mut scratch = Scratch::new();
    let sprite = sprite_whole(&mut scratch, &data, BG).expect("decode uniform image");
    for (cell, &value) in sprite.iter().enumerate() {
        assert_eq!(value, sprite[0], "cell {cell} differs from cell 0");
    }
    assert_eq!(sprite.len(), SPRITE_PIXELS);
    sprite[0]
}

/// `0xF800` is red in bits 15..11. Written as a literal on purpose: this is
/// the assertion that a channel swap cannot survive.
#[test]
fn red_occupies_the_top_five_bits() {
    assert_eq!(uniform(255, 0, 0), 0xF800);
}

/// `0x07E0` — green's six bits at 10..5.
#[test]
fn green_occupies_the_middle_six_bits() {
    assert_eq!(uniform(0, 255, 0), 0x07E0);
}

/// `0x001F` — blue's five bits at 4..0.
#[test]
fn blue_occupies_the_bottom_five_bits() {
    assert_eq!(uniform(0, 0, 255), 0x001F);
}

#[test]
fn black_and_white_are_the_endpoints() {
    assert_eq!(uniform(0, 0, 0), 0x0000);
    assert_eq!(uniform(255, 255, 255), 0xFFFF);
}

/// One asymmetric color, so a test suite that happened to be right about the
/// saturated primaries and wrong about the rescale still fails.
///
/// `(0x12, 0x34, 0x56)` → r5 = round(18·31/255) = 2, g6 = round(52·63/255) =
/// 13, b5 = round(86·31/255) = 10 → `(2 << 11) | (13 << 5) | 10` = `0x11AA`.
#[test]
fn a_mid_tone_rescales_round_to_nearest() {
    assert_eq!(uniform(0x12, 0x34, 0x56), 0x11AA);
}

/// The transparent case takes the same path: a fully transparent image is
/// pure background, and the background is packed by the identical formula.
///
/// `(90, 90, 90)` → r5 = round(90·31/255) = 11, g6 = round(90·63/255) = 22,
/// b5 = 11 → `(11 << 11) | (22 << 5) | 11` = `0x5ACB`.
#[test]
fn a_transparent_image_is_the_background_in_the_same_layout() {
    let (w, h) = (48, 48);
    let pixels: Vec<u8> = (0..w * h).flat_map(|_| [255, 0, 0, 0]).collect();
    let data = encode_png(w, h, png::ColorType::Rgba, png::Filter::NoFilter, &pixels);
    let mut scratch = Scratch::new();
    let sprite = sprite_whole(&mut scratch, &data, BG).expect("decode transparent image");
    for (cell, &value) in sprite.iter().enumerate() {
        assert_eq!(value, 0x5ACB, "cell {cell}");
    }
}

/// A sprite is 24×24 `u16`s, which is the 1,152 B a crest slot holds.
///
/// The firmware asserts the same equality against its own `LOGO_BYTES`
/// (`firmware-rs/app/src/logos.rs`); this is the half of it that belongs to
/// this crate, so a change here fails at home rather than only downstream.
#[test]
fn a_sprite_is_1152_bytes_of_pixels() {
    assert_eq!(SPRITE_PIXELS, 24 * 24);
    assert_eq!(SPRITE_PIXELS * size_of::<u16>(), 1_152);
}

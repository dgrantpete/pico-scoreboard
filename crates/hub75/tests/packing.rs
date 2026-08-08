//! Bitplane packing vs hand-computed cases (layout per bitplanes.c and
//! INVENTORY.md §3: plane-major, LSB plane first, one byte per pixel pair
//! packing R1,G1,B1,R2,G2,B2 in bits 0..=5).

use hub75::gamma::Gamma;
use hub75::geometry::{
    BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, RGB565_FRAME_BYTES, RGB888_FRAME_BYTES,
    ROW_ADDRESS_COUNT, SHIFT_REGISTER_DEPTH, WIDTH,
};
use hub75::packing::{pack_rgb565, pack_rgb888};

const IDENTITY: [u8; 256] = {
    let mut lut = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        lut[i] = i as u8;
        i += 1;
    }
    lut
};

fn set_rgb565(frame: &mut [u8; RGB565_FRAME_BYTES], x: usize, y: usize, color: u16) {
    let index = (y * WIDTH + x) * 2;
    frame[index..index + 2].copy_from_slice(&color.to_le_bytes());
}

/// Expected byte index of pixel-pair `(pair_row, x)` in plane `plane`.
fn plane_byte(plane: usize, pair_row: usize, x: usize) -> usize {
    plane * BITPLANE_BYTES + pair_row * SHIFT_REGISTER_DEPTH + x
}

#[test]
fn single_top_half_pixel_sets_r1_lane() {
    let mut frame = [0u8; RGB565_FRAME_BYTES];
    // Pure red, full scale: R5 = 31 expands to 255 -> bit set in all planes.
    set_rgb565(&mut frame, 0, 0, 0xF800);
    let mut out = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb565(&frame, &IDENTITY, &mut out);

    for plane in 0..8 {
        assert_eq!(out[plane_byte(plane, 0, 0)], 0b000001, "plane {plane}");
    }
    assert_eq!(out.iter().map(|&b| b as u32).sum::<u32>(), 8);
}

#[test]
fn bottom_half_pixel_lands_in_same_byte_upper_lanes() {
    let mut frame = [0u8; RGB565_FRAME_BYTES];
    // (x=5, y=40): bottom half, pair row 40 - 32 = 8. Full-scale green +
    // blue -> lanes G2 (bit 4) and B2 (bit 5) in every plane.
    set_rgb565(&mut frame, 5, 40, 0x07FF);
    let mut out = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb565(&frame, &IDENTITY, &mut out);

    for plane in 0..8 {
        assert_eq!(out[plane_byte(plane, 8, 5)], 0b110000, "plane {plane}");
    }
}

#[test]
fn rgb565_expansion_replicates_msbs() {
    let mut frame = [0u8; RGB565_FRAME_BYTES];
    // R5=1, G6=1, B5=1: expand to r=0b00001000, g=0b00000100, b=0b00001000
    // (no replication reaches the low bits for these small values).
    set_rgb565(&mut frame, 0, 0, (1 << 11) | (1 << 5) | 1);
    let mut out = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb565(&frame, &IDENTITY, &mut out);

    for plane in 0..8 {
        let mut expected = 0u8;
        if (8 >> plane) & 1 == 1 {
            expected |= 0b000001; // R1 carries bit 3
            expected |= 0b000100; // B1 carries bit 3
        }
        if (4 >> plane) & 1 == 1 {
            expected |= 0b000010; // G1 carries bit 2
        }
        assert_eq!(out[plane_byte(plane, 0, 0)], expected, "plane {plane}");
    }

    // Full-scale channels replicate to exactly 255: every plane lit in the
    // top-half lanes (the paired bottom-half pixel stays black).
    let mut frame = [0u8; RGB565_FRAME_BYTES];
    set_rgb565(&mut frame, 3, 2, 0xFFFF);
    let mut out = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb565(&frame, &IDENTITY, &mut out);
    for plane in 0..8 {
        assert_eq!(out[plane_byte(plane, 2, 3)], 0b000111, "plane {plane}");
    }
}

#[test]
fn rgb888_pixel_pair_packs_both_halves() {
    let mut frame = [0u8; RGB888_FRAME_BYTES];
    let top = (10 * WIDTH + 100) * 3; // y=10 (top half of pair 10)
    let bottom = ((10 + ROW_ADDRESS_COUNT) * WIDTH + 100) * 3; // y=42
    frame[top..top + 3].copy_from_slice(&[0x80, 0x01, 0x00]); // r1=128, g1=1
    frame[bottom..bottom + 3].copy_from_slice(&[0x00, 0x00, 0xFF]); // b2=255

    let mut out = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb888(&frame, &IDENTITY, &mut out);

    for plane in 0..8 {
        let mut expected = 0u8;
        if plane == 7 {
            expected |= 0b000001; // r1 = 0x80: bit 7 only
        }
        if plane == 0 {
            expected |= 0b000010; // g1 = 1: bit 0 only
        }
        expected |= 0b100000; // b2 = 255: every plane
        assert_eq!(out[plane_byte(plane, 10, 100)], expected, "plane {plane}");
    }
}

#[test]
fn gamma_lut_applies_during_packing() {
    let mut lut = IDENTITY;
    lut[255] = 0b1010_1010;
    let mut frame = [0u8; RGB565_FRAME_BYTES];
    set_rgb565(&mut frame, 0, 0, 0xF800); // r1 -> lut[255]
    let mut out = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb565(&frame, &lut, &mut out);

    for plane in 0..8 {
        let expected = if plane % 2 == 1 { 0b000001 } else { 0 };
        assert_eq!(out[plane_byte(plane, 0, 0)], expected, "plane {plane}");
    }
}

/// Cross-check the two loaders: an RGB888 frame built from the documented
/// RGB565 channel expansion must pack identically to the RGB565 original,
/// including through a non-trivial LUT.
#[test]
fn rgb888_and_rgb565_loaders_agree() {
    let mut rgb565 = [0u8; RGB565_FRAME_BYTES];
    let mut state = 0x12345678u32;
    for byte in rgb565.iter_mut() {
        // xorshift32: deterministic full-coverage noise
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }

    let mut rgb888 = [0u8; RGB888_FRAME_BYTES];
    for pixel in 0..rgb565.len() / 2 {
        let lo = rgb565[pixel * 2] as u32;
        let hi = rgb565[pixel * 2 + 1] as u32;
        let r = (hi & 0b1111_1000) as u8;
        let g = (((hi << 5) | (lo >> 3)) & 0b1111_1100) as u8;
        let b = ((lo << 3) & 0b1111_1000) as u8;
        rgb888[pixel * 3] = r | (r >> 5);
        rgb888[pixel * 3 + 1] = g | (g >> 6);
        rgb888[pixel * 3 + 2] = b | (b >> 5);
    }

    let lut = Gamma::Srgb.build_lut();
    let mut out_565 = [0u8; BITPLANE_BUFFER_BYTES];
    let mut out_888 = [0u8; BITPLANE_BUFFER_BYTES];
    pack_rgb565(&rgb565, &lut, &mut out_565);
    pack_rgb888(&rgb888, &lut, &mut out_888);
    assert_eq!(out_565, out_888);
}

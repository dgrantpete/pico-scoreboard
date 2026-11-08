//! Bitplane packing vs hand-computed cases (layout per bitplanes.c and
//! INVENTORY.md §3: plane-major, LSB plane first, one byte per pixel pair
//! packing R1,G1,B1,R2,G2,B2 in bits 0..=5), plus two properties over
//! full-coverage inputs: the fused-table pack is byte-identical to the scalar
//! RGB888 pack it replaced, and to the scalar RGB565 pack that ran on silicon
//! before it, over bit-replicated 565 frames (`reference/mod.rs`).

mod reference;

use hub75::gamma::Gamma;
use hub75::geometry::{
    BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, RGB888_FRAME_BYTES, ROW_ADDRESS_COUNT,
    SHIFT_REGISTER_DEPTH, WIDTH,
};
use hub75::packing::{FusedTables, pack_rgb888};
use reference::{RGB565_FRAME_BYTES, pack_rgb565_reference, pack_rgb888_reference};

const IDENTITY: [u8; 256] = {
    let mut lut = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        lut[i] = i as u8;
        i += 1;
    }
    lut
};

fn set_rgb888(frame: &mut [u8; RGB888_FRAME_BYTES], x: usize, y: usize, color: [u8; 3]) {
    let index = (y * WIDTH + x) * 3;
    frame[index..index + 3].copy_from_slice(&color);
}

/// Expected byte index of pixel-pair `(pair_row, x)` in plane `plane`.
fn plane_byte(plane: usize, pair_row: usize, x: usize) -> usize {
    plane * BITPLANE_BYTES + pair_row * SHIFT_REGISTER_DEPTH + x
}

fn pack(frame: &[u8; RGB888_FRAME_BYTES], lut: &[u8; 256]) -> Box<[u8; BITPLANE_BUFFER_BYTES]> {
    let mut out = Box::new([0u8; BITPLANE_BUFFER_BYTES]);
    pack_rgb888(frame, &FusedTables::new(lut), &mut out);
    out
}

#[test]
fn single_top_half_pixel_sets_r1_lane() {
    let mut frame = [0u8; RGB888_FRAME_BYTES];
    // Pure red, full scale -> bit set in all planes.
    set_rgb888(&mut frame, 0, 0, [255, 0, 0]);
    let out = pack(&frame, &IDENTITY);

    for plane in 0..8 {
        assert_eq!(out[plane_byte(plane, 0, 0)], 0b000001, "plane {plane}");
    }
    assert_eq!(out.iter().map(|&b| b as u32).sum::<u32>(), 8);
}

#[test]
fn bottom_half_pixel_lands_in_same_byte_upper_lanes() {
    let mut frame = [0u8; RGB888_FRAME_BYTES];
    // (x=5, y=40): bottom half, pair row 40 - 32 = 8. Full-scale green +
    // blue -> lanes G2 (bit 4) and B2 (bit 5) in every plane.
    set_rgb888(&mut frame, 5, 40, [0, 255, 255]);
    let out = pack(&frame, &IDENTITY);

    for plane in 0..8 {
        assert_eq!(out[plane_byte(plane, 8, 5)], 0b110000, "plane {plane}");
    }
}

#[test]
fn rgb888_pixel_pair_packs_both_halves() {
    let mut frame = [0u8; RGB888_FRAME_BYTES];
    set_rgb888(&mut frame, 100, 10, [0x80, 0x01, 0x00]); // r1=128, g1=1
    set_rgb888(&mut frame, 100, 10 + ROW_ADDRESS_COUNT, [0x00, 0x00, 0xFF]); // b2=255
    let out = pack(&frame, &IDENTITY);

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
fn every_lane_carries_every_plane_independently() {
    // One pixel pair, each of its six channels a different single-bit value,
    // so a lane shift that bled into its neighbour or a plane byte that landed
    // one slot off shows up as a wrong bit rather than a coincidental match.
    let mut frame = [0u8; RGB888_FRAME_BYTES];
    set_rgb888(&mut frame, 7, 3, [1 << 0, 1 << 1, 1 << 2]);
    set_rgb888(
        &mut frame,
        7,
        3 + ROW_ADDRESS_COUNT,
        [1 << 5, 1 << 6, 1 << 7],
    );
    let out = pack(&frame, &IDENTITY);

    let expected = [
        0b000001, 0b000010, 0b000100, 0, 0, 0b001000, 0b010000, 0b100000,
    ];
    for (plane, &want) in expected.iter().enumerate() {
        assert_eq!(out[plane_byte(plane, 3, 7)], want, "plane {plane}");
    }
}

#[test]
fn gamma_lut_applies_during_packing() {
    let mut lut = IDENTITY;
    lut[255] = 0b1010_1010;
    let mut frame = [0u8; RGB888_FRAME_BYTES];
    set_rgb888(&mut frame, 0, 0, [255, 0, 0]); // r1 -> lut[255]
    let out = pack(&frame, &lut);

    for plane in 0..8 {
        let expected = if plane % 2 == 1 { 0b000001 } else { 0 };
        assert_eq!(out[plane_byte(plane, 0, 0)], expected, "plane {plane}");
    }
}

/// The gamma tables the property tests sweep: the two shipped const tables,
/// two `Power` curves bracketing 1.0, and a LUT of pure noise — fused-table
/// construction assumes nothing about monotonicity, so nothing here should
/// either.
fn property_luts() -> Vec<(&'static str, [u8; 256])> {
    let mut noise = [0u8; 256];
    let mut state = 0x9E3779B9u32;
    for byte in noise.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    vec![
        ("identity", Gamma::Identity.build_lut()),
        ("srgb", Gamma::Srgb.build_lut()),
        ("power 2.2", Gamma::Power(2.2).build_lut()),
        ("power 0.45", Gamma::Power(0.45).build_lut()),
        ("noise", noise),
    ]
}

fn assert_matches_rgb888_reference(frame: &[u8; RGB888_FRAME_BYTES], case: &str) {
    for (lut_name, lut) in property_luts() {
        let mut reference_out = Box::new([0u8; BITPLANE_BUFFER_BYTES]);
        pack_rgb888_reference(frame, &lut, &mut reference_out);
        assert_eq!(
            pack(frame, &lut),
            reference_out,
            "fused pack diverged from the scalar RGB888 pack: {case}, gamma {lut_name}"
        );
    }
}

/// Every channel value in every lane: frame `k` gives pixel `p` the channels
/// `(p + k, p + 85 + 3k, p + 170 + 7k)` mod 256, so across the sweep each lane
/// — R, G, B, in both halves of the panel — sees all 256 values at many
/// pixel-pair positions, and no two lanes of a pair move in lockstep.
#[test]
fn fused_pack_matches_the_scalar_rgb888_pack_for_every_channel_value() {
    for k in 0..4usize {
        let mut frame = [0u8; RGB888_FRAME_BYTES];
        for (p, pixel) in frame.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            *pixel = [
                (p + k) as u8,
                (p + 85 + 3 * k) as u8,
                (p + 170 + 7 * k) as u8,
            ];
        }
        assert_matches_rgb888_reference(&frame, &format!("value sweep {k}"));
    }
}

/// Byte-identical to the scalar pack over unstructured noise frames, where
/// adjacent bytes share no structure the value sweep might mask.
#[test]
fn fused_pack_matches_the_scalar_rgb888_pack_on_random_frames() {
    let mut state = 0xDEADBEEFu32;
    for case in 0..6 {
        let mut frame = [0u8; RGB888_FRAME_BYTES];
        for byte in frame.iter_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        assert_matches_rgb888_reference(&frame, &format!("xorshift frame {case}"));
    }
}

/// An RGB565 frame widened the way the panel always widened it: each field's
/// top bits replicated into the low ones.
fn bit_replicated(rgb565: &[u8; RGB565_FRAME_BYTES]) -> Box<[u8; RGB888_FRAME_BYTES]> {
    let mut rgb888 = Box::new([0u8; RGB888_FRAME_BYTES]);
    for (pixel, out) in rgb565
        .as_chunks::<2>()
        .0
        .iter()
        .zip(rgb888.as_chunks_mut::<3>().0)
    {
        let value = u16::from_le_bytes(*pixel);
        let (r, g, b) = (value >> 11, (value >> 5) & 0x3F, value & 0x1F);
        *out = [
            ((r << 3) | (r >> 2)) as u8,
            ((g << 2) | (g >> 4)) as u8,
            ((b << 3) | (b >> 2)) as u8,
        ];
    }
    rgb888
}

/// Every one of the 65,536 RGB565 values, bit-replicated, lights exactly the
/// LEDs the scalar RGB565 pack lit — under every gamma table. A frame holds
/// 8,192 pixels, so eight frames enumerate them all, each visiting every
/// pixel-pair position and both halves of the panel. This is the pin that
/// keeps a 565 crest, and every colour the old surface could hold, unchanged
/// on the panel across the move to an eight-bit surface.
#[test]
fn fused_pack_of_bit_replicated_rgb565_matches_the_scalar_rgb565_pack() {
    for chunk in 0..8u32 {
        let mut frame = [0u8; RGB565_FRAME_BYTES];
        for (pixel, out) in frame.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            *out = ((chunk * 8192 + pixel as u32) as u16).to_le_bytes();
        }
        let widened = bit_replicated(&frame);
        for (lut_name, lut) in property_luts() {
            let mut reference_out = Box::new([0u8; BITPLANE_BUFFER_BYTES]);
            pack_rgb565_reference(&frame, &lut, &mut reference_out);
            assert_eq!(
                pack(&widened, &lut),
                reference_out,
                "565 value sweep chunk {chunk}, gamma {lut_name}"
            );
        }
    }
}

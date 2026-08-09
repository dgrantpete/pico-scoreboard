//! The scalar RGB565 pack the fused-table version replaced, kept verbatim
//! as the property-test oracle (and as the slow lane in `benches/pack.rs`).
//! This is the code that ran on silicon for the BUDGET.md frame-time
//! measurements; byte-identical output against it is the correctness bar
//! for any pack rewrite — the BCM buffer feeds running PIO/DMA hardware,
//! where a single wrong bit is a visibly wrong color at some plane weight.
//!
//! Shared by an integration test and a bench, which compile it separately;
//! either alone leaves some items unused.
#![allow(dead_code)]

use hub75::geometry::{
    BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, COLOR_BIT_DEPTH, RGB565_FRAME_BYTES,
    ROW_ADDRESS_COUNT, SHIFT_REGISTER_DEPTH,
};

/// `packing::pack_rgb565` as of the pre-FusedTables implementation.
pub fn pack_rgb565_reference(
    input: &[u8; RGB565_FRAME_BYTES],
    gamma_lut: &[u8; 256],
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    for pair in 0..ROW_ADDRESS_COUNT {
        let top = pair * SHIFT_REGISTER_DEPTH * 2;
        let bottom = (pair + ROW_ADDRESS_COUNT) * SHIFT_REGISTER_DEPTH * 2;
        for x in 0..SHIFT_REGISTER_DEPTH {
            let (r1, g1, b1) = expand_rgb565(input[top + 2 * x], input[top + 2 * x + 1]);
            let (r2, g2, b2) = expand_rgb565(input[bottom + 2 * x], input[bottom + 2 * x + 1]);
            write_pair(
                [
                    gamma_lut[r1], gamma_lut[g1], gamma_lut[b1],
                    gamma_lut[r2], gamma_lut[g2], gamma_lut[b2],
                ],
                output,
                pair * SHIFT_REGISTER_DEPTH + x,
            );
        }
    }
}

fn expand_rgb565(lo: u8, hi: u8) -> (usize, usize, usize) {
    let r = (hi & 0b1111_1000) as u32;
    let g = (((hi as u32) << 5) | ((lo as u32) >> 3)) & 0b1111_1100;
    let b = ((lo as u32) << 3) & 0b1111_1000;
    (
        (r | (r >> 5)) as usize,
        (g | (g >> 6)) as usize,
        (b | (b >> 5)) as usize,
    )
}

fn write_pair(channels: [u8; 6], output: &mut [u8; BITPLANE_BUFFER_BYTES], index: usize) {
    for plane in 0..COLOR_BIT_DEPTH {
        let mut packed = 0u8;
        for (lane, &value) in channels.iter().enumerate() {
            packed |= ((value >> plane) & 1) << lane;
        }
        output[plane * BITPLANE_BYTES + index] = packed;
    }
}

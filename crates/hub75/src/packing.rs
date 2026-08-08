//! RGB565/RGB888 → BCM bitplane conversion (port of the native
//! `bitplanes.c` kernels, identity row map — the production panel's
//! shift-register order matches the logical top-half/bottom-half layout).
//!
//! Output layout, dictated by the DMA/PIO consumption order: plane-major
//! (LSB plane first, so plane *i* lines up with the timing stream's
//! `base_cycles << i` weighting), then row pairs, then columns. Each byte
//! packs one pixel pair as `(R1, G1, B1, R2, G2, B2)` in bits 0..=5 — the
//! six data pins, lowest pin = bit 0. The data SM consumes 8 bits per pixel
//! (`out pins, 8`); the top two bits are padding.

use crate::geometry::{
    BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, COLOR_BIT_DEPTH, RGB565_FRAME_BYTES,
    RGB888_FRAME_BYTES, ROW_ADDRESS_COUNT, SHIFT_REGISTER_DEPTH,
};

/// Convert an RGB565 frame (little-endian, `framebuf.RGB565` layout: low
/// byte `GGGBBBBB`, high byte `RRRRRGGG`) into bitplanes.
pub fn pack_rgb565(
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

/// Convert an RGB888 frame (three bytes per pixel: R, G, B) into bitplanes.
pub fn pack_rgb888(
    input: &[u8; RGB888_FRAME_BYTES],
    gamma_lut: &[u8; 256],
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    for pair in 0..ROW_ADDRESS_COUNT {
        let top = pair * SHIFT_REGISTER_DEPTH * 3;
        let bottom = (pair + ROW_ADDRESS_COUNT) * SHIFT_REGISTER_DEPTH * 3;
        for x in 0..SHIFT_REGISTER_DEPTH {
            let t = top + 3 * x;
            let b = bottom + 3 * x;
            write_pair(
                [
                    gamma_lut[input[t] as usize],
                    gamma_lut[input[t + 1] as usize],
                    gamma_lut[input[t + 2] as usize],
                    gamma_lut[input[b] as usize],
                    gamma_lut[input[b + 1] as usize],
                    gamma_lut[input[b + 2] as usize],
                ],
                output,
                pair * SHIFT_REGISTER_DEPTH + x,
            );
        }
    }
}

/// Expand RGB565 channels to 8 bits, replicating the MSBs into the empty
/// LSBs so full-scale reaches 255 (at the cost of slight nonlinearity),
/// exactly as `bitplanes.c` does. Returned as LUT indices.
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

/// Scatter one gamma-corrected pixel pair (`[r1, g1, b1, r2, g2, b2]`)
/// across all bitplanes at byte `index` within each plane.
fn write_pair(channels: [u8; 6], output: &mut [u8; BITPLANE_BUFFER_BYTES], index: usize) {
    for plane in 0..COLOR_BIT_DEPTH {
        let mut packed = 0u8;
        for (lane, &value) in channels.iter().enumerate() {
            packed |= ((value >> plane) & 1) << lane;
        }
        output[plane * BITPLANE_BYTES + index] = packed;
    }
}

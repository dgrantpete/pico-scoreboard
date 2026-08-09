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
//!
//! The RGB565 path goes through [`FusedTables`], which folds the channel
//! expansion, the gamma LUT, and the spread of the eight plane bits into one
//! table lookup per channel (BACKLOG 63). The RGB888 path keeps the scalar
//! per-plane pluck: nothing on the render path loads RGB888, so it is not
//! worth tables of its own. `tests/packing.rs` pins the RGB565 path
//! byte-identical to the pre-table implementation it keeps as an oracle.

use crate::geometry::{
    BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, COLOR_BIT_DEPTH, RGB565_FRAME_BYTES,
    RGB888_FRAME_BYTES, ROW_ADDRESS_COUNT, SHIFT_REGISTER_DEPTH,
};

/// Fused gamma + bit-spread lookup tables for the RGB565 pack: one `u64`
/// per raw channel value whose LE byte *i* carries that channel's
/// post-gamma plane-*i* bit, already positioned at the channel's top-half
/// data-pin lane (R1, G1, B1 = bits 0, 1, 2). ORing three entries yields
/// all eight output bytes of a top-half pixel at once; the bottom-half
/// pixel uses the same tables and one `<< 3`, because the bottom lanes are
/// the top lanes shifted by three.
///
/// Indices are the *raw* framebuffer channels — 5-bit R/B, 6-bit G — with
/// the MSB-replicating 8-bit expansion folded into construction, so the
/// pack touches neither the expansion nor the 256-entry gamma LUT.
///
/// Three tables plus a shift rather than six pre-shifted tables: on the
/// host the two are within noise of each other, and on thumbv8m the six
/// bases (twelve, once each `u64` load splits into a register-offset word
/// pair) spill — 86.5 instructions per pixel pair against 78 for this
/// shape under the app's actual codegen. The asm comparison lives with the
/// decision record in `benches/pack.rs`; a paired-channel 17 KiB variant
/// measured only 5 % better and was rejected outright. 1,024 B total,
/// living wherever the driver does (the core-1 task arena on the device).
///
/// Derived from the active gamma LUT, so it must be rebuilt whenever that
/// changes — [`Hub75Driver::set_gamma`] does, in microseconds (128 entries
/// of shift-and-mask; nothing like the 27.6 ms `Power` LUT expansion that
/// [`GammaTable`](crate::gamma::GammaTable) exists to keep off core 1).
///
/// [`Hub75Driver::set_gamma`]: crate::driver::Hub75Driver::set_gamma
pub struct FusedTables {
    r: [u64; 32],
    g: [u64; 64],
    b: [u64; 32],
}

impl FusedTables {
    /// Build the tables for one gamma LUT.
    pub fn new(gamma_lut: &[u8; 256]) -> FusedTables {
        let mut tables = FusedTables {
            r: [0; 32],
            g: [0; 64],
            b: [0; 32],
        };
        for raw in 0..32 {
            // R and B share the 5-bit expansion, so one gamma read feeds
            // both tables; only the lane position differs.
            let expanded = (raw << 3) | (raw >> 2);
            let spread = spread_planes(gamma_lut[expanded]);
            tables.r[raw] = spread;
            tables.b[raw] = spread << 2;
        }
        for raw in 0..64 {
            let expanded = (raw << 2) | (raw >> 4);
            tables.g[raw] = spread_planes(gamma_lut[expanded]) << 1;
        }
        tables
    }
}

/// LE byte `i` of the result carries bit `i` of `value` in its bit 0: the
/// post-gamma channel byte, spread across the eight plane slots.
fn spread_planes(value: u8) -> u64 {
    let mut spread = 0u64;
    for plane in 0..COLOR_BIT_DEPTH {
        spread |= (((value >> plane) & 1) as u64) << (8 * plane);
    }
    spread
}

/// Convert an RGB565 frame (little-endian, `framebuf.RGB565` layout: low
/// byte `GGGBBBBB`, high byte `RRRRRGGG`) into bitplanes.
///
/// Per pixel pair: four byte loads, six [`FusedTables`] lookups, five ORs
/// and a shift, eight byte stores — where the scalar version did six gamma
/// lookups and then plucked and placed 48 bits one at a time.
///
/// The flat loop walks pixel pairs directly: pair row `p`, column `x` is
/// output byte `p * 128 + x` in every plane, and the matching input bytes
/// sit at exactly twice that offset into each half of the frame — the
/// top-half rows are the frame's first half, the bottom-half rows its
/// second.
///
/// Placed in RAM on the device (`.data.*` is copied out of flash by the
/// cortex-m-rt startup, and every device binary here links its script):
/// this loop runs every drawn frame and BACKLOG 63 measured its time moving
/// 5.07 → 5.25 ms between builds that differed by 120 B of unrelated code —
/// XIP cache placement, not the loop. RAM residency removes that variance
/// (and the XIP miss cost itself); `inline(never)` keeps LTO from folding
/// the body back into a flash-resident caller.
#[cfg_attr(target_os = "none", unsafe(link_section = ".data.hub75_pack"))]
#[inline(never)]
pub fn pack_rgb565(
    input: &[u8; RGB565_FRAME_BYTES],
    tables: &FusedTables,
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    let (top, bottom) = input.split_at(RGB565_FRAME_BYTES / 2);
    for index in 0..BITPLANE_BYTES {
        let lo1 = top[2 * index] as usize;
        let hi1 = top[2 * index + 1] as usize;
        let lo2 = bottom[2 * index] as usize;
        let hi2 = bottom[2 * index + 1] as usize;
        let top_word = tables.r[hi1 >> 3]
            | tables.g[((hi1 << 3) | (lo1 >> 5)) & 0b11_1111]
            | tables.b[lo1 & 0b1_1111];
        let bottom_word = tables.r[hi2 >> 3]
            | tables.g[((hi2 << 3) | (lo2 >> 5)) & 0b11_1111]
            | tables.b[lo2 & 0b1_1111];
        let word = top_word | (bottom_word << 3);
        let [p0, p1, p2, p3, p4, p5, p6, p7] = word.to_le_bytes();
        output[index] = p0;
        output[BITPLANE_BYTES + index] = p1;
        output[2 * BITPLANE_BYTES + index] = p2;
        output[3 * BITPLANE_BYTES + index] = p3;
        output[4 * BITPLANE_BYTES + index] = p4;
        output[5 * BITPLANE_BYTES + index] = p5;
        output[6 * BITPLANE_BYTES + index] = p6;
        output[7 * BITPLANE_BYTES + index] = p7;
    }
}

/// Convert an RGB888 frame (three bytes per pixel: R, G, B) into bitplanes.
///
/// Stays in flash and on the scalar path: nothing on the render path loads
/// RGB888 (the display buffer is RGB565), so it is worth neither RAM nor
/// tables.
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

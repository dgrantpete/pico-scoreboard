//! RGB888 → BCM bitplane conversion (port of the native `bitplanes.c`
//! kernels, identity row map — the production panel's shift-register order
//! matches the logical top-half/bottom-half layout).
//!
//! Output layout, dictated by the DMA/PIO consumption order: plane-major
//! (LSB plane first, so plane *i* lines up with the timing stream's
//! `base_cycles << i` weighting), then row pairs, then columns. Each byte
//! packs one pixel pair as `(R1, G1, B1, R2, G2, B2)` in bits 0..=5 — the
//! six data pins, lowest pin = bit 0. The data SM consumes 8 bits per pixel
//! (`out pins, 8`); the top two bits are padding.
//!
//! The pack goes through [`FusedTables`], which folds the gamma LUT and the
//! spread of the eight plane bits into one table lookup per channel — BACKLOG
//! 63's technique, re-derived for eight-bit channels when the drawing surface
//! left RGB565 (BACKLOG 115).
//! `tests/packing.rs` pins it byte-identical to the scalar implementation it
//! keeps as an oracle, and to the scalar RGB565 pack that ran on silicon for
//! every frame before that, over bit-replicated 565 input.

use crate::geometry::{BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, COLOR_BIT_DEPTH, RGB888_FRAME_BYTES};

/// Fused gamma + bit-spread lookup table: for every 8-bit channel value, that
/// value's eight post-gamma plane bits, interleaved so that byte `i` of the
/// entry carries plane `i` in bit 0 and plane `i + 4` in bit 4.
///
/// A pixel's word is three lookups — R, then G shifted one, then B shifted
/// two — which leaves each byte holding that pixel's R/G/B lanes for plane
/// `i` in bits 0–2 and for plane `i + 4` in bits 4–6. Its pair-mate in the
/// bottom half builds the same word. The eight output bytes then fall out of
/// two masks and two shifts:
///
/// ```text
/// planes 0–3 = (top        & 0x07070707) | ((bottom << 3) & 0x38383838)
/// planes 4–7 = ((top >> 4) & 0x07070707) | ((bottom >> 1) & 0x38383838)
/// ```
///
/// One table serves every lane because the lanes differ only by a shift that
/// never carries out of its nibble, and every shift folds into the `orr` or
/// `and` that consumes it on thumbv8m. 1,024 B — the size of the RGB565
/// pack's three tables it replaced — living wherever the driver does (the
/// core-1 task arena on the device). The shapes it was chosen over, and their
/// instruction counts, are the decision record in `benches/pack.rs`.
///
/// Derived from the active gamma LUT, so it must be rebuilt whenever that
/// changes — [`Hub75Driver::set_gamma`] does, in microseconds (256 entries of
/// shift-and-mask; nothing like the 27.6 ms `Power` LUT expansion that
/// [`GammaTable`](crate::gamma::GammaTable) exists to keep off core 1).
///
/// [`Hub75Driver::set_gamma`]: crate::driver::Hub75Driver::set_gamma
pub struct FusedTables {
    planes: [u32; 256],
}

impl FusedTables {
    /// Build the table for one gamma LUT.
    pub fn new(gamma_lut: &[u8; 256]) -> FusedTables {
        let mut tables = FusedTables { planes: [0; 256] };
        for (entry, &corrected) in tables.planes.iter_mut().zip(gamma_lut) {
            *entry = interleave_planes(corrected);
        }
        tables
    }
}

/// Bit `p` of `value` at bit `8 * (p % 4) + 4 * (p / 4)`: planes 0–3 in the
/// low nibble of bytes 0–3, planes 4–7 in their high nibble.
fn interleave_planes(value: u8) -> u32 {
    let mut entry = 0u32;
    for plane in 0..COLOR_BIT_DEPTH {
        let bit = ((value >> plane) & 1) as u32;
        entry |= bit << (8 * (plane % 4) + 4 * (plane / 4));
    }
    entry
}

/// Lanes R1, G1, B1 of planes `i` and `i + 4`, in byte `i`'s two nibbles.
const LANES_LOW: u32 = 0x0707_0707;
/// Lanes R2, G2, B2 once a bottom-half word is shifted onto bits 3–5.
const LANES_HIGH: u32 = 0x3838_3838;

/// Convert an RGB888 frame (three bytes per pixel: R, G, B, row-major) into
/// bitplanes.
///
/// Per pixel pair: six byte loads, six [`FusedTables`] lookups, four ORs, the
/// four masks and two ORs that split the pair into its two plane quads, and
/// eight byte stores — about 47 instructions on thumbv8m against the RGB565
/// pack's 78.
///
/// The flat loop walks pixel pairs directly: pair row `p`, column `x` is
/// output byte `p * 128 + x` in every plane, and the matching input pixel
/// sits at exactly that pixel offset into each half of the frame — the
/// top-half rows are the frame's first half, the bottom-half rows its
/// second.
///
/// Placed in RAM on the device (`.data.*` is copied out of flash by the
/// cortex-m-rt startup, and every device binary here links its script):
/// this loop runs every drawn frame and BACKLOG 63 measured its RGB565
/// predecessor moving 5.07 → 5.25 ms between builds that differed by 120 B
/// of unrelated code — XIP cache placement, not the loop. RAM residency
/// removes that variance (and the XIP miss cost itself); `inline(never)`
/// keeps LTO from folding the body back into a flash-resident caller.
#[cfg_attr(target_os = "none", unsafe(link_section = ".data.hub75_pack"))]
#[inline(never)]
pub fn pack_rgb888(
    input: &[u8; RGB888_FRAME_BYTES],
    tables: &FusedTables,
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    let (top, bottom) = input.split_at(RGB888_FRAME_BYTES / 2);
    let (top, bottom) = (top.as_chunks::<3>().0, bottom.as_chunks::<3>().0);
    for (index, (&[r1, g1, b1], &[r2, g2, b2])) in top.iter().zip(bottom).enumerate() {
        let table = &tables.planes;
        let upper = table[r1 as usize] | (table[g1 as usize] << 1) | (table[b1 as usize] << 2);
        let lower = table[r2 as usize] | (table[g2 as usize] << 1) | (table[b2 as usize] << 2);
        let low = (upper & LANES_LOW) | ((lower << 3) & LANES_HIGH);
        let high = ((upper >> 4) & LANES_LOW) | ((lower >> 1) & LANES_HIGH);
        let [p0, p1, p2, p3] = low.to_le_bytes();
        let [p4, p5, p6, p7] = high.to_le_bytes();
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

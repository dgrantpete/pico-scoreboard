//! Host timing harness for the RGB888 → bitplane pack (BACKLOG 63, and
//! BACKLOG 115's move to an eight-bit drawing surface).
//!
//! A plain timed loop, no criterion: the pack is a pure function and the
//! question each variant answers is "did this change make it faster, and
//! by roughly what factor" — a ratio, not an absolute. Host ratios are
//! indicative only, and for this workload actively misleading (see below);
//! the authoritative number is the on-device frame probe's `show` lap
//! (`firmware-rs/BUDGET.md`, "Core 1: measured frame times").
//!
//! # The decision record
//!
//! ## RGB565, task #20 (2026-08-09)
//!
//! Three tables (32/64/32 `u64` entries, 1 KiB) plus a `<< 3` for the bottom
//! pixel, byte stores: 78 loop instructions per pixel pair on thumbv8m against
//! ~165 for the scalar pack, measured on device at 2,241 µs a frame (from
//! 5,250). Six pre-shifted tables (86.5), wider stores (91–94.5) and a 17 KiB
//! paired-channel table (73.5, rejected for its RAM) lost. Two host-vs-device
//! inversions came out of that work, both register-pressure artifacts that
//! x86-64's sixteen renamed registers hide: six pre-shifted tables beat
//! three-plus-shift on the host but need twelve reg-offset bases on ARM
//! (every `u64` load splits into a word pair) and spill; a u32 quad-store
//! transpose was 1.5× on the host and lost on ARM. That is the benching trap:
//! **read the asm, not this harness, before choosing a pack shape.**
//!
//! ## RGB888, BACKLOG 115 (2026-11-08)
//!
//! When the drawing surface went to eight bits a channel the RGB565 pack was
//! deleted and the RGB888 one — until then a scalar per-plane pluck, since
//! nothing on the render path loaded RGB888 — needed the same treatment.
//! Loop-body instructions per pixel pair (rustc 1.99.0, `-C opt-level=3 -C
//! codegen-units=1`, generic thumbv8m.main like the app; spills = stack
//! traffic inside the loop; every shape is byte-identical):
//!
//! | variant                                   | table | instr/pair | spill ld+st |
//! |-------------------------------------------|------:|-----------:|------------:|
//! | RGB565 three-table + shift (the old pack) | 1 KiB |       78.5 |           9 |
//! | one `u64` table, `u64` lane shifts        | 2 KiB |       72.5 |           7 |
//! | three `u64` tables + `<< 3`, the 565 shape | 6 KiB |       66.5 |         6.5 |
//! | nibble-per-plane `u32`, spread to bytes   | 1 KiB |         66 |         3.5 |
//! | two `u32` halves (planes 0–3, 4–7)        | 2 KiB |         64 |          12 |
//! | **interleaved nibbles, two masks (ships)** | 1 KiB |   **47.5** |           4 |
//!
//! A `u64` table loses on ARM for the reason the 565 record found: its lane
//! shifts cannot fold into the `orr` (the compiler cannot know no bit crosses
//! the word boundary), so each costs three instructions. Splitting it into
//! two `u32` halves folds the shifts but doubles the lookups. The shipped
//! table packs planes `i` and `i + 4` into the two nibbles of byte `i`, so a
//! pixel is three lookups, and the pair splits into its two plane quads with
//! two masks and two shifts that all fold into their `and`/`orr` — fewer
//! instructions than any shape here, in the RGB565 pack's 1 KiB.
//!
//! Run with `cargo bench -p hub75`. Each variant reports the mean
//! microseconds per frame of its best round — best-of-rounds discards
//! scheduler noise, which on a desktop OS is one-sided. Every variant is
//! checked byte-identical to the shipped pack before anything is timed (the
//! RGB565 one against a bit-replicated copy of its own frame).

#[path = "../tests/reference/mod.rs"]
mod reference;

use std::hint::black_box;
use std::time::Instant;

use hub75::gamma::Gamma;
use hub75::geometry::{BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, RGB888_FRAME_BYTES};
use hub75::packing::{FusedTables, pack_rgb888};
use reference::RGB565_FRAME_BYTES;

const WARMUP_ITERS: usize = 100;
const ROUNDS: usize = 10;
const ITERS_PER_ROUND: usize = 300;

fn spread_planes(value: u8) -> u64 {
    let mut spread = 0u64;
    for plane in 0..8 {
        spread |= (((value >> plane) & 1) as u64) << (8 * plane);
    }
    spread
}

/// The RGB565 pack's tables, as it shipped from task #20 until BACKLOG 115.
struct Rgb565Tables {
    r: [u64; 32],
    g: [u64; 64],
    b: [u64; 32],
}

fn rgb565_tables(gamma_lut: &[u8; 256]) -> Rgb565Tables {
    let mut tables = Rgb565Tables {
        r: [0; 32],
        g: [0; 64],
        b: [0; 32],
    };
    for raw in 0..32 {
        let spread = spread_planes(gamma_lut[(raw << 3) | (raw >> 2)]);
        tables.r[raw] = spread;
        tables.b[raw] = spread << 2;
    }
    for raw in 0..64 {
        tables.g[raw] = spread_planes(gamma_lut[(raw << 2) | (raw >> 4)]) << 1;
    }
    tables
}

/// The RGB565 pack, as it shipped: the number the RGB888 pack has to beat.
fn pack_rgb565(
    input: &[u8; RGB565_FRAME_BYTES],
    tables: &Rgb565Tables,
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
        for (plane, byte) in word.to_le_bytes().into_iter().enumerate() {
            output[plane * BITPLANE_BYTES + index] = byte;
        }
    }
}

/// The obvious RGB888 analogue of the RGB565 shape: one table per channel,
/// lane shift pre-applied, bottom pixel `<< 3` (6 KiB).
struct ThreeTables {
    r: [u64; 256],
    g: [u64; 256],
    b: [u64; 256],
}

fn three_tables(gamma_lut: &[u8; 256]) -> ThreeTables {
    let mut tables = ThreeTables {
        r: [0; 256],
        g: [0; 256],
        b: [0; 256],
    };
    for (value, &corrected) in gamma_lut.iter().enumerate() {
        let spread = spread_planes(corrected);
        tables.r[value] = spread;
        tables.g[value] = spread << 1;
        tables.b[value] = spread << 2;
    }
    tables
}

fn pack_three_tables(
    input: &[u8; RGB888_FRAME_BYTES],
    tables: &ThreeTables,
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    let (top, bottom) = input.split_at(RGB888_FRAME_BYTES / 2);
    let (top, bottom) = (top.as_chunks::<3>().0, bottom.as_chunks::<3>().0);
    for (index, (&[r1, g1, b1], &[r2, g2, b2])) in top.iter().zip(bottom).enumerate() {
        let upper = tables.r[r1 as usize] | tables.g[g1 as usize] | tables.b[b1 as usize];
        let lower = tables.r[r2 as usize] | tables.g[g2 as usize] | tables.b[b2 as usize];
        let word = upper | (lower << 3);
        for (plane, byte) in word.to_le_bytes().into_iter().enumerate() {
            output[plane * BITPLANE_BYTES + index] = byte;
        }
    }
}

/// Deterministic full-coverage noise, same generator as the packing tests.
fn xorshift_bytes<const N: usize>(mut state: u32) -> Box<[u8; N]> {
    let mut frame = vec![0u8; N].into_boxed_slice();
    for byte in frame.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    frame.try_into().unwrap()
}

/// `frame` widened by bit replication, the panel's own 565 expansion.
fn bit_replicated(frame: &[u8; RGB565_FRAME_BYTES]) -> Box<[u8; RGB888_FRAME_BYTES]> {
    let mut wide: Box<[u8; RGB888_FRAME_BYTES]> = vec![0u8; RGB888_FRAME_BYTES]
        .into_boxed_slice()
        .try_into()
        .unwrap();
    for (pixel, out) in frame
        .as_chunks::<2>()
        .0
        .iter()
        .zip(wide.as_chunks_mut::<3>().0)
    {
        let value = u16::from_le_bytes(*pixel);
        let (r, g, b) = (value >> 11, (value >> 5) & 0x3F, value & 0x1F);
        *out = [
            ((r << 3) | (r >> 2)) as u8,
            ((g << 2) | (g >> 4)) as u8,
            ((b << 3) | (b >> 2)) as u8,
        ];
    }
    wide
}

/// Best-round mean, in µs per iteration.
fn time_variant(name: &str, baseline_us: Option<f64>, mut iteration: impl FnMut()) -> f64 {
    for _ in 0..WARMUP_ITERS {
        iteration();
    }
    let mut best_us = f64::INFINITY;
    for _ in 0..ROUNDS {
        let started = Instant::now();
        for _ in 0..ITERS_PER_ROUND {
            iteration();
        }
        let round_us = started.elapsed().as_secs_f64() * 1e6 / ITERS_PER_ROUND as f64;
        best_us = best_us.min(round_us);
    }
    match baseline_us {
        Some(baseline) => {
            println!(
                "{name:<42} {best_us:8.2} µs/frame  ({:.2}x)",
                baseline / best_us
            );
        }
        None => println!("{name:<42} {best_us:8.2} µs/frame  (baseline)"),
    }
    best_us
}

fn main() {
    let frame = xorshift_bytes::<RGB888_FRAME_BYTES>(0x12345678);
    let frame_565 = xorshift_bytes::<RGB565_FRAME_BYTES>(0x12345678);
    let lut = Gamma::Srgb.build_lut();
    let fused = FusedTables::new(&lut);
    let three = three_tables(&lut);
    let tables_565 = rgb565_tables(&lut);
    let mut output = vec![0u8; BITPLANE_BUFFER_BYTES].into_boxed_slice();
    let output: &mut [u8; BITPLANE_BUFFER_BYTES] = (&mut *output).try_into().unwrap();

    let mut expected = vec![0u8; BITPLANE_BUFFER_BYTES].into_boxed_slice();
    let expected: &mut [u8; BITPLANE_BUFFER_BYTES] = (&mut *expected).try_into().unwrap();
    pack_rgb888(&frame, &fused, expected);
    reference::pack_rgb888_reference(&frame, &lut, output);
    assert_eq!(
        &*output, &*expected,
        "scalar reference disagrees with shipped pack"
    );
    pack_three_tables(&frame, &three, output);
    assert_eq!(
        &*output, &*expected,
        "three-table variant disagrees with shipped pack"
    );
    pack_rgb888(&bit_replicated(&frame_565), &fused, expected);
    pack_rgb565(&frame_565, &tables_565, output);
    assert_eq!(
        &*output, &*expected,
        "RGB565 pack disagrees with its widened frame"
    );

    println!(
        "pack bench: {} px/frame, {} rounds x {} iters, best-round mean",
        RGB888_FRAME_BYTES / 3,
        ROUNDS,
        ITERS_PER_ROUND
    );

    let baseline = time_variant("scalar RGB888 (pre-BACKLOG-115)", None, || {
        reference::pack_rgb888_reference(black_box(&frame), black_box(&lut), black_box(output));
    });
    time_variant(
        "fused RGB565 (shipped until BACKLOG 115)",
        Some(baseline),
        || {
            pack_rgb565(
                black_box(&frame_565),
                black_box(&tables_565),
                black_box(output),
            );
        },
    );
    time_variant("fused three u64 tables (6 KiB)", Some(baseline), || {
        pack_three_tables(black_box(&frame), black_box(&three), black_box(output));
    });
    time_variant(
        "fused interleaved nibbles (shipped)",
        Some(baseline),
        || {
            pack_rgb888(black_box(&frame), black_box(&fused), black_box(output));
        },
    );

    // The table rebuild that rides along with every set_gamma: confirm the
    // "microseconds" claim in FusedTables' docs stays true.
    time_variant("FusedTables::new (set_gamma cost)", None, || {
        black_box(FusedTables::new(black_box(&lut)));
    });
}

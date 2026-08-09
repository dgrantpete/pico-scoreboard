//! Host timing harness for the RGB565 → bitplane pack (BACKLOG 63).
//!
//! A plain timed loop, no criterion: the pack is a pure function and the
//! question each variant answers is "did this change make it faster, and
//! by roughly what factor" — a ratio, not an absolute. Host ratios are
//! indicative only, and for this workload actively misleading in one spot
//! (see below); the authoritative number is the on-device frame probe's
//! `show` lap (`firmware-rs/BUDGET.md`, "Core 1: measured frame times").
//!
//! # The decision record (task #20)
//!
//! The shipped shape — three tables plus a `<< 3` for the bottom pixel,
//! byte stores — was chosen against thumbv8m codegen, not against this
//! bench. Loop-body instructions per pixel pair (rustc 1.97.1, `-C
//! opt-level=3 -C codegen-units=1`, generic thumbv8m.main like the app;
//! spills = stack traffic inside the loop):
//!
//! | variant                     | instr/pair | of which spill ld+st |
//! |-----------------------------|-----------:|---------------------:|
//! | scalar reference            |       ~165 |                  ~7  |
//! | three-table + shift (ships) |         78 |                 8.5  |
//! | six-table pre-shifted       |       86.5 |                19.5  |
//! | six-table, u16 stores       |         91 |                  26  |
//! | six-table, u32 quad stores  |       94.5 |                  31  |
//! | paired-channel 17 KiB       |       73.5 |                  15  |
//!
//! Two host-vs-device inversions, both register-pressure artifacts that
//! x86-64's sixteen renamed registers hide: six pre-shifted tables beat
//! three-plus-shift on the host but need twelve reg-offset bases on ARM
//! (every `u64` load splits into a word pair) and spill; the u32
//! quad-store transpose is 1.5× on the host but keeps four `u64`s live on
//! ARM and loses. Tier 3 (wide stores) is therefore *declined*: the
//! host bench proves stores dominate the host, and the asm shows the
//! transpose that exploits it costs ARM more than the byte stores it
//! saves. The paired-channel table hits 73.5 but pays 17 KiB of RAM for
//! 5 % — rejected. Scalar-reference calibration: ~165 instr/pair against
//! the measured 192 cycles/pair (5.25 ms/frame at 150 MHz).
//!
//! Run with `cargo bench -p hub75`. Each variant reports the mean
//! microseconds per frame of its best round — best-of-rounds discards
//! scheduler noise, which on a desktop OS is one-sided. Every variant is
//! checked byte-identical to the shipped pack before anything is timed.

#[path = "../tests/reference/mod.rs"]
mod reference;

use std::hint::black_box;
use std::time::Instant;

use hub75::gamma::Gamma;
use hub75::geometry::{BITPLANE_BUFFER_BYTES, BITPLANE_BYTES, RGB565_FRAME_BYTES};
use hub75::packing::{FusedTables, pack_rgb565};

const WARMUP_ITERS: usize = 100;
const ROUNDS: usize = 10;
const ITERS_PER_ROUND: usize = 300;

/// The six-table layout the shipped three-table shape was chosen over:
/// bottom-half lanes pre-shifted into their own tables.
struct SixTables {
    r1: [u64; 32],
    g1: [u64; 64],
    b1: [u64; 32],
    r2: [u64; 32],
    g2: [u64; 64],
    b2: [u64; 32],
}

fn spread_planes(value: u8) -> u64 {
    let mut spread = 0u64;
    for plane in 0..8 {
        spread |= (((value >> plane) & 1) as u64) << (8 * plane);
    }
    spread
}

fn six_tables(gamma_lut: &[u8; 256]) -> SixTables {
    let mut tables = SixTables {
        r1: [0; 32],
        g1: [0; 64],
        b1: [0; 32],
        r2: [0; 32],
        g2: [0; 64],
        b2: [0; 32],
    };
    for raw in 0..32 {
        let expanded = (raw << 3) | (raw >> 2);
        let spread = spread_planes(gamma_lut[expanded]);
        tables.r1[raw] = spread;
        tables.b1[raw] = spread << 2;
        tables.r2[raw] = spread << 3;
        tables.b2[raw] = spread << 5;
    }
    for raw in 0..64 {
        let expanded = (raw << 2) | (raw >> 4);
        let spread = spread_planes(gamma_lut[expanded]);
        tables.g1[raw] = spread << 1;
        tables.g2[raw] = spread << 4;
    }
    tables
}

/// Six pre-shifted tables, byte stores.
fn pack_six_tables(
    input: &[u8; RGB565_FRAME_BYTES],
    tables: &SixTables,
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    let (top, bottom) = input.split_at(RGB565_FRAME_BYTES / 2);
    for index in 0..BITPLANE_BYTES {
        let lo1 = top[2 * index] as usize;
        let hi1 = top[2 * index + 1] as usize;
        let lo2 = bottom[2 * index] as usize;
        let hi2 = bottom[2 * index + 1] as usize;
        let word = tables.r1[hi1 >> 3]
            | tables.g1[((hi1 << 3) | (lo1 >> 5)) & 0b11_1111]
            | tables.b1[lo1 & 0b1_1111]
            | tables.r2[hi2 >> 3]
            | tables.g2[((hi2 << 3) | (lo2 >> 5)) & 0b11_1111]
            | tables.b2[lo2 & 0b1_1111];
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

/// Declined tier 3: u32 accumulation across four columns per plane —
/// 8 stores per 4 pixel pairs instead of 32.
fn pack_u32_stores(
    input: &[u8; RGB565_FRAME_BYTES],
    tables: &SixTables,
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    let (top, bottom) = input.split_at(RGB565_FRAME_BYTES / 2);
    let mut index = 0;
    while index < BITPLANE_BYTES {
        let mut words = [0u64; 4];
        for (k, word) in words.iter_mut().enumerate() {
            let i = index + k;
            let lo1 = top[2 * i] as usize;
            let hi1 = top[2 * i + 1] as usize;
            let lo2 = bottom[2 * i] as usize;
            let hi2 = bottom[2 * i + 1] as usize;
            *word = tables.r1[hi1 >> 3]
                | tables.g1[((hi1 << 3) | (lo1 >> 5)) & 0b11_1111]
                | tables.b1[lo1 & 0b1_1111]
                | tables.r2[hi2 >> 3]
                | tables.g2[((hi2 << 3) | (lo2 >> 5)) & 0b11_1111]
                | tables.b2[lo2 & 0b1_1111];
        }
        for plane in 0..8 {
            let quad = ((words[0] >> (8 * plane)) as u32 & 0xFF)
                | (((words[1] >> (8 * plane)) as u32 & 0xFF) << 8)
                | (((words[2] >> (8 * plane)) as u32 & 0xFF) << 16)
                | (((words[3] >> (8 * plane)) as u32 & 0xFF) << 24);
            let at = plane * BITPLANE_BYTES + index;
            output[at..at + 4].copy_from_slice(&quad.to_le_bytes());
        }
        index += 4;
    }
}

/// Declined tier 3, milder form: u16 stores over pixel-pair pairs — half
/// the memory ops of byte stores at half the live registers of the u32
/// transpose.
fn pack_u16_stores(
    input: &[u8; RGB565_FRAME_BYTES],
    tables: &SixTables,
    output: &mut [u8; BITPLANE_BUFFER_BYTES],
) {
    let (top, bottom) = input.split_at(RGB565_FRAME_BYTES / 2);
    let mut index = 0;
    while index < BITPLANE_BYTES {
        let mut words = [0u64; 2];
        for (k, word) in words.iter_mut().enumerate() {
            let i = index + k;
            let lo1 = top[2 * i] as usize;
            let hi1 = top[2 * i + 1] as usize;
            let lo2 = bottom[2 * i] as usize;
            let hi2 = bottom[2 * i + 1] as usize;
            *word = tables.r1[hi1 >> 3]
                | tables.g1[((hi1 << 3) | (lo1 >> 5)) & 0b11_1111]
                | tables.b1[lo1 & 0b1_1111]
                | tables.r2[hi2 >> 3]
                | tables.g2[((hi2 << 3) | (lo2 >> 5)) & 0b11_1111]
                | tables.b2[lo2 & 0b1_1111];
        }
        for plane in 0..8 {
            let pair = ((words[0] >> (8 * plane)) as u16 & 0xFF)
                | (((words[1] >> (8 * plane)) as u16 & 0xFF) << 8);
            let at = plane * BITPLANE_BYTES + index;
            output[at..at + 2].copy_from_slice(&pair.to_le_bytes());
        }
        index += 2;
    }
}

/// Deterministic full-coverage noise, same generator as the packing tests.
fn xorshift_frame(mut state: u32) -> Box<[u8; RGB565_FRAME_BYTES]> {
    let mut frame = vec![0u8; RGB565_FRAME_BYTES].into_boxed_slice();
    for byte in frame.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    frame.try_into().unwrap()
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
            println!("{name:<42} {best_us:8.2} µs/frame  ({:.2}x)", baseline / best_us);
        }
        None => println!("{name:<42} {best_us:8.2} µs/frame  (baseline)"),
    }
    best_us
}

fn main() {
    let frame = xorshift_frame(0x12345678);
    let lut = Gamma::Srgb.build_lut();
    let fused = FusedTables::new(&lut);
    let six = six_tables(&lut);
    let mut output = vec![0u8; BITPLANE_BUFFER_BYTES].into_boxed_slice();
    let output: &mut [u8; BITPLANE_BUFFER_BYTES] = (&mut *output).try_into().unwrap();

    let mut expected = vec![0u8; BITPLANE_BUFFER_BYTES].into_boxed_slice();
    let expected: &mut [u8; BITPLANE_BUFFER_BYTES] = (&mut *expected).try_into().unwrap();
    pack_rgb565(&frame, &fused, expected);
    reference::pack_rgb565_reference(&frame, &lut, output);
    assert_eq!(&*output, &*expected, "reference disagrees with shipped pack");
    pack_six_tables(&frame, &six, output);
    assert_eq!(&*output, &*expected, "six-table variant disagrees with shipped pack");
    pack_u32_stores(&frame, &six, output);
    assert_eq!(&*output, &*expected, "u32-store variant disagrees with shipped pack");
    pack_u16_stores(&frame, &six, output);
    assert_eq!(&*output, &*expected, "u16-store variant disagrees with shipped pack");

    println!(
        "pack bench: {} px/frame, {} rounds x {} iters, best-round mean",
        RGB565_FRAME_BYTES / 2,
        ROUNDS,
        ITERS_PER_ROUND
    );

    let baseline = time_variant("scalar reference (pre-BACKLOG-63)", None, || {
        reference::pack_rgb565_reference(black_box(&frame), black_box(&lut), black_box(output));
    });
    time_variant("fused three-table + shift (shipped)", Some(baseline), || {
        pack_rgb565(black_box(&frame), black_box(&fused), black_box(output));
    });
    time_variant("fused six-table pre-shifted", Some(baseline), || {
        pack_six_tables(black_box(&frame), black_box(&six), black_box(output));
    });
    time_variant("fused six-table, u32 stores (declined)", Some(baseline), || {
        pack_u32_stores(black_box(&frame), black_box(&six), black_box(output));
    });
    time_variant("fused six-table, u16 stores (declined)", Some(baseline), || {
        pack_u16_stores(black_box(&frame), black_box(&six), black_box(output));
    });

    // The table rebuild that rides along with every set_gamma: confirm the
    // "microseconds" claim in FusedTables' docs stays true.
    time_variant("FusedTables::new (set_gamma cost)", None, || {
        black_box(FusedTables::new(black_box(&lut)));
    });
}

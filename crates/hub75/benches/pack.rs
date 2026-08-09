//! Host timing harness for the RGB565 → bitplane pack (BACKLOG 63).
//!
//! A plain timed loop, no criterion: the pack is a pure function and the
//! question each tier answers is "did this change make it faster, and by
//! roughly what factor" — a ratio, not an absolute. Host ratios are
//! indicative only (x86-64 has native u64 ops and a data cache; the
//! RP2350 has neither); the authoritative number is the on-device frame
//! probe's `show` lap (`firmware-rs/BUDGET.md`, "Core 1: measured frame
//! times").
//!
//! Run with `cargo bench -p hub75`. Each variant reports the mean
//! microseconds per frame of its best round — best-of-rounds discards
//! scheduler noise, which on a desktop OS is one-sided.

use std::hint::black_box;
use std::time::Instant;

use hub75::gamma::Gamma;
use hub75::geometry::{BITPLANE_BUFFER_BYTES, RGB565_FRAME_BYTES};
use hub75::packing::pack_rgb565;

const WARMUP_ITERS: usize = 100;
const ROUNDS: usize = 10;
const ITERS_PER_ROUND: usize = 300;

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
        Some(baseline) => println!("{name:<40} {best_us:8.2} µs/frame  ({:.2}x)", baseline / best_us),
        None => println!("{name:<40} {best_us:8.2} µs/frame  (baseline)"),
    }
    best_us
}

fn main() {
    let frame = xorshift_frame(0x12345678);
    let lut = Gamma::Srgb.build_lut();
    let mut output = vec![0u8; BITPLANE_BUFFER_BYTES].into_boxed_slice();
    let output: &mut [u8; BITPLANE_BUFFER_BYTES] = (&mut *output).try_into().unwrap();

    println!(
        "pack bench: {} px/frame, {} rounds x {} iters, best-round mean",
        RGB565_FRAME_BYTES / 2,
        ROUNDS,
        ITERS_PER_ROUND
    );

    let baseline = time_variant("pack_rgb565 (current)", None, || {
        pack_rgb565(black_box(&frame), black_box(&lut), black_box(output));
    });
    let _ = baseline;
}

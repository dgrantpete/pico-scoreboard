//! BCM timing-stream construction and the refresh-rate cycle model
//! (ports of driver.py's `_update_timing_buffer`, `_estimate_refresh_rate`,
//! and `set_target_refresh_rate`'s base-cycle search).
//!
//! All float math is `f64` and mirrors the Python expression-for-expression
//! (CPython floats are IEEE doubles), so the host goldens generated from the
//! real driver.py compare bit-exactly.

use crate::geometry::{COLOR_BIT_DEPTH, ROW_ADDRESS_COUNT, SHIFT_REGISTER_DEPTH, TIMING_WORDS};

// PIO cycle-model constants, re-derived by counting src/programs.rs (they
// agree with driver.py's, as they must — the instruction streams are 1:1).
//
// Address SM, per displayed row, beyond the on/off delay counts themselves:
//   irq 0 (latch-safe)                                   1
//   mov y, isr  + off loop exit (jmp y-- fall-through)   2
//   mov y, osr  + on loop exit                           2
//   mov y, isr  + off loop exit                          2
// (each `mov y, N; loop: jmp y-- loop` block runs N + 2 cycles: the mov,
// N taken jumps, and one falling-through jump).
/// Address-SM fixed cycles per row on top of `on + 2*off` delay iterations.
pub const ADDRESS_DISPLAY_OVERHEAD_CYCLES: u32 = 7;

/// Binary addressing's `increment_address` happy path:
/// `jmp x-- write_address` (taken) + `mov pins, !x`.
pub const ADDRESS_UPDATE_CYCLES: u32 = 2;

/// Extra address-SM cycles per bitplane transition (not per row):
/// `jmp x--` not taken + `jmp increment_bitplane` (2), `out null, 32` (1),
/// the 4-instruction `set/mov/in/mov` row-counter reload, `out isr, 32` (1).
/// The wrap back into `increment_address` is free (hardware wrap).
pub const BITPLANE_TRANSITION_EXTRA_CYCLES: u32 = 8;

/// Data-SM sequential handshake per row: `wait 1 irq 0` (minimum 1 cycle,
/// the address SM having long since fired it) + `irq 1`.
pub const DATA_HANDSHAKE_OVERHEAD_CYCLES: u32 = 2;

/// Data-SM per-row setup before the pixel loop: `mov x, y`.
pub const DATA_RELOAD_OVERHEAD_CYCLES: u32 = 1;

/// Data-SM pixel loop: `out pins, 8` + `jmp x-- write_data`.
pub const DATA_CYCLES_PER_PIXEL: u32 = 2;

fn blanking_cycles(blanking_ns: u32, system_hz: u32) -> u64 {
    (blanking_ns as u64 * system_hz as u64) / 1_000_000_000
}

/// On/off delay-loop counts for one bitplane. `base_cycles << plane` IS the
/// binary-code-modulation weighting; brightness is the OE duty cycle within
/// that window. The off count is halved because the off delay runs twice per
/// bitframe — once before OE asserts and once after it deasserts, to prevent
/// ghosting. `int(...)` truncation and `//` floor match CPython exactly.
fn plane_on_off(
    base_cycles: u64,
    plane: usize,
    brightness: f64,
    blanking_cycles: u64,
) -> (u64, u64) {
    let brightness_cycle = base_cycles << plane;
    let on = ((brightness * brightness_cycle as f64) as i64).max(0) as u64;
    let off = (((brightness_cycle as i64 - on as i64) / 2) + blanking_cycles as i64).max(0) as u64;
    (on, off)
}

/// The 16-word timing stream the address SM consumes: interleaved
/// `[off, on]` pairs, LSB plane first (matching the framebuffer plane order).
pub fn timing_words(
    base_cycles: u64,
    brightness: f64,
    blanking_ns: u32,
    system_hz: u32,
) -> [u32; TIMING_WORDS] {
    let blanking = blanking_cycles(blanking_ns, system_hz);
    let mut words = [0u32; TIMING_WORDS];
    for plane in 0..COLOR_BIT_DEPTH {
        let (on, off) = plane_on_off(base_cycles, plane, brightness, blanking);
        words[plane * 2] = off.try_into().expect("off cycles exceed u32");
        words[plane * 2 + 1] = on.try_into().expect("on cycles exceed u32");
    }
    words
}

/// Estimated refresh rate in Hz for a given base-cycle count.
///
/// Per bitplane, per row: the two SMs run concurrently after the handshake,
/// so the row time is `max(address display time, data transfer time)` plus
/// the sequential handshake. Data-SM cycles are scaled to system cycles by
/// the clock ratio (the data SM runs at `data_hz * 2`: one SM cycle per
/// pixel-clock edge).
pub fn estimate_refresh_rate(
    base_cycles: u64,
    brightness: f64,
    blanking_ns: u32,
    system_hz: u32,
    data_hz: u32,
) -> f64 {
    let data_clock_ratio = system_hz as f64 / (data_hz as f64 * 2.0);

    let data_transfer_cycles = (DATA_RELOAD_OVERHEAD_CYCLES
        + DATA_CYCLES_PER_PIXEL * SHIFT_REGISTER_DEPTH as u32) as f64
        * data_clock_ratio;

    // Address SM contributes fixed cycles (increment_address + the 1-cycle
    // minimum of its wait); data-SM cycles scale by the clock ratio.
    let handshake_cycles = (ADDRESS_UPDATE_CYCLES + 1) as f64
        + DATA_HANDSHAKE_OVERHEAD_CYCLES as f64 * data_clock_ratio;

    let blanking = blanking_cycles(blanking_ns, system_hz);
    let mut total_frame_cycles = 0.0f64;

    for plane in 0..COLOR_BIT_DEPTH {
        let (on, off) = plane_on_off(base_cycles, plane, brightness, blanking);
        let address_display_cycles = on + 2 * off + ADDRESS_DISPLAY_OVERHEAD_CYCLES as u64;
        let row_cycles = (address_display_cycles as f64).max(data_transfer_cycles) + handshake_cycles;
        total_frame_cycles += ROW_ADDRESS_COUNT as f64 * row_cycles;
    }

    total_frame_cycles += (BITPLANE_TRANSITION_EXTRA_CYCLES as usize * COLOR_BIT_DEPTH) as f64;

    if total_frame_cycles <= 0.0 {
        return 0.0;
    }
    system_hz as f64 / total_frame_cycles
}

/// Pick the integer base-cycle count whose refresh rate is closest to
/// `target_hz` (driver.py `set_target_refresh_rate`): clamp to the maximum
/// (base = 1) if the target is unreachable, otherwise binary-search the
/// smallest base with rate ≤ target and compare it against base − 1 for the
/// arithmetically closer rate. Returns `(base_cycles, achieved_rate)`.
pub fn base_cycles_for_target(
    target_hz: f64,
    brightness: f64,
    blanking_ns: u32,
    system_hz: u32,
    data_hz: u32,
) -> (u64, f64) {
    let estimate =
        |base: u64| estimate_refresh_rate(base, brightness, blanking_ns, system_hz, data_hz);

    let maximum_refresh_rate = estimate(1);
    if target_hz >= maximum_refresh_rate {
        return (1, maximum_refresh_rate);
    }

    // Upper bound for the search: frame time approximated as display-limited,
    // rows * base * (2^depth - 1) cycles, solved for base and doubled.
    let bitplane_sum = ((1u64 << COLOR_BIT_DEPTH) - 1) as f64;
    let divisor = ((target_hz * ROW_ADDRESS_COUNT as f64) * bitplane_sum) as u64;
    let estimated_base_cycles = system_hz as u64 / divisor;
    let mut upper = (estimated_base_cycles * 2).max(2);
    while estimate(upper) > target_hz {
        upper *= 2;
    }

    // Smallest base whose rate is at or below the target.
    let mut lower = 1u64;
    while lower < upper {
        let midpoint = (lower + upper) / 2;
        if estimate(midpoint) > target_hz {
            lower = midpoint + 1;
        } else {
            upper = midpoint;
        }
    }

    let mut base_cycles = lower;
    let rate_at_candidate = estimate(base_cycles);
    if base_cycles > 1 {
        let rate_above_target = estimate(base_cycles - 1);
        let distance_below = target_hz - rate_at_candidate;
        let distance_above = rate_above_target - target_hz;
        if distance_above <= distance_below {
            base_cycles -= 1;
        }
    }

    (base_cycles, estimate(base_cycles))
}

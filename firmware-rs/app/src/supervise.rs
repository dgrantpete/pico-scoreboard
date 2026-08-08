//! Liveness and the core-1 stack measurement.
//!
//! The hardware watchdog, the boot-fail counter and the full `ThreadHealth`
//! port are a later Phase 3 task (SPEC §12). What is here is the half that the
//! app shell can already justify: the render loop publishes
//! [`FRAME_SEQ`](crate::display_core1::FRAME_SEQ) every tick, and something has
//! to read it, or the counter is a fact nobody checks. When the watchdog task
//! lands it feeds from exactly this signal — a stalled loop stops the counter,
//! a *quiet* loop (an idle screen skipping every frame) does not.

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_time::{Duration, Ticker};

use crate::display_core1::FRAME_SEQ;

/// The byte the core-1 stack is painted with before core 1 is started.
///
/// 0xAA rather than zero: `.bss` is already zero, so zero would report the
/// whole stack as touched. The residual error is the other direction — a live
/// stack byte that happens to hold 0xAA at the deepest point reads as untouched
/// — which under-reports by a byte or two at most.
pub const STACK_PAINT: u8 = 0xAA;

/// A read-only view of the core-1 stack, for the high-water mark.
///
/// The stack grows down from the top, so the untouched paint is at the bottom
/// and the deepest byte core 1 ever pushed is the first one that is no longer
/// 0xAA.
pub struct StackProbe {
    bytes: &'static [AtomicU8],
}

impl StackProbe {
    /// # Safety
    ///
    /// `base` must point at `len` bytes of the core-1 stack, painted with
    /// [`STACK_PAINT`] before core 1 was started.
    ///
    /// This is a second view of memory `spawn_core1` also holds a `&'static
    /// mut` to — unavoidable, because that is the signature. Two things keep it
    /// honest: every access here goes through [`AtomicU8`], so core 1 running
    /// on the same bytes is not a data race, and this type has no method that
    /// writes. It is a bench instrument; nothing in the product path reads it.
    pub const unsafe fn new(base: *mut u8, len: usize) -> StackProbe {
        // SAFETY: AtomicU8 and u8 have the same layout and alignment, and the
        // caller promised the region.
        let first = unsafe { AtomicU8::from_ptr(base) };
        StackProbe {
            bytes: unsafe { core::slice::from_raw_parts(first as *const AtomicU8, len) },
        }
    }

    /// `(deepest bytes used, bytes available)`.
    pub fn high_water(&self) -> (usize, usize) {
        let untouched = self
            .bytes
            .iter()
            .take_while(|byte| byte.load(Ordering::Relaxed) == STACK_PAINT)
            .count();
        (self.bytes.len() - untouched, self.bytes.len())
    }
}

/// Report core 1's tick rate and stack depth.
#[embassy_executor::task]
pub async fn liveness(stack: StackProbe) -> ! {
    const PERIOD_S: u32 = 10;
    let mut ticker = Ticker::every(Duration::from_secs(PERIOD_S as u64));
    let mut previous = FRAME_SEQ.load(Ordering::Relaxed);
    loop {
        ticker.next().await;
        let current = FRAME_SEQ.load(Ordering::Relaxed);
        let ticks = current.wrapping_sub(previous);
        previous = current;
        let (used, total) = stack.high_water();
        if ticks == 0 {
            defmt::error!("core 1 has not ticked in {} s — render loop stalled", PERIOD_S);
        } else {
            defmt::info!(
                "core 1: {} ticks in {} s ({} FPS), stack high-water {} of {} B",
                ticks,
                PERIOD_S,
                ticks / PERIOD_S,
                used,
                total,
            );
        }
    }
}

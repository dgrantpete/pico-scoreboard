//! Liveness and the core-1 stack measurement.
//!
//! The hardware watchdog, the boot-fail counter and the full `ThreadHealth`
//! port are a later Phase 3 task (SPEC §12). What is here is the half that the
//! app shell can already justify: the render loop publishes
//! [`FRAME_SEQ`](crate::display_core1::FRAME_SEQ) every tick, and something has
//! to read it, or the counter is a fact nobody checks. When the watchdog task
//! lands it feeds from exactly this signal — a stalled loop stops the counter,
//! a *quiet* loop (an idle screen skipping every frame) does not.
//!
//! It also reads [`poller::health`], which is the *network's* liveness and a
//! genuinely different question — BACKLOG 69's bench unit fell off the Wi-Fi
//! and kept rendering at a perfect 20 FPS all night. The gate task #12's
//! watchdog feeder should use is documented on
//! [`Health`](crate::poller::Health); this reports it every tick so the number
//! is visible before anything depends on it.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Ticker, Timer};

use crate::display_core1::FRAME_SEQ;
use crate::poller;

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

/// What `/api/status` reports where MicroPython reported GC numbers.
///
/// The reasoning behind each field is in [`crate::http::status`]'s docs; this
/// is the plumbing. Published as plain atomics because the measurement is a
/// periodic scan and the reader is an HTTP handler — a request must not trigger
/// a walk of half a megabyte, and must not be able to perturb what it observes.
static CORE0_STACK_USED: AtomicU32 = AtomicU32::new(0);
static CORE0_STACK_TOTAL: AtomicU32 = AtomicU32::new(0);
static CORE1_STACK_USED: AtomicU32 = AtomicU32::new(0);
static CORE1_STACK_TOTAL: AtomicU32 = AtomicU32::new(0);
static STATIC_RAM: AtomicU32 = AtomicU32::new(0);

/// The memory readouts, as one value.
#[derive(Debug, Clone, Copy)]
pub struct Memory {
    /// `.data` + `.bss` — everything statically claimed. A constant per image.
    pub static_ram: u32,
    /// RAM that is not static, which is what the stacks grow into.
    pub ram_free: u32,
    /// Bytes this image occupies in its partition.
    pub image_bytes: u32,
    /// Room an image could still grow into.
    pub partition_free: u32,
    pub core0_stack_used: u32,
    pub core0_stack_total: u32,
    pub core1_stack_used: u32,
    pub core1_stack_total: u32,
}

/// Record how much RAM the image claimed statically.
///
/// Derived in `main` from the two addresses that bracket it, rather than from
/// linker symbols: with flip-link the layout is stack-then-statics, so the top
/// of the stack *is* the bottom of the statics, and RAM's end is a constant.
/// One subtraction, and it cannot disagree with the linker script because it
/// reads the registers the linker script produced.
pub fn record_static_ram(bytes: u32) {
    STATIC_RAM.store(bytes, Ordering::Relaxed);
}

pub fn memory() -> Memory {
    let static_ram = STATIC_RAM.load(Ordering::Relaxed);
    // The flash figures are the linked image's, which the running image cannot
    // read off itself — nothing records its own length. They come from the
    // partition map plus the one number `build.rs` could not know either, so
    // for now the image size is reported as the partition's used extent being
    // unknown: see PARITY.md. `flash_free` still answers the useful half.
    let partition = scoreboard_layout::ACTIVE_SIZE;
    Memory {
        static_ram,
        ram_free: scoreboard_layout::RAM_SIZE.saturating_sub(static_ram),
        image_bytes: image_bytes(),
        partition_free: partition.saturating_sub(image_bytes()),
        core0_stack_used: CORE0_STACK_USED.load(Ordering::Relaxed),
        core0_stack_total: CORE0_STACK_TOTAL.load(Ordering::Relaxed),
        core1_stack_used: CORE1_STACK_USED.load(Ordering::Relaxed),
        core1_stack_total: CORE1_STACK_TOTAL.load(Ordering::Relaxed),
    }
}

/// How much flash this image occupies.
///
/// `__erodata` is the last thing cortex-m-rt's `link.x` places in FLASH before
/// the `.data` initializer image, and `__sidata` + the length of `.data` is
/// where that ends — so the difference from the flash origin is the image.
fn image_bytes() -> u32 {
    unsafe extern "C" {
        static __sidata: u32;
        static __sdata: u32;
        static __edata: u32;
    }
    // Addresses only — the symbols are linker-defined and never dereferenced,
    // which is why taking their addresses needs no `unsafe`.
    let (sidata, sdata, edata) = (
        &raw const __sidata as u32,
        &raw const __sdata as u32,
        &raw const __edata as u32,
    );
    let origin = scoreboard_layout::FLASH_BASE + scoreboard_layout::ACTIVE_OFFSET;
    sidata
        .saturating_add(edata.saturating_sub(sdata))
        .saturating_sub(origin)
}

/// `POST /api/reboot`'s signal.
static REBOOT: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Ask for a reset. Returns immediately; see [`reboot_on_request`].
pub fn request_reboot() {
    REBOOT.signal(());
}

/// Reset the device one second after `/api/reboot` asks.
///
/// `_delayed_reboot`'s port, and the delay is the whole point: `machine.reset()`
/// called from inside the handler would cut the connection before the response
/// reached the browser, and the settings page would show a network error for a
/// reboot that worked. A second is long enough for a 40-byte body to clear a
/// Wi-Fi link and short enough that nobody wonders whether the button worked.
///
/// MicroPython also flushed the log ring to flash here. There is no flash log
/// (SPEC §9), and the RAM ring does not survive a reset by construction — task
/// #12's panic breadcrumb is the record that will, and this is where its write
/// goes when it lands.
#[embassy_executor::task]
pub async fn reboot_on_request() -> ! {
    REBOOT.wait().await;
    defmt::info!("reboot requested: resetting in 1 s");
    Timer::after(Duration::from_secs(1)).await;
    cortex_m::peripheral::SCB::sys_reset()
}

/// Report core 1's tick rate, and measure both stacks.
#[embassy_executor::task]
pub async fn liveness(core1: StackProbe, core0: StackProbe) -> ! {
    const PERIOD_S: u32 = 10;
    let mut ticker = Ticker::every(Duration::from_secs(PERIOD_S as u64));
    let mut previous = FRAME_SEQ.load(Ordering::Relaxed);
    loop {
        ticker.next().await;
        let current = FRAME_SEQ.load(Ordering::Relaxed);
        let ticks = current.wrapping_sub(previous);
        previous = current;

        let (used, total) = core1.high_water();
        CORE1_STACK_USED.store(used as u32, Ordering::Relaxed);
        CORE1_STACK_TOTAL.store(total as u32, Ordering::Relaxed);
        // Core 0's is the expensive one — its stack is the whole RAM remainder,
        // so this walks a few hundred kilobytes. On a 10 s tick that is
        // irrelevant, and it is exactly why `/api/status` reads the published
        // number instead of measuring on demand.
        let (core0_used, core0_total) = core0.high_water();
        CORE0_STACK_USED.store(core0_used as u32, Ordering::Relaxed);
        CORE0_STACK_TOTAL.store(core0_total as u32, Ordering::Relaxed);

        if ticks == 0 {
            defmt::error!("core 1 has not ticked in {} s: render loop stalled", PERIOD_S);
        } else {
            defmt::info!(
                "core 1: {} ticks in {} s ({} FPS), stack high-water {} of {} B; core 0 stack {} of {} B",
                ticks,
                PERIOD_S,
                ticks / PERIOD_S,
                used,
                total,
                core0_used,
                core0_total,
            );
        }

        // The other half of liveness, and the half BACKLOG 69 is about: core 1
        // ticking proves the *renderer* is alive, which the bench unit
        // demonstrated is entirely compatible with having silently fallen off
        // the network. The poller is the only thing that finds that out.
        let health = poller::health();
        match health.since_success_s {
            Some(since) => defmt::info!(
                "poll: {} s since the last success, failure streak {}",
                since,
                health.streak
            ),
            None => defmt::warn!(
                "poll: no successful poll since boot, failure streak {}",
                health.streak
            ),
        }
    }
}

//! Liveness, the hardware watchdog, and the record a death leaves behind.
//!
//! SPEC §12. Three things live here and they are the same subject at three
//! timescales:
//!
//! * [`liveness`] — a 10 s report of core 1's tick rate, both stacks'
//!   high-water marks and the poller's health. Instrumentation; nothing
//!   depends on it.
//! * [`watchdog`] — the hardware watchdog, fed only while the device is
//!   *demonstrably* working, and **deliberately starved** when it is not.
//! * The panic path — [`panic`] in a release build writes a breadcrumb to
//!   uninitialised RAM and resets; the next boot promotes it to flash and
//!   `GET /api/logs/previous` serves it.
//!
//! # The health gate, and the failure it exists for
//!
//! BACKLOG 69: the bench unit fell off the Wi-Fi overnight and **kept rendering
//! at a perfect 20 FPS**. No link-down event fired — embassy-net still believed
//! its IPv4 configuration was up — so a frame counter, which is what a
//! render-loop watchdog would read, proved nothing at all. That is the whole
//! argument for [`gate`] having two halves:
//!
//! * **core 1 is ticking**, from [`FRAME_SEQ`]. Catches a hung or crashed
//!   render loop. A *quiet* loop is not a hung one — an idle screen skips every
//!   frame and still counts ticks — which is why the counter is bumped per
//!   iteration and not per draw.
//! * **something on the network is still answering**, from
//!   [`poller::health`]. Catches the overnight failure, and nothing else in the
//!   firmware can: the poller is the only thing that talks to anything.
//!
//! Both must hold to earn a feed. The second half is skipped in setup mode,
//! where there is no poller by design and gating on it would reset the device
//! every eight seconds while somebody was typing their Wi-Fi password into it.
//!
//! **"Answering" is not "succeeding", and the difference is BACKLOG 70.** The
//! first version of this gate also starved on a failure streak, which meant a
//! backend outage rebooted the device every hundred seconds for as long as the
//! outage lasted — where MicroPython showed the error screen and sat there. A
//! 404 or a 500 proves the radio is on the network just as well as a 200 does,
//! so only *silence* starves now. The whole decision, including why "answer"
//! means the HTTP layer and not TCP, lives on
//! [`Health`](scoreboard_model::poll::Health) where it can be host-tested.
//!
//! # Where `ThreadHealth.healthy` went
//!
//! `main.py`'s feeder checked two things about core 1: `frame_seq` for a *hung*
//! thread, and a `healthy` boolean the display thread's `except` handler cleared
//! for a *crashed* one. Only the first is ported, because the second has no
//! state left to describe. MicroPython's render thread wrapped its loop body in
//! `try/except`, so a render bug could kill the thread and leave the rest of the
//! firmware running — which is exactly the situation a flag is for. Core 1 here
//! has no such handler: a panic goes to [`panic`], which stashes a breadcrumb
//! and resets the chip. There is no window in which core 1 is dead and the
//! device is alive to notice, so the flag would be a variable that is never
//! false. What replaced it is strictly better — the crash is *reported*, at
//! `/api/logs/previous`, rather than inferred from a counter that stopped.
//!
//! # Starving is not the same as crashing, and the difference is worth a write
//!
//! A watchdog reset looks exactly like a power cut from the far side: the ring
//! log is RAM and is gone, and nothing in the logs says why. So the feeder
//! writes a breadcrumb *before* it stops feeding, and the next boot reports it.
//! Without that, the single most important diagnostic — "did this device reset
//! itself, and for which of the two reasons" — is unavailable exactly when a
//! unit is misbehaving in someone else's living room.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker, Timer};
use scoreboard_log::breadcrumb::{Breadcrumb, Cause, Watermarks};

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

// ---------------------------------------------------------------------------
// The breadcrumb: RAM at the moment of death, flash at the next boot
// ---------------------------------------------------------------------------

/// The crash record, in RAM that survives a reset.
///
/// # Why the panic handler does not write flash, though SPEC §9 says it does
///
/// It cannot, and the reason is structural rather than fiddly. Writing flash on
/// this chip means running out of RAM with XIP off, which embassy-rp arranges by
/// **parking core 1 through the multicore FIFO and waiting for it to answer**.
/// Two consequences, either of them fatal to a panic handler:
///
/// * A write from core 1 is refused outright (`Error::InvalidCore`).
/// * A write from core 0 while *core 1* is the one that panicked hangs forever
///   — core 1 is sitting in its own panic handler and will never answer the
///   FIFO. A crash on the render loop is precisely the crash worth recording,
///   so a mechanism that hangs on exactly that case records nothing.
///
/// So the panic handler writes here, to a `.uninit` section the startup code
/// does not clear, and resets. The next boot finds it *before core 1 starts* —
/// where a flash write is free, because there is no core 1 to park — promotes it
/// to storage, and clears the cell. The guarantee SPEC §9 asked for is intact
/// and one it did not ask for comes along: this works when core 1 is the
/// casualty.
///
/// The cell survives both reset paths for the same reason: SRAM keeps its
/// contents through a `SYSRESETREQ` and through a watchdog reset, because
/// neither cuts power to it.
#[unsafe(link_section = ".uninit.SCOREBOARD_BREADCRUMB")]
static mut CELL: BreadcrumbCell = BreadcrumbCell {
    magic: 0,
    length: 0,
    checksum: 0,
    bytes: [0; scoreboard_log::breadcrumb::MAX_BYTES],
};

/// Deliberately not the breadcrumb's own magic: this one says "the RAM cell was
/// filled by a handler on this boot", which is a different claim from "these
/// bytes are a breadcrumb", and uninitialised RAM must fail both.
const CELL_MAGIC: u32 = 0x5343_5242;

#[repr(C)]
struct BreadcrumbCell {
    magic: u32,
    length: u32,
    /// Wrapping sum over `bytes[..length]`. RAM that happens to hold the magic
    /// after a cold boot has to also hold a matching sum, which it will not.
    checksum: u32,
    bytes: [u8; scoreboard_log::breadcrumb::MAX_BYTES],
}

/// First writer wins.
///
/// Both cores can panic, and the second one to arrive must not overwrite the
/// first — the first is the cause and the second is very likely the
/// consequence. A `compare_exchange` is the whole synchronisation: there is no
/// unlock, because a claimed cell is only ever released by the reset that
/// follows.
static CELL_CLAIMED: AtomicBool = AtomicBool::new(false);

fn checksum(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0u32, |sum, byte| sum.wrapping_mul(31).wrapping_add(*byte as u32))
}

/// Record a breadcrumb in RAM and return whether this call is the one that got
/// the cell.
///
/// Safe to call from a panic handler on either core, which is the whole design
/// constraint: no allocation, no lock that could already be held, no peripheral.
pub fn stash_breadcrumb(crumb: &Breadcrumb) -> bool {
    if CELL_CLAIMED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let mut record = [0u8; scoreboard_log::breadcrumb::MAX_BYTES];
    let Ok(length) = crumb.encode(&mut record) else {
        return false;
    };
    // Built as a value and written whole: the alternative — filling the fields
    // through the raw pointer, magic last — buys an ordering guarantee that the
    // claim above already provides, and costs a reference into a `static mut`
    // that the borrow checker cannot help with.
    let cell = BreadcrumbCell {
        magic: CELL_MAGIC,
        length: length as u32,
        checksum: checksum(&record[..length]),
        bytes: record,
    };
    // SAFETY: the claim makes this the only writer for the rest of this boot,
    // and the only reader is `take_breadcrumb`, which runs at the next boot
    // before core 1 exists. Through a raw pointer because a `&mut` to a
    // `static mut` is not available in edition 2024.
    unsafe { (&raw mut CELL).write(cell) };
    true
}

/// Take whatever the previous boot left in RAM, clearing the cell.
///
/// Call once, from `main`, before core 1 starts.
fn take_breadcrumb() -> Option<Breadcrumb> {
    // SAFETY: single-threaded — core 1 does not exist yet — and the only other
    // accessor is `stash_breadcrumb`, which cannot run concurrently for the
    // same reason.
    let (magic, length, stored, bytes) = unsafe {
        let cell = &raw const CELL;
        (
            (*cell).magic,
            (*cell).length as usize,
            (*cell).checksum,
            (*cell).bytes,
        )
    };
    // Clear first, and unconditionally: a cell that fails validation is garbage
    // that must not be re-examined at every boot, and a cell that passes has
    // already been copied out.
    // SAFETY: as above.
    unsafe {
        (&raw mut CELL).write_bytes(0, 1);
    }
    CELL_CLAIMED.store(false, Ordering::Release);

    if magic != CELL_MAGIC || length > bytes.len() {
        return None;
    }
    let record = &bytes[..length];
    if checksum(record) != stored {
        defmt::warn!("supervise: the RAM breadcrumb did not checksum; discarded");
        return None;
    }
    Breadcrumb::decode(record).ok()
}

/// The record `GET /api/logs/previous` serves, read once at boot.
///
/// A copy in RAM rather than a read per request: the endpoint is served from an
/// HTTP handler, and reaching into flash from there would mean parking core 1
/// — dropping a frame — because somebody opened the logs page.
static PREVIOUS: Mutex<CriticalSectionRawMutex, RefCell<Option<Breadcrumb>>> =
    Mutex::new(RefCell::new(None));

/// Promote a RAM breadcrumb into flash, and load the stored one for serving.
///
/// Call once, from `main`, after [`crate::storage::install`] and before core 1
/// starts. Returns whether there is a record to serve.
pub fn load_previous_record() -> bool {
    // The RAM cell is strictly newer than anything in flash — it is from the
    // boot that just ended — so it wins and gets written.
    if let Some(crumb) = take_breadcrumb() {
        defmt::warn!(
            "supervise: the previous boot ended in a {=str} after {} s",
            crumb.cause.as_str(),
            crumb.uptime_s
        );
        crate::storage::save_breadcrumb(&crumb);
        PREVIOUS.lock(|slot| *slot.borrow_mut() = Some(crumb));
        return true;
    }
    match crate::storage::load_breadcrumb() {
        Some(crumb) => {
            defmt::info!(
                "supervise: stored record: {=str} after {} s of uptime",
                crumb.cause.as_str(),
                crumb.uptime_s
            );
            PREVIOUS.lock(|slot| *slot.borrow_mut() = Some(crumb));
            true
        }
        None => false,
    }
}

/// Render the stored record as the plain text `/api/logs/previous` serves.
/// `None` when there has never been one, which is the endpoint's `404`.
pub fn render_previous_record(out: &mut [u8]) -> Option<usize> {
    PREVIOUS.lock(|slot| slot.borrow().as_ref().and_then(|crumb| crumb.render(out)))
}

/// Fill in everything a breadcrumb knows that is not about the cause.
fn describe(cause: Cause) -> Breadcrumb {
    let memory = memory();
    let mut crumb = Breadcrumb::new(cause, embassy_rp::multicore::current_core() as u8);
    crumb.uptime_s = Instant::now().as_secs() as u32;
    crumb.unix_s = crate::ringlog::unix_seconds().unwrap_or(0);
    crumb.watermarks = Watermarks {
        core0_used: memory.core0_stack_used,
        core0_total: memory.core0_stack_total,
        core1_used: memory.core1_stack_used,
        core1_total: memory.core1_stack_total,
    };
    crumb
}

// ---------------------------------------------------------------------------
// Resetting, which is harder than it looks
// ---------------------------------------------------------------------------

/// Reset the chip, from either core, and never return.
///
/// # `SCB::sys_reset()` does not work on this silicon
///
/// Measured on the bench, 2026-08-08. `cortex_m`'s `sys_reset` sets
/// `AIRCR.SYSRESETREQ` and then spins; on the RP2350 that request does **not**
/// reach the power-on state machine, so the chip carries on running and the
/// calling context spins in the loop forever. The observable symptom is worth
/// recording because it is so misleading: the device stays up, core 1 keeps
/// rendering at 20 FPS and the poller keeps polling — because they are other
/// tasks on the other side of an executor that the spinning one has stopped
/// yielding to only in the sense that *this* task never returns — while the
/// thing that asked for the reset simply never gets one. It looked exactly like
/// a hung HTTP server.
///
/// This was already true of `POST /api/reboot`, which has used `sys_reset`
/// since task #10; the induced-panic drill is what found it.
///
/// # What does work
///
/// The watchdog's `TRIGGER` bit, which goes to the PSM and resets the whole
/// system. `Watchdog::trigger_reset` also programs `PSM.WDSEL` first, so the
/// reset covers everything except the two oscillators — and, importantly, not
/// the SRAM *contents*: the breadcrumb cell above survives, which is the whole
/// mechanism, and the starvation drill demonstrates it.
///
/// The peripheral is stolen rather than owned. It has to be: a panic can happen
/// on either core at any moment, and threading a `&mut Watchdog` to every
/// possible panic site is not a thing that can be done. Stealing it is sound
/// here for the reason `steal` exists — the only operation performed is the one
/// that ends the program, so there is no "two owners" window to be wrong about.
pub fn force_reset() -> ! {
    // SAFETY: the returned handle is used for exactly one register write, which
    // resets the chip. Any other holder of this peripheral ceases to exist
    // microseconds later.
    let mut watchdog =
        embassy_rp::watchdog::Watchdog::new(unsafe { embassy_rp::peripherals::WATCHDOG::steal() });
    watchdog.trigger_reset();
    loop {
        cortex_m::asm::nop();
    }
}

// ---------------------------------------------------------------------------
// The panic path
// ---------------------------------------------------------------------------

/// The release panic handler: leave a record, then reset.
///
/// **It does not log.** `defmt`'s global logger takes a lock, and a panic
/// raised from inside a log statement would find that lock already held and
/// deadlock — turning a crash that reboots into a device that hangs. The
/// breadcrumb is the record, and after the reset it is at
/// `GET /api/logs/previous`, which is where a deployed unit's diagnostics live
/// anyway (SPEC §9).
///
/// Debug builds keep `panic-probe` instead: it prints over RTT and traps for
/// the debugger, which is what you want with a probe attached and a breakpoint
/// to inspect. The switch is the profile, so `cargo run --release` — the bench
/// workflow — exercises this path and nothing has to be remembered.
#[cfg(not(debug_assertions))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut crumb = describe(Cause::Panic);
    match info.location() {
        Some(location) => crumb.set_message(format_args!(
            "panicked at {}:{}: {}",
            location.file(),
            location.line(),
            info.message()
        )),
        None => crumb.set_message(format_args!("panicked: {}", info.message())),
    }
    stash_breadcrumb(&crumb);
    force_reset()
}

// ---------------------------------------------------------------------------
// The watchdog
// ---------------------------------------------------------------------------

/// The gate is [`scoreboard_model::poll::gate`], where it is host-tested.
///
/// Nothing about it needs a peripheral: it is a decision over two numbers and a
/// boolean, which is exactly the kind of thing SPEC §2's crate-boundary rule
/// says belongs where a desktop can run it. What stays here is reading the
/// signals and acting on the verdict.
use scoreboard_model::poll::{Unhealthy, gate};

/// Arm the hardware watchdog and feed it while the device is working.
///
/// Opt-in (`config.watchdog.enabled`, default off) and armed **only after the
/// network phase**, both of which are `main.py`'s. Once armed a watchdog cannot
/// be disarmed, so a probe session that halts the core would reset the device a
/// few seconds later — which is exactly why the bench default is off.
///
/// The feed interval is `timeout / 4`, so three consecutive missed feeds are
/// needed before a reset. That is the margin: one late tick is a busy executor,
/// three is a stopped one.
#[embassy_executor::task]
pub async fn watchdog(
    mut hardware: embassy_rp::watchdog::Watchdog,
    timeout_ms: u32,
    poll_interval_s: Option<u32>,
) -> ! {
    let timeout = Duration::from_millis(timeout_ms as u64);
    hardware.start(timeout);
    // embassy-rp's `feed` reloads the counter with the timeout it is given, so
    // the value has to be repeated on every feed rather than remembered from
    // `start`.
    let interval = timeout / 4;
    crate::debug!(
        "watchdog: armed, timeout {} ms, feeding every {} ms",
        timeout_ms,
        interval.as_millis() as u32
    );

    let mut ticker = Ticker::every(interval);
    let mut previous_frame = FRAME_SEQ.load(Ordering::Relaxed);
    loop {
        ticker.next().await;
        let current = FRAME_SEQ.load(Ordering::Relaxed);
        let ticked = current != previous_frame;
        previous_frame = current;

        let health = poller::health();
        let uptime_s = Instant::now().as_secs() as u32;
        let Some(reason) = gate(ticked, poll_interval_s, uptime_s, &health) else {
            hardware.feed(timeout);
            continue;
        };

        // The record first. After the reset the ring log is gone and this is
        // the only thing that will say why the device came back.
        let mut crumb = describe(Cause::WatchdogStarved);
        match reason {
            Unhealthy::RenderLoopStalled => crumb.set_message(format_args!(
                "watchdog starved: render loop stalled (frame_seq held at {previous_frame})"
            )),
            // The failure streak rides along in the message even though it is
            // not what starved anything: "silent for 91 s after 3 failed polls"
            // and "silent for 91 s having never polled" are different faults
            // and this is the only place the difference is recorded.
            Unhealthy::LinkSilent { seconds } => crumb.set_message(format_args!(
                "watchdog starved: nothing answered in {seconds} s, over {} poll intervals \
                 (failure streak {})",
                scoreboard_model::poll::SILENCE_INTERVALS,
                health.streak
            )),
        }
        stash_breadcrumb(&crumb);

        crate::error!(
            "watchdog: starving on purpose ({}); hardware reset within {} ms",
            reason.as_str(),
            timeout_ms
        );
        // Stop feeding and never resume. Returning would end the task, which is
        // the same thing; parking is clearer about the intent and keeps the
        // `-> !` honest.
        core::future::pending::<()>().await;
        unreachable!()
    }
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
/// (SPEC §9), and the RAM ring does not survive a reset by construction. The
/// breadcrumb is deliberately **not** written here either: a reboot somebody
/// asked for is not an abnormal shutdown, and recording it would push the last
/// real crash out of the one slot there is.
///
/// The reset itself goes through [`force_reset`] and not `SCB::sys_reset`. That
/// is a bug fix, not a preference — see that function.
#[embassy_executor::task]
pub async fn reboot_on_request() -> ! {
    REBOOT.wait().await;
    defmt::info!("reboot requested: resetting in 1 s");
    Timer::after(Duration::from_secs(1)).await;
    force_reset()
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

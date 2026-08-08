//! The scoreboard firmware: embassy on both cores, the render loop on core 1
//! driving the HUB75 panel.
//!
//! This is the Phase 3 app shell. Core 0 owns every peripheral and all I/O and
//! stays thin; core 1 runs exactly one task, the render loop. The pure logic —
//! wire format, state machine, pixels — lives in `crates/*` and never imports
//! embassy (SPEC §2's crate-boundary rule).
//!
//! Core 0's work is [`net::bringup`] through the boot and [`poller`] after it:
//! the poller owns the display state, publishes every snapshot, and is the only
//! thing that talks to the backend. Storage, inputs and supervision are the
//! remaining Phase 3 tasks and land beside it.
//!
//! # Two PACs, one chip, and a contract that cannot be compiled
//!
//! This binary links **two** peripheral access crates. `hub75` drives PIO and
//! DMA through `rp235x-pac` because embassy-rp's DMA API cannot express the
//! read-address-trigger chaining the driver depends on; embassy-rp brings its
//! own `rp-pac` for everything else. Each has its own singleton bookkeeping and
//! neither knows the other exists, so "the driver owns PIO0 and DMA 12-15"
//! cannot be checked at the boundary — it is a documented contract
//! (`hub75::driver`'s public constants name the channels).
//!
//! [`main`] honours it the only way available: it takes embassy's handles for
//! that silicon out of `Peripherals` and parks them for the life of the
//! program. Anything that later wants PIO0 has to come to this file and take it
//! out of that binding, which is exactly the conversation that should happen.
//!
//! # Stack overflow faults on both cores
//!
//! `flip-link` (see `.cargo/config.toml`) inverts core 0's RAM layout so its
//! stack sits below `.bss`/`.data` and grows *away* from the statics;
//! `install_core0_stack_guard` then arms MSPLIM at the bottom, so overflow
//! faults instead of quietly eating the 64 KB of hub75 framebuffers.
//! embassy-rp arms the same register for core 1 at the bottom of the stack
//! handed to `spawn_core1`.

#![no_std]
#![no_main]
// picoserve's router is a type, not a table: every `.route()` wraps the
// previous one as its fallback, so eight routes are eight layers of nested
// generics and the trait solver walks all of them. The default limit of 128 is
// not enough to prove `PathRouter` for the result.
#![recursion_limit = "256"]

mod brightness;
mod config;
mod display_core1;
mod http;
mod inputs;
mod logos;
mod net;
mod poller;
mod probe;
mod ringlog;
mod settings;
mod storage;
mod ota;
mod supervise;
mod veml7700;

use core::cell::UnsafeCell;

use cortex_m_rt::entry;
use defmt_rtt as _;
use embassy_executor::Executor;
use embassy_rp::multicore::{Stack, spawn_core1};
use embassy_rp::watchdog::Watchdog;
use hub75::display::{FrameBytes, Hub75Display};
use hub75::driver::{Config, Hub75Driver};
use hub75::geometry::RGB565_FRAME_BYTES;
use scoreboard_model::{SnapshotChannel, Store};
use static_cell::{ConstStaticCell, StaticCell};

// Debug builds print the panic over RTT and trap for the debugger, which is
// what a probe session wants. Release builds install `supervise::panic`
// instead: it writes a breadcrumb to uninitialised RAM and resets, so the crash
// is at `/api/logs/previous` after the reboot. See that function for why the two
// cannot both exist and why the release one does not log.
#[cfg(debug_assertions)]
use panic_probe as _;

/// Core 1's stack.
///
/// 8 KB, per SPEC §11. The loop's own cross-frame state is the bulk of the
/// fixed part — a `PreparedView` carries a 343 B QR bitmap and the pregame
/// cycle — and the deepest transient is `QrBitmap::encode`, which puts two
/// 211 B Reed-Solomon buffers on the stack for the duration of one call. The
/// rest is renderer call frames. `supervise::liveness` reports the measured
/// high-water mark every 10 s; BUDGET.md carries the number.
const CORE1_STACK_BYTES: usize = 8 * 1024;

/// The core-1 stack, in a cell rather than a `ConstStaticCell` so the liveness
/// probe can hold a read-only view of the same bytes without deriving it from
/// the `&'static mut` that `spawn_core1` demands.
struct Core1Stack(UnsafeCell<Stack<CORE1_STACK_BYTES>>);
// SAFETY: the `&'static mut` is taken exactly once, in `main`, before core 1
// starts; the only other reader is `StackProbe`, which reads through atomics
// and never writes.
unsafe impl Sync for Core1Stack {}

static CORE1_STACK: Core1Stack = Core1Stack(UnsafeCell::new(Stack::new()));

static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();

/// The RGB565 drawing surface core 1 renders into — 16,384 B. App-owned, not a
/// driver static: the driver never allocates the drawing surface, and putting
/// it in a static means constructing the display never moves a frame through
/// anybody's stack.
static FRAME: ConstStaticCell<FrameBytes> = ConstStaticCell::new([0; RGB565_FRAME_BYTES]);

/// The core-0 → core-1 handoff: three snapshot slots and an atomic index.
static CHANNEL: SnapshotChannel = SnapshotChannel::new();

/// The team crests core 1 draws from, and the authoritative display state core
/// 0 publishes.
///
/// Both are `StaticCell` rather than task-arena locals for the same reason:
/// they are handed to exactly one owner, once, and that owner is not the
/// function that creates them. The crest pool goes to core 1 (see
/// [`logos`]'s module docs for why the pixels live there and the bookkeeping
/// does not); the store is lent to [`net::bringup`] for the boot screen and
/// then moves to the poller.
static CRESTS: StaticCell<logos::CrestPool> = StaticCell::new();
static STORE: StaticCell<Store> = StaticCell::new();

/// Paint core 0's stack so its high-water mark can be measured, and record how
/// much RAM the statics took.
///
/// flip-link inverts the RAM layout: the stack sits at the bottom, growing down
/// *away* from `.data`/`.bss`, with MSPLIM armed at the very bottom. That gives
/// two addresses that bracket everything — the limit is the floor of the stack,
/// and the current stack pointer at the top of `main` is within a frame or two
/// of its ceiling, which is also the bottom of the statics. So:
///
/// - **static RAM** = end of RAM − stack ceiling, and
/// - **stack size** = ceiling − MSPLIM.
///
/// Neither needs a linker symbol; both are read from the registers the linker
/// script produced, which is one fewer thing that can silently disagree with
/// `memory.x`.
///
/// The paint stops a margin below the current stack pointer, because the region
/// being painted is the one this function is running on. The margin costs a
/// slightly pessimistic watermark — the deepest frames of `main` itself read as
/// already-used — and buys the guarantee that painting cannot scribble on its
/// own return address.
fn paint_core0_stack() -> supervise::StackProbe {
    /// Bytes left unpainted below the stack pointer: room for this function's
    /// frame and the compiler's spills, with a wide margin.
    const MARGIN: u32 = 1024;

    let floor = cortex_m::register::msplim::read();
    let pointer = cortex_m::register::msp::read();
    let ceiling = pointer.saturating_sub(MARGIN);

    supervise::record_static_ram(
        (scoreboard_layout::RAM_BASE + scoreboard_layout::RAM_SIZE).saturating_sub(pointer),
    );

    let len = ceiling.saturating_sub(floor) as usize;
    // SAFETY: `[floor, ceiling)` is core-0 stack below the live frame — MSPLIM
    // is its floor by construction, and `ceiling` is a full margin below the
    // stack pointer, so nothing live is in the range.
    let region = unsafe { core::slice::from_raw_parts_mut(floor as *mut u8, len) };
    region.fill(supervise::STACK_PAINT);

    // SAFETY: just painted, and `StackProbe` only ever reads, through atomics.
    unsafe { supervise::StackProbe::new(floor as *mut u8, len) }
}

#[entry]
fn main() -> ! {
    // Before `init`, per embassy-rp: it fails if the MPU is already configured.
    embassy_rp::install_core0_stack_guard().expect("core-0 stack guard already installed");
    // Immediately after, because it reads the limit that call just programmed.
    let core0_stack = paint_core0_stack();
    let peripherals = embassy_rp::init(Default::default());
    let system_clock_hz = embassy_rp::clocks::clk_sys_freq();

    // The two-PAC contract, made as physical as it can be: embassy's handles
    // for the silicon `hub75` drives go here and never come out.
    let _owned_by_hub75 = (
        peripherals.PIO0,
        peripherals.DMA_CH12,
        peripherals.DMA_CH13,
        peripherals.DMA_CH14,
        peripherals.DMA_CH15,
    );

    // After `embassy_rp::init`, not before: embassy owns clock and XOSC
    // bring-up, and the driver derives its pixel clock, refresh rate and OE
    // timing from whatever `clk_sys` ended up at. There is no `machine.freq()`
    // to consult later and the driver does not support the clock moving under
    // it, so it has to be told the settled value.
    // Everything that touches flash happens **here**, before `spawn_core1`.
    //
    // A flash program or erase runs from RAM with XIP disabled, and embassy-rp
    // arranges that by parking core 1 through the multicore FIFO. With core 1
    // not yet started that park is a no-op, so the boot's reads — and the one
    // write that promotes a crash breadcrumb — cost nothing at all. The same
    // calls after the render loop is up cost the panel a frame, which is why
    // `storage`'s API is blocking and says so.
    storage::install(peripherals.FLASH);
    storage::read_device_id();
    let has_previous_record = supervise::load_previous_record();

    // The watchdog comes out of `Peripherals` **before** the configuration is
    // read, and that ordering is Phase 4's doing. Under `link-boot-integrated`
    // the bootloader armed an 8 s watchdog before it jumped here and an RP2350
    // watchdog cannot be disarmed, so every blocking step of this boot is on a
    // clock. `config::load` is the one that can take real time: a storage
    // region that does not parse is erased sector by sector, ~245 of them, and
    // it feeds through this closure between each one.
    let mut watchdog = Watchdog::new(peripherals.WATCHDOG);
    let boot_config = config::load(&mut || {
        watchdog.feed(embassy_time::Duration::from_millis(
            supervise::BOOT_WATCHDOG_TIMEOUT_MS as u64,
        ))
    });

    // What the bootloader did on the way in — a plain boot, a trial that has
    // not been confirmed, or a rollback. Before core 1 starts, because a
    // rollback writes the attempt record and a flash write is free until the
    // render loop is up.
    ota::read_boot_state();

    // Read before the watchdog task can arm it again, and logged because it is
    // the one fact that separates "somebody pulled the plug" from "this device
    // reset itself" — which is exactly the question BACKLOG 69 left open.
    match watchdog.reset_reason() {
        Some(embassy_rp::watchdog::ResetReason::TimedOut) => defmt::warn!(
            "boot: the previous run ended in a watchdog timeout{=str}",
            if has_previous_record {
                "; see /api/logs/previous"
            } else {
                " with no breadcrumb — see supervise's docs"
            }
        ),
        Some(embassy_rp::watchdog::ResetReason::Forced) => {
            defmt::info!("boot: reset was forced (probe, or POST /api/reboot)")
        }
        None => defmt::info!("boot: power-on or SYSRESETREQ"),
    }

    let pac = rp235x_pac::Peripherals::take().expect("rp235x-pac peripherals already taken");
    let driver = Hub75Driver::new(pac.PIO0, pac.DMA, Config::defaults(system_clock_hz));
    defmt::info!(
        "scoreboard-app {=str} up ({=str} image): sys {} Hz, panel refresh {} Hz",
        ota::VERSION,
        env!("LINK_PROFILE"),
        system_clock_hz,
        driver.refresh_rate() as u32
    );
    let display = Hub75Display::new(FRAME.take(), driver);

    // Handed out once: core 0 keeps the publisher, core 1 takes the reader.
    let (publisher, reader) = CHANNEL.split();

    // SAFETY: first and only use of this cell, single-threaded, before core 1
    // exists.
    let stack = unsafe { &mut *CORE1_STACK.0.get() };
    stack.mem.fill(supervise::STACK_PAINT);
    // SAFETY: the region was just painted, and `StackProbe` only ever reads it,
    // through atomics. See its constructor.
    let stack_probe =
        unsafe { supervise::StackProbe::new(CORE1_STACK.0.get().cast::<u8>(), CORE1_STACK_BYTES) };

    // Every task below has a pool of one, so `unwrap` can only fire on a second
    // spawn of the same task — which would be a bug in this function, not a
    // condition to handle.
    let crests = CRESTS.init(logos::CrestPool::new());
    spawn_core1(peripherals.CORE1, stack, move || {
        let executor = EXECUTOR1.init(Executor::new());
        executor.run(|spawner| {
            spawner.spawn(defmt::unwrap!(display_core1::render_loop(
                reader, display, crests
            )))
        });
    });

    // The radio's silicon, decided here so the resource map lives in one place.
    // PIO2 is RP2350-only, which is what lets the panel keep PIO0 whole and
    // leaves PIO1 for the button driver. DMA CH0 is low on purpose: hub75 owns
    // 12-15 through the other PAC and neither side can see the other's claim,
    // so the two ranges are kept apart by convention and by `net`'s module
    // docs, which is the only mechanism available.
    let radio = net::NetPeripherals {
        pio: peripherals.PIO2,
        dma: peripherals.DMA_CH0,
        pwr: peripherals.PIN_23,
        cs: peripherals.PIN_25,
        dio: peripherals.PIN_24,
        clk: peripherals.PIN_29,
    };

    // Everything the boot hands to whichever provisioning arm wins. The buttons
    // and the watchdog are both *after the network phase* by design — the
    // buttons because there is no poller to send presses to in setup mode, and
    // the watchdog because arming it around a blocking Wi-Fi join would reset
    // the device mid-join. `main.py` ordered both the same way.
    let deferred = net::Deferred {
        inputs: inputs::InputPeripherals {
            pio: peripherals.PIO1,
            a: peripherals.PIN_10,
            b: peripherals.PIN_22,
        },
        watchdog,
        system_clock_hz,
    };

    // Core 1 starts from `RenderSettings::new()` and the driver from
    // `Config::defaults`, neither of which has seen the stored configuration —
    // so it is sent once here, with every hook on.
    settings::publish_display(settings::DisplayUpdate::boot(&boot_config));

    let store = STORE.init(Store::new());

    let executor = EXECUTOR0.init(Executor::new());
    executor.run(|spawner| {
        // Owns the store and the publisher through the boot — together they are
        // what draws the startup screen — and hands both to whichever mode
        // wins. On the station path that is the poller; in setup mode nothing
        // publishes again, because the setup screen does not change.
        spawner.spawn(defmt::unwrap!(net::bringup(
            spawner, store, publisher, radio, deferred
        )));
        spawner.spawn(defmt::unwrap!(supervise::liveness(stack_probe, core0_stack)));
        spawner.spawn(defmt::unwrap!(supervise::reboot_on_request()));
        // Both modes, always: a setup screen in a dark room should dim too, and
        // an absent sensor is a supported configuration either way.
        spawner.spawn(defmt::unwrap!(brightness::auto_brightness(
            brightness::SensorPeripherals {
                i2c: peripherals.I2C0,
                sda: peripherals.PIN_0,
                scl: peripherals.PIN_1,
            }
        )));
    });
}

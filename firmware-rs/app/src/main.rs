//! The scoreboard firmware: embassy on both cores, the render loop on core 1
//! driving the HUB75 panel.
//!
//! This is the Phase 3 app shell. Core 0 owns every peripheral and all I/O and
//! stays thin; core 1 runs exactly one task, the render loop. The pure logic —
//! wire format, state machine, pixels — lives in `crates/*` and never imports
//! embassy (SPEC §2's crate-boundary rule).
//!
//! What core 0 does *today* is [`demo`]: a placeholder that pushes hard frames
//! through the snapshot channel so the render loop can be measured. Wi-Fi, the
//! poller, the HTTP server, storage and inputs are the remaining Phase 3 tasks
//! and land beside it.
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

mod demo;
mod display_core1;
mod probe;
mod supervise;

use core::cell::UnsafeCell;

use cortex_m_rt::entry;
use defmt_rtt as _;
use embassy_executor::Executor;
use embassy_rp::multicore::{Stack, spawn_core1};
use hub75::display::{FrameBytes, Hub75Display};
use hub75::driver::{Config, Hub75Driver};
use hub75::geometry::RGB565_FRAME_BYTES;
use panic_probe as _;
use scoreboard_model::SnapshotChannel;
use scoreboard_render::game::{LOGO_BYTES, LogoSlot};
use static_cell::{ConstStaticCell, StaticCell};

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

/// Stand-in team crests, until the poller downloads real ones.
///
/// `const`-built and never written, so all 2,304 B live in flash — the crest
/// pool the renderer borrows is the one thing a snapshot deliberately does not
/// carry (it holds a `LogoRef` handle instead, because the pool outlives any
/// one commit and copying two crests per publish would double the handoff).
static CRESTS: [LogoSlot; 2] = [flat_crest(0xF800), flat_crest(0x001F)];

const fn flat_crest(color: u16) -> LogoSlot {
    let [low, high] = color.to_le_bytes();
    let mut slot = [0u8; LOGO_BYTES];
    let mut index = 0;
    while index < LOGO_BYTES {
        slot[index] = low;
        slot[index + 1] = high;
        index += 2;
    }
    slot
}

#[entry]
fn main() -> ! {
    // Before `init`, per embassy-rp: it fails if the MPU is already configured.
    embassy_rp::install_core0_stack_guard().expect("core-0 stack guard already installed");
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
    let pac = rp235x_pac::Peripherals::take().expect("rp235x-pac peripherals already taken");
    let driver = Hub75Driver::new(pac.PIO0, pac.DMA, Config::defaults(system_clock_hz));
    defmt::info!(
        "scoreboard-app up ({=str} image): sys {} Hz, panel refresh {} Hz",
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
    spawn_core1(peripherals.CORE1, stack, move || {
        let executor = EXECUTOR1.init(Executor::new());
        executor.run(|spawner| {
            spawner.spawn(defmt::unwrap!(display_core1::render_loop(
                reader, display, &CRESTS
            )))
        });
    });

    let executor = EXECUTOR0.init(Executor::new());
    executor.run(|spawner| {
        spawner.spawn(defmt::unwrap!(demo::feed(publisher)));
        spawner.spawn(defmt::unwrap!(demo::brightness()));
        spawner.spawn(defmt::unwrap!(supervise::liveness(stack_probe)));
    });
}

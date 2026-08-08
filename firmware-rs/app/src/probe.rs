//! The frame-time probe — Phase 3's acceptance instrument.
//!
//! # The question it answers
//!
//! MicroPython's render thread could not afford to draw a scrolling line of
//! text glyph by glyph: three line-score rows measured ~41 ms of a 50 ms frame,
//! and a long play line measured over 50 ms, which is why `state.py`
//! pre-rendered every scrolling line into a 1-bit strip on core 0 and why a
//! whole strip-pool machinery existed to hold them. `scoreboard-render` threw
//! all of that away on the thesis that a compiled glyph blit is cheap enough to
//! do inline, every frame (see `prepared`'s module docs). That thesis is a
//! claim about real silicon, so this measures it on real silicon.
//!
//! Budget: **≤ 50 ms per frame is the hard ceiling** (SPEC §6); low single
//! digits is what the thesis predicts.
//!
//! # What it separates, and why
//!
//! Four buckets, because they answer different questions:
//!
//! * `rebuild` — [`PreparedView::sync`] on the frames where the commit changed.
//!   Per-commit work, not per-frame; it is in the budget only because it lands
//!   inside a frame.
//! * `render` — the whole screen draw. This is the number the thesis is about.
//! * `show` — `load_rgb565` + `flip`: repacking 8,192 RGB565 pixels into eight
//!   BCM bitplanes. Pure driver cost, unaffected by what was drawn, and the
//!   floor under every frame.
//! * `brightness` — re-deriving the OE timing stream when core 0 moves the
//!   auto-brightness atomic. Measured separately to show it is not a per-frame
//!   cost hiding in the render number.
//!
//! [`PreparedView::sync`]: scoreboard_render::PreparedView::sync
//!
//! # Attribution
//!
//! Core 0 announces which scenario it is driving through [`enter`]; core 1
//! reads it at the top of each frame and flushes its counters when it changes,
//! so every reported line belongs to exactly one scenario with no reliance on
//! log ordering between two cores. The demo owns the scenario sequence; when
//! the real poller replaces it, this hook goes with it.

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_time::Instant;

/// What core 0 is currently putting on the panel.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
pub enum Scenario {
    /// Boot progress: static screen, redrawn only on a commit.
    Startup = 0,
    /// Static screen, no commits — the skip path, and the cheapest frame there
    /// is.
    Idle = 1,
    /// Live MLB with a play flash at the wire's 255-byte cap scrolling through
    /// the bottom strip. The play-line worst case.
    MlbPlayFlash = 2,
    /// Final line score with three overflowing rows scrolling in lockstep. The
    /// case that measured ~41 ms in MicroPython.
    FinalLinescore = 3,
    /// The same final screen under a sticky toast overlay: an icon blit plus a
    /// dim pass over all 8,192 pixels.
    ToastOverlay = 4,
    /// The league menu, which preempts the mode dispatch and marquees every
    /// frame, so nothing is ever skipped.
    Menu = 5,
}

impl Scenario {
    const ALL: [Scenario; 6] = [
        Scenario::Startup,
        Scenario::Idle,
        Scenario::MlbPlayFlash,
        Scenario::FinalLinescore,
        Scenario::ToastOverlay,
        Scenario::Menu,
    ];

    fn from_index(index: u8) -> Scenario {
        Scenario::ALL[index as usize % Scenario::ALL.len()]
    }
}

static SCENARIO: AtomicU8 = AtomicU8::new(Scenario::Startup as u8);

/// Core 0: announce the scenario every subsequent frame belongs to.
pub fn enter(scenario: Scenario) {
    SCENARIO.store(scenario as u8, Ordering::Relaxed);
}

/// Core 1: which scenario this frame belongs to.
pub fn current() -> Scenario {
    Scenario::from_index(SCENARIO.load(Ordering::Relaxed))
}

/// A running stopwatch. `embassy_time`'s tick is 1 µs on RP2350, so the
/// quantisation is 1 µs against a budget of 50,000.
pub struct Lap(Instant);

impl Lap {
    pub fn start() -> Lap {
        Lap(Instant::now())
    }

    pub fn elapsed_us(self) -> u32 {
        // A lap that somehow ran for over an hour saturates rather than
        // wrapping into a small, believable number.
        self.0.elapsed().as_micros().min(u32::MAX as u64) as u32
    }
}

/// Count, total and worst case for one bucket. Reset at every report, so the
/// max is a per-scenario max rather than a high-water mark for all time.
#[derive(Clone, Copy)]
struct Bucket {
    count: u32,
    total_us: u32,
    max_us: u32,
}

impl Bucket {
    const EMPTY: Bucket = Bucket {
        count: 0,
        total_us: 0,
        max_us: 0,
    };

    fn record(&mut self, microseconds: u32) {
        self.count += 1;
        self.total_us = self.total_us.saturating_add(microseconds);
        self.max_us = self.max_us.max(microseconds);
    }

    fn mean_us(&self) -> u32 {
        if self.count == 0 {
            0
        } else {
            self.total_us / self.count
        }
    }
}

/// The probe itself. Lives in the render loop's loop-local state, like every
/// other piece of cross-frame state on core 1.
pub struct FrameProbe {
    scenario: Scenario,
    ticks: u32,
    drawn: u32,
    overruns: u32,
    rebuild: Bucket,
    render: Bucket,
    show: Bucket,
    brightness: Bucket,
    /// Wall time from the top of a tick to the end of its work — what the
    /// 50 ms deadline is actually spent against.
    frame: Bucket,
}

/// A backstop, not the normal trigger: scenario changes are what flush the
/// counters, and the demo's scenarios are 100 ticks each. This only fires if
/// core 0 stops advancing them, at which point 30 s of silence from the probe
/// would itself be the confusing part.
const REPORT_EVERY: u32 = 600;

impl FrameProbe {
    pub const fn new() -> FrameProbe {
        FrameProbe {
            scenario: Scenario::Startup,
            ticks: 0,
            drawn: 0,
            overruns: 0,
            rebuild: Bucket::EMPTY,
            render: Bucket::EMPTY,
            show: Bucket::EMPTY,
            brightness: Bucket::EMPTY,
            frame: Bucket::EMPTY,
        }
    }

    /// Call at the top of every tick. Flushes and resets when core 0 has moved
    /// on to a different scenario, so a report never blends two of them.
    pub fn begin_tick(&mut self, scenario: Scenario) {
        if scenario != self.scenario {
            self.report();
            self.scenario = scenario;
        }
        self.ticks += 1;
    }

    pub fn record_rebuild(&mut self, microseconds: u32) {
        self.rebuild.record(microseconds);
    }

    pub fn record_render(&mut self, microseconds: u32) {
        self.drawn += 1;
        self.render.record(microseconds);
    }

    pub fn record_show(&mut self, microseconds: u32) {
        self.show.record(microseconds);
    }

    pub fn record_brightness(&mut self, microseconds: u32) {
        self.brightness.record(microseconds);
    }

    pub fn record_overrun(&mut self) {
        self.overruns += 1;
    }

    /// Call at the bottom of every tick, before the deadline sleep.
    pub fn end_tick(&mut self, microseconds: u32) {
        self.frame.record(microseconds);
        if self.ticks >= REPORT_EVERY {
            self.report();
        }
    }

    /// Emit one line per scenario and start counting again.
    fn report(&mut self) {
        if self.ticks == 0 {
            return;
        }
        defmt::info!(
            "probe {}: {} ticks ({} drawn, {} skipped, {} overrun) | \
             frame mean {} us max {} | render mean {} us max {} (n={}) | \
             show mean {} us max {} | rebuild mean {} us max {} (n={}) | \
             brightness mean {} us max {} (n={})",
            self.scenario,
            self.ticks,
            self.drawn,
            self.ticks - self.drawn,
            self.overruns,
            self.frame.mean_us(),
            self.frame.max_us,
            self.render.mean_us(),
            self.render.max_us,
            self.render.count,
            self.show.mean_us(),
            self.show.max_us,
            self.rebuild.mean_us(),
            self.rebuild.max_us,
            self.rebuild.count,
            self.brightness.mean_us(),
            self.brightness.max_us,
            self.brightness.count,
        );
        self.ticks = 0;
        self.drawn = 0;
        self.overruns = 0;
        self.rebuild = Bucket::EMPTY;
        self.render = Bucket::EMPTY;
        self.show = Bucket::EMPTY;
        self.brightness = Bucket::EMPTY;
        self.frame = Bucket::EMPTY;
    }
}

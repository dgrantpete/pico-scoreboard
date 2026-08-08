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
//! A report belongs to one screen, because blending an idle screen's skipped
//! frames with a scrolling line score's would hide exactly the number this
//! exists to find. Which screen is **read off the latched snapshot** — the
//! frame's own input — so it needs no announcement from core 0 and cannot
//! disagree with what was drawn. Before the poller existed, a demo module
//! published a scenario through an atomic and this read that instead; the
//! snapshot was always the better source, and it became available when the demo
//! went away.

use embassy_time::Instant;
use scoreboard_model::{Mode, ScoreboardSnapshot};

/// What a frame drew, for attribution. [`Mode`], plus the one thing that is not
/// a mode: the league menu preempts the mode dispatch entirely, so a menu frame
/// is nothing like the frame the mode underneath would have produced.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Screen {
    Menu,
    Mode(ModeName),
}

impl Screen {
    pub fn of(snapshot: &ScoreboardSnapshot) -> Screen {
        if snapshot.menu.active {
            return Screen::Menu;
        }
        Screen::Mode(ModeName(snapshot.mode))
    }
}

/// [`Mode`], printable. The model has no defmt dependency by design — it is a
/// host-tested crate — so the name is spelled here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ModeName(Mode);

impl defmt::Format for ModeName {
    fn format(&self, formatter: defmt::Formatter<'_>) {
        defmt::write!(
            formatter,
            "{=str}",
            match self.0 {
                Mode::Idle => "idle",
                Mode::Startup => "startup",
                Mode::NoGames => "no_games",
                Mode::Setup => "setup",
                Mode::Error => "error",
                Mode::Updating => "updating",
                Mode::MlbLive => "mlb_live",
                Mode::Pregame => "pregame",
                Mode::Final => "final",
                Mode::SoccerLive => "soccer_live",
                Mode::SoccerFinal => "soccer_final",
                Mode::NbaLive => "nba_live",
                Mode::FootballLive => "football_live",
            }
        )
    }
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
        // A scenario the probe never entered has no samples and no mean.
        self.total_us.checked_div(self.count).unwrap_or(0)
    }
}

/// The probe itself. Lives in the render loop's loop-local state, like every
/// other piece of cross-frame state on core 1.
pub struct FrameProbe {
    screen: Screen,
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

/// A screen change flushes the counters; this is what reports a screen that
/// does not change. A live game holds for `game_rotation_seconds` — 60 by
/// default — and an idle scoreboard holds for as long as there are no games, so
/// without this the probe would go silent for hours on exactly the case worth
/// watching. 600 ticks is 30 s.
const REPORT_EVERY: u32 = 600;

impl FrameProbe {
    pub const fn new() -> FrameProbe {
        FrameProbe {
            screen: Screen::Mode(ModeName(Mode::Startup)),
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

    /// Call at the top of every tick. Flushes and resets when the snapshot has
    /// moved on to a different screen, so a report never blends two of them.
    pub fn begin_tick(&mut self, screen: Screen) {
        if screen != self.screen {
            self.report();
            self.screen = screen;
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

    /// Emit one line per screen and start counting again.
    fn report(&mut self) {
        if self.ticks == 0 {
            return;
        }
        defmt::info!(
            "probe {}: {} ticks ({} drawn, {} skipped, {} overrun) | \
             frame mean {} us max {} | render mean {} us max {} (n={}) | \
             show mean {} us max {} | rebuild mean {} us max {} (n={}) | \
             brightness mean {} us max {} (n={})",
            self.screen,
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

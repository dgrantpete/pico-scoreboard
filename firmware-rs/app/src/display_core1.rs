//! Core 1: the render loop, and nothing else.
//!
//! The port of `display.py`'s `run_display_thread`. Core 1 never touches the
//! network, storage, or flash. Everything it reads from core 0 arrives through
//! exactly two channels, both of them lock-free: the
//! [`SnapshotChannel`](scoreboard_model::SnapshotChannel) it latches once per
//! frame, and the [`BRIGHTNESS`] atomic. Everything it publishes back is
//! [`FRAME_SEQ`].
//!
//! # Pacing: deadline-based, and it never fast-forwards
//!
//! Each tick targets `deadline + FRAME_MS` and the sleep absorbs however long
//! the frame took, so the wall-clock spacing between frames stays even. The
//! older shape — render, then sleep a fixed 50 ms — let frame cost leak into
//! the spacing and made scroll steps visibly uneven.
//!
//! An overrun **re-anchors**: the next deadline is measured from the moment the
//! late frame finished, not from the slot it missed. Bursting to catch up would
//! make a stalled display fast-forward through the animation it owed, which is
//! worse than the stall. Overruns are counted, not corrected.
//!
//! # The mutation contract, in Rust
//!
//! `display.py:1761-1817` allows core 1 to write exactly four things, and
//! `scoreboard-render`'s crate docs map each to something the type system
//! enforces. The one that lands here is the first: **all cross-frame state
//! lives in [`LoopState`], which is a local of [`render_loop`]**. It is never
//! passed into `frame::render` or below — renderers take `WallMs` and
//! `FrameElapsed` *values*, so a renderer has no way to name it. MicroPython
//! enforced that with a grep (`ls` must not appear below `render_frame`); here
//! the reference genuinely does not exist in a renderer's scope.
//!
//! `FRAME_SEQ` is the deliberate exception the contract already carved out:
//! it is cross-core on purpose, which is why it is an atomic and not a field.
//!
//! # What is not here yet
//!
//! MicroPython wrapped the loop body in `try/except` so a render bug logged and
//! continued. Rust has no equivalent that is worth having: a panic on core 1
//! goes to `panic-probe` today, and to the breadcrumb-plus-reset path once
//! supervision lands (SPEC §12). Reading `FRAME_SEQ` is how a stalled render
//! loop is detected either way.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use embassy_futures::yield_now;
use embassy_time::{Duration, Instant, Timer};
use hub75::display::Hub75Display;
use hub75::driver::Hub75Driver;
use scoreboard_model::Reader;
use scoreboard_render::blit::Canvas;
use scoreboard_render::game::{LogoSlot, Logos, Scene};
use scoreboard_render::geometry::{HEIGHT, RenderSettings, WIDTH};
use scoreboard_render::time::{FRAME_MS, FrameRail, WallMs};
use scoreboard_render::{PreparedView, SkipMemo, frame};

use crate::probe::{FrameProbe, Lap};

/// One frame period, from the render crate's own constant so the loop cannot
/// pace at a rate the scroll speeds were not chosen for.
const FRAME: Duration = Duration::from_millis(FRAME_MS);

/// Panel brightness, 0..=255, mapped onto the driver's `[0.0, 1.0]`.
///
/// Core 0's auto-brightness loop writes it at whatever rate the light sensor
/// justifies; core 1 applies it at the top of a frame and only when it changed,
/// because applying it rewrites the driver's whole OE timing stream. SPEC §4:
/// anything higher-rate than a commit is a plain atomic, not a snapshot field.
pub static BRIGHTNESS: AtomicU8 = AtomicU8::new(u8::MAX);

/// Ticks completed, rendered *or* skipped.
///
/// The watchdog feeder reads this to tell a hung render loop from a quiet one
/// (SPEC §12): an idle scoreboard draws nothing for minutes at a time, so
/// "frames drawn" is not liveness — "loop went round" is. Bumped at the end of
/// a tick's work, so a hang inside the render path shows up as a stall.
pub static FRAME_SEQ: AtomicU32 = AtomicU32::new(0);

/// Everything on core 1 that outlives a frame. A local of [`render_loop`],
/// never reachable from a renderer.
struct LoopState {
    rail: FrameRail,
    prepared: PreparedView,
    memo: SkipMemo,
    probe: FrameProbe,
    /// Last value pushed into the driver, so an unchanged atomic costs nothing.
    brightness: u8,
    /// The configured variants, dividers and scroll speed.
    ///
    /// Cross-frame state, so it lives here and nowhere else — the mutation
    /// contract's first bucket. `Scene` borrows it for the length of one frame
    /// and no renderer can name it. Core 0 replaces it through
    /// [`crate::settings`]; until the first update arrives it is the compiled
    /// default, which is what the panel showed before this loop existed.
    settings: RenderSettings,
}

/// Apply a config change from core 0.
///
/// The order is `api_routes.py`'s, and it is load-bearing in one place: the
/// data clock goes in before the refresh rate, because `set_data_clock`
/// deliberately does not re-balance the timing and `set_target_refresh_rate`
/// is what re-derives it from the new clock. Blanking time going in *after*
/// the refresh rate is also MicroPython's order, and it leaves the rate
/// un-rebalanced against the new blanking — preserved rather than fixed,
/// because the achieved rate is observable and this is a parity release.
///
/// Both invalidations are needed and for different reasons: the prepared view
/// caches a scroll window measured against the old speed, and the skip memo
/// would otherwise let a static screen keep showing the old dividers until the
/// next commit — which, on an idle scoreboard, could be minutes.
fn apply_settings(
    state: &mut LoopState,
    driver: &mut Hub75Driver,
    update: crate::settings::DisplayUpdate,
) {
    let lap = Lap::start();
    if update.applied.render_settings {
        state.settings = update.render;
    }
    if update.applied.data_clock {
        driver.set_data_clock(update.data_clock_hz);
    }
    if update.applied.refresh_rate {
        driver.set_target_refresh_rate(update.target_refresh_rate_hz);
    }
    if update.applied.gamma {
        driver.set_gamma(update.gamma);
    }
    if update.applied.blanking_time {
        driver.set_blanking_time(update.blanking_time_ns);
    }
    state.prepared.invalidate();
    state.memo.invalidate();
    // Not a `ringlog` line: this is core 1, and the ring's lock is a critical
    // section that would land inside the frame. The probe's timing is the
    // measurement that matters here anyway.
    defmt::info!(
        "core 1: settings applied in {} us, refresh now {} Hz",
        lap.elapsed_us(),
        driver.refresh_rate() as u32
    );
}

/// Core 1's only task.
#[embassy_executor::task]
pub async fn render_loop(
    mut reader: Reader<'static>,
    mut display: Hub75Display<'static, Hub75Driver>,
    logos: &'static [LogoSlot],
) -> ! {
    let mut state = LoopState {
        rail: FrameRail::new(),
        prepared: PreparedView::new(),
        memo: SkipMemo::new(),
        probe: FrameProbe::new(),
        brightness: BRIGHTNESS.load(Ordering::Relaxed),
        settings: RenderSettings::new(),
    };
    // Whatever the atomic said at startup has not reached the driver yet.
    display
        .sink_mut()
        .set_brightness(state.brightness as f64 / 255.0);

    defmt::info!("core 1: render loop up at {} FPS", 1000 / FRAME_MS);

    let mut deadline = Instant::now() + FRAME;
    loop {
        // Before the stopwatch starts, because this is where the probe emits
        // its per-scenario report and defmt formatting is dev-only cost that
        // should not land in a frame-time number. The deadline check at the
        // bottom still sees it — it works in absolute time — so an overrun
        // caused by logging is still counted as one.
        state.probe.begin_tick(crate::probe::current());
        let tick = Lap::start();

        // Acquire-ordered: everything core 0 wrote before publishing is visible
        // to this frame, and the reference stays valid until the next latch, so
        // the frame renders from one consistent state however many times core 0
        // publishes underneath it.
        let snapshot = reader.latch();
        let now = WallMs(Instant::now().as_millis());

        let requested = BRIGHTNESS.load(Ordering::Relaxed);
        if requested != state.brightness {
            let lap = Lap::start();
            display.sink_mut().set_brightness(requested as f64 / 255.0);
            state.brightness = requested;
            state.probe.record_brightness(lap.elapsed_us());
        }

        if let Some(update) = crate::settings::take_display() {
            apply_settings(&mut state, display.sink_mut(), update);
        }

        state.rail.advance_and_latch(snapshot);

        // Before the skip check, and before the scene exists: a rebuilt
        // prepared view *is* what a new commit means, and the scene borrows it.
        let lap = Lap::start();
        if state.prepared.sync(snapshot, &state.settings) {
            state.probe.record_rebuild(lap.elapsed_us());
        }

        let scene = Scene {
            snapshot,
            prepared: &state.prepared,
            settings: &state.settings,
            logos: Logos::new(logos),
            now,
            view: state.rail.view_elapsed(),
            play: state.rail.play_elapsed(),
        };

        if state.memo.should_render(snapshot, now) {
            let lap = Lap::start();
            {
                let mut canvas = Canvas::new(display.buffer_mut(), WIDTH, HEIGHT);
                frame::render(&mut canvas, &scene);
            }
            state.probe.record_render(lap.elapsed_us());

            let lap = Lap::start();
            display.show();
            state.probe.record_show(lap.elapsed_us());
        }

        FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
        state.probe.end_tick(tick.elapsed_us());

        let finished = Instant::now();
        if finished >= deadline {
            // The frame ran past its slot. Re-anchor to now: a display never
            // fast-forwards through the frames it owed.
            state.probe.record_overrun();
            deadline = finished + FRAME;
            // No sleep to yield on, and core 1's executor has exactly one task
            // — but let it round the loop properly anyway, so adding a second
            // task here never turns into a starvation bug.
            yield_now().await;
        } else {
            Timer::at(deadline).await;
            deadline += FRAME;
        }
    }
}

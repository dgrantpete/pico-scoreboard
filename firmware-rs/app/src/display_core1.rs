//! Core 1: the render loop, and nothing else.
//!
//! The port of `display.py`'s `run_display_thread`. Core 1 never touches the
//! network, storage, or flash — it *reads* flash, where an event's clip lies,
//! but never writes it. Everything it reads from core 0 arrives lock-free: the
//! [`SnapshotChannel`](scoreboard_model::SnapshotChannel) it latches once per
//! frame, the [`BRIGHTNESS`] atomic, and [`crate::event`]'s live-event and
//! dismiss atomics. Everything it publishes back is [`FRAME_SEQ`] and whether
//! a press would dismiss the event on screen.
//!
//! # Events take the whole panel
//!
//! While an event clip plays, the tick decodes a clip frame onto the surface
//! instead of rendering a screen; [`EventPlayer`] says which, and how far to
//! scale the brightness for its fades. The screen underneath keeps its rail,
//! its prepared view and its commits — it is only not drawn — and the memo is
//! invalidated when the clip leaves, because a static screen has no commit
//! coming to repaint over the clip's last frame.
//!
//! # Pacing: deadline-based, and it never fast-forwards
//!
//! Each tick targets the next frame slot and the sleep absorbs however long the
//! frame took, so the wall-clock spacing between frames stays even. The older
//! shape — render, then sleep a fixed period — let frame cost leak into the
//! spacing and made scroll steps visibly uneven.
//!
//! An overrun **re-anchors**: the next deadline is measured from the moment the
//! late frame finished, not from the slot it missed. Bursting to catch up would
//! make a stalled display fast-forward through the animation it owed, which is
//! worse than the stall. Overruns are counted, not corrected.
//!
//! ## The slot is computed, not accumulated
//!
//! At 60 FPS the period is 16⅔ ms, which is not a whole number of the
//! microseconds `embassy_time` counts in. Adding a rounded per-frame duration
//! would drift without bound — 16,667 µs per frame is 2 ppm fast, which is only
//! a second a week, but 16,666 would be 8 ppm slow and there is no reason to
//! pick either. So the loop keeps an anchor plus a frame index and targets
//! `anchor + frame_us(index)`, the same exact-rational quantiser the frame rail
//! uses ([`scoreboard_render::time::frame_us`]). The error against true time
//! never exceeds one microsecond, at any uptime.
//!
//! Re-anchoring on overrun is what resets the index: the anchor moves to the
//! moment the late frame finished and the count starts again, which is
//! precisely "measured from the moment it finished".
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
use scoreboard_render::event::{Clip, Draw, EventPlayer};
use scoreboard_render::game::{Logos, Scene};
use scoreboard_render::geometry::{HEIGHT, RenderSettings, WIDTH};
use scoreboard_render::time::{FPS, FrameRail, WallMs, frame_us};
use scoreboard_render::{PreparedView, SkipMemo, frame};

use crate::logos::CrestPool;
use crate::probe::{FrameProbe, Lap, Screen};

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
    ///
    /// The *effective* value: [`BRIGHTNESS`] scaled by the event player's fade.
    /// A fade moves it every frame for about a second, at the 112–190 µs a
    /// change costs (BUDGET.md), well inside the frame.
    brightness: u8,
    event: EventPlayer,
    /// The live event's clip, parsed once when the event goes live rather than
    /// per frame, and which [`crate::event::EVENTS`] index it was parsed for —
    /// `Some(index)` with no clip is an event whose clip was rejected, kept so
    /// the rejection is logged once and not every frame.
    clip_for: Option<u8>,
    clip: Option<Clip<'static>>,
    /// Slowest clip-frame decode of the current play, for the one line logged
    /// when it ends — how a bench run answers "does this fit the frame".
    clip_decode_max_us: u32,
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
///
/// Every branch here is a store or a small register restamp. The one that was
/// not is gamma: expanding a `Power` LUT is 27.6 ms, which does not fit a
/// 16.7 ms frame, so [`crate::settings::DisplayUpdate`] carries the finished
/// table and this only copies it.
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
    crests: &'static mut CrestPool,
) -> ! {
    let mut state = LoopState {
        rail: FrameRail::new(),
        prepared: PreparedView::new(),
        memo: SkipMemo::new(),
        probe: FrameProbe::new(),
        brightness: BRIGHTNESS.load(Ordering::Relaxed),
        settings: RenderSettings::new(),
        event: EventPlayer::new(),
        clip_for: None,
        clip: None,
        clip_decode_max_us: 0,
    };
    // Whatever the atomic said at startup has not reached the driver yet.
    display
        .sink_mut()
        .set_brightness(state.brightness as f64 / 255.0);

    defmt::info!("core 1: render loop up at {} FPS", FPS);

    // The slot index counts from `anchor`, and both reset together on an
    // overrun. See the module docs for why the deadline is computed from the
    // pair rather than accumulated.
    let mut anchor = Instant::now();
    let mut slot: u64 = 1;
    loop {
        let tick = Lap::start();

        // Acquire-ordered: everything core 0 wrote before publishing is visible
        // to this frame, and the reference stays valid until the next latch, so
        // the frame renders from one consistent state however many times core 0
        // publishes underneath it.
        let snapshot = reader.latch();
        // **After** the latch, never before. A snapshot naming a crest slot was
        // published *after* that slot's pixels were sent, so latching it first
        // guarantees the update is already in the channel — see `logos`'s
        // module docs.
        crests.apply_pending();
        let now = WallMs(Instant::now().as_millis());

        // Before the stopwatch would matter, because this is where the probe
        // emits its per-screen report and defmt formatting is dev-only cost
        // that should not land in a frame-time number. The deadline check at
        // the bottom still sees it — it works in absolute time — so an overrun
        // caused by logging is still counted as one.
        state.probe.begin_tick(Screen::of(snapshot));

        let live = crate::event::live();
        if live.map(|(index, _)| index) != state.clip_for {
            // A different event, or none: whatever was playing belongs to the
            // old clip, and its deltas mean nothing on top of the new one.
            state.event.abort(now);
            state.clip_for = live.map(|(index, _)| index);
            state.clip = live.and_then(|(_, event)| match Clip::parse(event.clip) {
                Ok(clip) => Some(clip),
                Err(error) => {
                    defmt::error!(
                        "core 1: event {} clip rejected: {}",
                        event.name,
                        defmt::Debug2Format(&error)
                    );
                    None
                }
            });
        }
        let step = state.event.tick(
            now,
            state.clip.map(|clip| clip.timing()),
            snapshot.mode,
            snapshot.menu.active,
            crate::event::take_dismiss(),
        );
        crate::event::set_capturing(step.captures_input);

        let requested = BRIGHTNESS.load(Ordering::Relaxed);
        let effective = ((requested as u16 * step.fade as u16 + 127) / 255) as u8;
        if effective != state.brightness {
            let lap = Lap::start();
            display.sink_mut().set_brightness(effective as f64 / 255.0);
            state.brightness = effective;
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
            logos: Logos::new(crests.slots()),
            now,
            view: state.rail.view_elapsed(),
            play: state.rail.play_elapsed(),
        };

        match step.draw {
            Draw::Screen { redraw } => {
                if redraw {
                    state.memo.invalidate();
                    defmt::info!(
                        "core 1: event clip off, slowest decode {} us",
                        state.clip_decode_max_us
                    );
                }
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
            }
            Draw::ClipFrame(index) => {
                let lap = Lap::start();
                // The player only plays with a clip, so `None` cannot happen;
                // it is handled like a decode failure rather than unwrapped.
                if index == 0 {
                    defmt::info!("core 1: event clip on");
                    state.clip_decode_max_us = 0;
                }
                let decoded = state
                    .clip
                    .map(|clip| clip.decode_into(index, display.buffer_mut()));
                let decode_us = lap.elapsed_us();
                state.clip_decode_max_us = state.clip_decode_max_us.max(decode_us);
                state.probe.record_render(decode_us);
                if let Some(Ok(())) = decoded {
                    let lap = Lap::start();
                    display.show();
                    state.probe.record_show(lap.elapsed_us());
                } else {
                    // The surface holds a partial frame, which is never shown:
                    // the next tick repaints the screen over it.
                    defmt::error!(
                        "core 1: event clip frame {} failed: {}",
                        index,
                        defmt::Debug2Format(&decoded)
                    );
                    state.event.abort(now);
                }
            }
            // The clip frame on the panel stays up for its second tick.
            Draw::Hold => {}
        }

        FRAME_SEQ.fetch_add(1, Ordering::Relaxed);
        state.probe.end_tick(tick.elapsed_us());

        // Stamp first, then work out what it should have been: the deadline is
        // arithmetic on values that are already fixed, and reading the clock
        // after it would charge this frame for computing it.
        let finished = Instant::now();
        let deadline = anchor + Duration::from_micros(frame_us(slot));
        if finished >= deadline {
            // The frame ran past its slot. Re-anchor to now: a display never
            // fast-forwards through the frames it owed.
            state.probe.record_overrun();
            anchor = finished;
            slot = 1;
            // No sleep to yield on, and core 1's executor has exactly one task
            // — but let it round the loop properly anyway, so adding a second
            // task here never turns into a starvation bug.
            yield_now().await;
        } else {
            Timer::at(deadline).await;
            slot += 1;
        }
    }
}

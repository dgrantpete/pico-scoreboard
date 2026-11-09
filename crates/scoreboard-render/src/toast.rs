//! Toasts: centered icon overlays — lock, unlock, the skip spinner — that dim
//! the whole frame behind them.
//!
//! An icon composes over the finished frame: the frame fades down the dim
//! ladder to half brightness so the icon reads against a busy background, then
//! back up after the toast expires — dim only, no icon — so the overlay eases
//! in and out instead of snapping. It never claims a screen's bottom strip.
//!
//! A toast rides the **wall rail**: its life is a duration, and a stalled frame
//! really did spend that time. See [`crate::time`].
//!
//! # Sticky
//!
//! A sticky toast persists past [`TOAST_DISPLAY_MS`] until something clears it
//! — an in-flight skip owns its spinner for exactly the work it announces.
//! [`TOAST_STICKY_MAX_MS`] is the belt against a bug stranding one on screen;
//! requests hard-cap at 15 s, so 20 s is only reachable through a logic error.

use crate::blit::{Canvas, FADE_TERMS};
use crate::geometry::{HEIGHT, WIDTH};
use crate::time::WallMs;
use crate::{padlock, pulse, scale, spinner};
use scoreboard_model::snapshot::{TOAST_DISPLAY_MS, TOAST_STICKY_MAX_MS, ToastView};
use scoreboard_model::{Millis, Rgb888, ScoreboardSnapshot, ToastKind};

/// How long one rung of the dim ladder holds.
///
/// **A duration, not a frame count** — it happens to equal the frame period the
/// parity release ran at, which made it one rung per frame and makes it look
/// frame-coupled. It is not: [`overlay`] divides a `WallMs` elapsed by this, so
/// the fade is 150 ms in and 150 ms out at any frame rate, and a faster loop
/// samples the same four rungs more often rather than racing through them.
/// `tests/screens.rs` pins that.
///
/// Which also means a finer ladder is now *available* and was not before — 60
/// FPS can show three times as many rungs in the same 150 ms — BACKLOG 83.
const FADE_STEP_MS: Millis = 50;
/// Fade-out walks 5/8 → 3/4 → 7/8, then the frame is clean.
const FADE_OUT_MS: Millis = FADE_STEP_MS * 3;

/// A press rejected while a skip is in flight dims the visible toast for one
/// triangle cycle toward [`PULSE_DIP`] darkness, then back.
const PULSE_MS: Millis = 1000;
const PULSE_DIP: u32 = 128;

const CENTER_X: i32 = WIDTH / 2;
const CENTER_Y: i32 = HEIGHT / 2;

/// One revolution of the skip spinner.
const SPINNER_PERIOD_MS: Millis = 1000;
/// Dots of fading tail behind the head, leaving a 2-dot gap.
const SPINNER_TRAIL: u32 = 10;

/// The ring and the padlock, centred on the panel.
const SPINNER_X: i32 = CENTER_X - spinner::WIDTH / 2;
const SPINNER_Y: i32 = CENTER_Y - spinner::HEIGHT / 2;
const LOCK_X: i32 = CENTER_X - padlock::WIDTH / 2;
/// The padlock's row 12 (its body's top) lands on y = 31.
const LOCK_Y: i32 = 19;

/// Whether a toast is currently up. `updated_ms == 0` means one was never set.
pub fn is_active(toast: &ToastView, now: WallMs) -> bool {
    toast.updated_ms != 0 && now.since(toast.updated_ms).0 < window(toast)
}

/// Whether an expired icon toast's dim is still fading back out.
///
/// The render loop must keep re-rendering static screens through this tail —
/// there is no commit to trigger the redraw that finishes the fade.
pub fn overlay_fading(toast: &ToastView, now: WallMs) -> bool {
    if toast.updated_ms == 0 {
        return false;
    }
    let elapsed = now.since(toast.updated_ms).0;
    let window = window(toast);
    (window..window + FADE_OUT_MS).contains(&elapsed)
}

const fn window(toast: &ToastView) -> Millis {
    if toast.sticky {
        TOAST_STICKY_MAX_MS
    } else {
        TOAST_DISPLAY_MS
    }
}

/// Toast brightness, 0..=255, honoring a rejected-press dim cycle.
fn brightness(toast: &ToastView, now: WallMs) -> u8 {
    if toast.pulse_ms != 0 {
        let elapsed = now.since(toast.pulse_ms).0;
        if elapsed < PULSE_MS {
            let triangle = pulse(elapsed, PULSE_MS);
            return (255 - ((triangle * PULSE_DIP) >> 8)) as u8;
        }
    }
    255
}

/// Draw an active icon toast over the finished frame.
///
/// Called last in each game-facing render so nothing paints over it. `canvas`
/// must be the whole frame: the dim works on four-byte words of channels
/// across the contiguous buffer.
pub fn overlay(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot, now: WallMs) {
    let toast = &snapshot.toast;
    if toast.updated_ms == 0 {
        return;
    }
    let elapsed = now.since(toast.updated_ms).0;
    let window = window(toast);

    if elapsed >= window {
        // Fade-out tail: walk the ladder back up. The icon is already gone.
        let steps = ((elapsed - window) / FADE_STEP_MS) as usize;
        if steps <= 2 {
            canvas.dim(FADE_TERMS[2 - steps]);
        }
        return;
    }

    let step = ((elapsed / FADE_STEP_MS) as usize).min(3);
    canvas.dim(FADE_TERMS[step]);

    let brightness = brightness(toast, now);
    let primary = snapshot.ui_colors.primary;
    match toast.kind {
        ToastKind::Spinner => spinner(canvas, elapsed, brightness, primary),
        ToastKind::Unlock => lock(canvas, scale(primary, brightness.into(), 255), true),
        ToastKind::Lock => lock(canvas, scale(primary, brightness.into(), 255), false),
    }
}

/// The skip spinner: a comet of 12 dots on a radius-12 ring, one revolution per
/// second.
///
/// The head advances in 1/256ths of a dot step, so the trail's brightness
/// shifts every frame rather than stepping dot to dot — the fluidity is the
/// point, and it is the one thing on the panel that visibly gains from the
/// frame rate, since the ring is driven off the wall clock and simply gets
/// three times as many samples per revolution. Dots outside the trail get no
/// colour at all, so they are not drawn.
fn spinner(canvas: &mut Canvas<'_>, elapsed: Millis, dim: u8, primary: Rgb888) {
    let full_turn = spinner::DOTS as u32 * 256;
    let head = ((elapsed % SPINNER_PERIOD_MS) * full_turn as u64 / SPINNER_PERIOD_MS) as u32;
    let span = SPINNER_TRAIL * 256;
    spinner::draw(canvas, SPINNER_X, SPINNER_Y, |dot| {
        let lag = (head + full_turn - dot as u32 * 256) % full_turn;
        if lag >= span {
            return None;
        }
        let mut value = 255 - (lag * 255) / span;
        if dim != 255 {
            value = (value * dim as u32) >> 8;
        }
        Some(scale(primary, value, 255))
    });
}

/// A padlock centered on the panel. Open lifts the shackle's left leg out of
/// the body; both states share one rectangle, so one position serves.
fn lock(canvas: &mut Canvas<'_>, color: Rgb888, open: bool) {
    padlock::draw(canvas, LOCK_X, LOCK_Y, open, color);
}

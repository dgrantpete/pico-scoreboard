//! Toasts: the bottom-strip text form, and the centered icon overlays that dim
//! the whole frame behind them.
//!
//! # Two shapes, one lifetime
//!
//! A [`ToastKind::Text`] toast owns the bottom strip of a live screen, which is
//! why the bottom-strip priority on every live screen is *toast > play flash >
//! sport content*. The three icon kinds instead compose over the finished frame:
//! the frame fades down the dim ladder to half brightness so the icon reads
//! against a busy background, then back up after the toast expires — dim only,
//! no icon — so the overlay eases in and out instead of snapping.
//!
//! Both ride the **wall rail**: a toast's life is a duration, and a stalled
//! frame really did spend that time. See [`crate::time`].
//!
//! # Sticky
//!
//! A sticky toast persists past [`TOAST_DISPLAY_MS`] until something clears it
//! — an in-flight skip owns its spinner for exactly the work it announces.
//! [`TOAST_STICKY_MAX_MS`] is the belt against a bug stranding one on screen;
//! requests hard-cap at 15 s, so 20 s is only reachable through a logic error.

use crate::blit::{Canvas, FADE_TERMS};
use crate::font::{self, Align, Style};
use crate::generated::layout;
use crate::geometry::{HEIGHT, PLAY_TEXT, WIDTH};
use crate::time::WallMs;
use crate::{BLACK, WHITE, generated, pulse, rgb565};
use scoreboard_model::snapshot::{TOAST_DISPLAY_MS, TOAST_STICKY_MAX_MS, ToastView};
use scoreboard_model::{Millis, ScoreboardSnapshot, ToastKind};

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
/// FPS can show three times as many rungs in the same 150 ms. It would cost a
/// deliberate pixel divergence from the MicroPython baseline the parity harness
/// compares against, so it is BACKLOG 83 rather than part of this change.
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
/// Dots on the ring.
const SPINNER_DOTS: usize = 12;

const SPINNER_X: i32 = CENTER_X - layout::toast_spinner::SPRITE.width / 2;
const SPINNER_Y: i32 = CENTER_Y - layout::toast_spinner::SPRITE.height / 2;
const LOCK_X: i32 = CENTER_X - layout::toast_lock_closed::SPRITE.width / 2;
/// Sprite row 12 (the lock body's top) lands on y = 31.
const LOCK_Y: i32 = 19;

/// Angular dot index → palette index.
///
/// `tools/gen_toast_icons.py` bakes dot *k*'s color so that its RGB565 value is
/// exactly `k + 1`, while `compile_layout.py` assigns palette indices in
/// row-major first-seen order — which is not angular order. MicroPython
/// inverted the compiled palette once at import and raised there if the
/// contract had drifted. Doing it in a `const fn` moves that check from import
/// time to build time: recolored dots or a wrong count fail `cargo build`.
const SPINNER_ORDER: [u8; SPINNER_DOTS] = spinner_order();

const fn spinner_order() -> [u8; SPINNER_DOTS] {
    let palette = &layout::toast_spinner::PALETTE;
    assert!(
        palette.len() == SPINNER_DOTS + 1,
        "the spinner sprite must have one transparent entry plus one per dot"
    );
    let mut order = [0u8; SPINNER_DOTS];
    let mut seen = [false; SPINNER_DOTS];
    let mut entry = 1;
    while entry <= SPINNER_DOTS {
        let angular = palette[entry] as usize;
        assert!(
            angular >= 1 && angular <= SPINNER_DOTS,
            "a spinner dot's baked color is not its angular index — regenerate \
             the icons with tools/gen_toast_icons.py"
        );
        assert!(
            !seen[angular - 1],
            "two spinner dots claim one angular slot"
        );
        seen[angular - 1] = true;
        order[angular - 1] = entry as u8;
        entry += 1;
    }
    order
}

/// Whether a toast is currently up.
///
/// A text toast with no text is not a toast; `updated_ms == 0` means one was
/// never set.
pub fn is_active(toast: &ToastView, now: WallMs) -> bool {
    if toast.updated_ms == 0 {
        return false;
    }
    if toast.kind == ToastKind::Text && toast.text.is_empty() {
        return false;
    }
    now.since(toast.updated_ms).0 < window(toast)
}

/// Whether an expired icon toast's dim is still fading back out.
///
/// The render loop must keep re-rendering static screens through this tail —
/// there is no commit to trigger the redraw that finishes the fade.
pub fn overlay_fading(toast: &ToastView, now: WallMs) -> bool {
    if toast.kind == ToastKind::Text || toast.updated_ms == 0 {
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

fn shade(brightness: u8) -> u16 {
    if brightness == 255 {
        WHITE
    } else {
        rgb565(brightness, brightness, brightness)
    }
}

/// Draw an active text toast into the bottom strip.
///
/// Returns whether it drew, which is the caller's signal to skip its own
/// bottom-strip content. Icon toasts return false here and go through
/// [`overlay`] instead — they never consume the strip.
pub fn strip(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot, now: WallMs) -> bool {
    let toast = &snapshot.toast;
    if toast.kind != ToastKind::Text || !is_active(toast, now) {
        return false;
    }
    let color = shade(brightness(toast, now));
    let mut strip = canvas.slice(PLAY_TEXT);
    strip.fill(BLACK);
    font::draw_unscrolled(
        &mut strip,
        &toast.text,
        Align::Center,
        Style::new(&generated::UNSCII_16, color),
    );
    true
}

/// Draw an active icon toast over the finished frame.
///
/// Called last in each game-facing render so nothing paints over it. `canvas`
/// must be the whole frame: the dim works on pixel pairs across the buffer.
pub fn overlay(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot, now: WallMs) {
    let toast = &snapshot.toast;
    if toast.kind == ToastKind::Text || toast.updated_ms == 0 {
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
    match toast.kind {
        ToastKind::Spinner => spinner(canvas, elapsed, brightness),
        ToastKind::Unlock => lock(canvas, shade(brightness), true),
        _ => lock(canvas, shade(brightness), false),
    }
}

/// The skip spinner: a comet of 12 dots on a radius-12 ring, one revolution per
/// second.
///
/// The head advances in 1/256ths of a dot step, so the trail's brightness
/// shifts every frame rather than stepping dot to dot — the fluidity is the
/// point, and it is the one thing on the panel that visibly gains from the
/// frame rate, since the ring is driven off the wall clock and simply gets
/// three times as many samples per revolution. Dots outside the trail get the
/// key as their color, so the blit skips them entirely.
fn spinner(canvas: &mut Canvas<'_>, elapsed: Millis, dim: u8) {
    let sprite = layout::toast_spinner::SPRITE;
    let mut palette = layout::toast_spinner::PALETTE;
    let key = sprite.key.unwrap_or(0);

    let head =
        ((elapsed % SPINNER_PERIOD_MS) * (SPINNER_DOTS as u64 * 256) / SPINNER_PERIOD_MS) as u32;
    let span = SPINNER_TRAIL * 256;
    for (dot, entry) in SPINNER_ORDER.iter().enumerate() {
        let full_turn = SPINNER_DOTS as u32 * 256;
        let lag = (head + full_turn - dot as u32 * 256) % full_turn;
        let entry = *entry as usize;
        if lag >= span {
            palette[entry] = key;
            continue;
        }
        let mut value = 255 - (lag * 255) / span;
        if dim != 255 {
            value = (value * dim as u32) >> 8;
        }
        palette[entry] = rgb565(value as u8, value as u8, value as u8);
    }
    canvas.blit(&sprite.tinted(&palette), SPINNER_X, SPINNER_Y);
}

/// A padlock centered on the panel. Open lifts the shackle's left leg out of
/// the body; both sprites share a canvas, so one blit position serves.
fn lock(canvas: &mut Canvas<'_>, color: u16, open: bool) {
    let (sprite, defaults) = if open {
        (
            layout::toast_lock_open::SPRITE,
            layout::toast_lock_open::PALETTE,
        )
    } else {
        (
            layout::toast_lock_closed::SPRITE,
            layout::toast_lock_closed::PALETTE,
        )
    };
    let mut palette = defaults;
    palette[1] = color;
    canvas.blit(&sprite.tinted(&palette), LOCK_X, LOCK_Y);
}

//! The screens that show no game: boot progress, idle, no games, AP setup, the
//! error card, and the OTA progress card.
//!
//! Every one is a pure reader. The strings were built by core 0 when the state
//! changed, so nothing here formats text — with one deliberate exception noted
//! on [`startup`].
//!
//! # Regions
//!
//! `display.Regions` preallocated a `Region` object per text slot on core 0,
//! because allocating one per frame on core 1 meant garbage. A [`Canvas`]
//! sub-view is a borrow with no allocation behind it, so the slots are `const`
//! rectangles taken fresh each frame and the whole `Regions` class disappears.

use crate::blit::{Canvas, Slice};
use crate::font::{self, Align, Scroll, Style};
use crate::geometry::WIDTH;
use crate::prepared::PreparedView;
use crate::time::WallMs;
use crate::widgets::{count_dots, progress_bar};
use crate::{BLACK, generated, pack, toast};
use core::fmt::Write as _;
use scoreboard_model::{ScoreboardSnapshot, SetupReason};

// -- Slot rectangles ---------------------------------------------------------

const STARTUP_TITLE: Slice = rect(0, 4, WIDTH, 16);
/// The step counter sits right of the progress bar: bar_x + bar_width + 4.
const STARTUP_STEP: Slice = rect(108, 24, WIDTH - 108, 8);
const STARTUP_OPERATION: Slice = rect(0, 42, WIDTH, 8);
const STARTUP_DETAIL: Slice = rect(0, 54, WIDTH, 8);
const PROGRESS_BAR: Slice = rect((WIDTH - 80) / 2, 24, 80, 8);

const IDLE_TITLE: Slice = rect(0, 16, WIDTH, 16);
const IDLE_SUBTITLE: Slice = rect(0, 40, WIDTH, 8);

const NO_GAMES_TITLE: Slice = rect(0, 20, WIDTH, 16);
const NO_GAMES_SUBTITLE: Slice = rect(0, 40, WIDTH, 8);

const ERROR_TITLE: Slice = rect(0, 0, WIDTH, 16);
const ERROR_LINE_Y: [i32; 4] = [24, 34, 44, 54];

/// Left inset shared by every setup line.
const SETUP_PAD: i32 = 2;
/// Where the QR sits, and the gap the text keeps from it.
const QR_TOP: i32 = 2;
const QR_RIGHT_MARGIN: i32 = 2;
const QR_TEXT_GAP: i32 = 4;

/// Wi-Fi attempt dots, centered in the gap between the progress bar (ending at
/// y = 31) and the operation line (y = 42). Sized for exactly three dots —
/// coupled to `max_retries = 3` in the station-mode loop.
const STARTUP_DOTS: Slice = {
    let width = 3 * (generated::layout::dot::SPRITE.width + 1) - 1;
    rect((WIDTH - width) / 2, 34, width, 4)
};

// -- Screens -----------------------------------------------------------------

/// Boot progress: a title, a step bar, and the current operation.
///
/// The step counter ("2/5") is the one string this crate formats. `state.py`
/// pre-built it on core 0 as `step_text` because `f"{step}/{total}"` allocates
/// in MicroPython; the Rust model carries the two numbers instead, and
/// formatting them into a stack local allocates nothing.
pub fn startup(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot) {
    let colors = &snapshot.ui_colors;
    let startup = &snapshot.startup;
    canvas.fill(BLACK);

    title(canvas, STARTUP_TITLE, "BOOTING", pack(colors.accent));

    let progress = if startup.total_steps == 0 {
        0
    } else {
        (startup.step as u32 * 100 / startup.total_steps as u32) as u8
    };
    progress_bar(
        canvas,
        PROGRESS_BAR,
        progress,
        pack(colors.secondary),
        pack(colors.accent),
    );

    if startup.attempts_total > 0 {
        count_dots(canvas, STARTUP_DOTS, startup.attempt, None);
    }

    let mut step_text = heapless::String::<8>::new();
    let _ = write!(step_text, "{}/{}", startup.step, startup.total_steps);
    small(
        canvas,
        STARTUP_STEP,
        &step_text,
        Align::Left,
        pack(colors.secondary),
    );
    small(
        canvas,
        STARTUP_OPERATION,
        &startup.operation,
        Align::Center,
        pack(colors.primary),
    );
    if !startup.detail.is_empty() {
        small(
            canvas,
            STARTUP_DETAIL,
            &startup.detail,
            Align::Center,
            pack(colors.secondary),
        );
    }
}

/// OTA progress: download percentage, then the restart countdown.
///
/// Reuses the startup slots — the geometry is identical and the two modes can
/// never coexist, since `Updating` is only entered long after boot finishes.
pub fn updating(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot) {
    let colors = &snapshot.ui_colors;
    let updating = &snapshot.updating;
    canvas.fill(BLACK);

    title(canvas, STARTUP_TITLE, "UPDATING", pack(colors.accent));
    progress_bar(
        canvas,
        PROGRESS_BAR,
        updating.progress,
        pack(colors.secondary),
        pack(colors.accent),
    );

    if !updating.percent_text.is_empty() {
        small(
            canvas,
            STARTUP_STEP,
            &updating.percent_text,
            Align::Left,
            pack(colors.secondary),
        );
    }
    small(
        canvas,
        STARTUP_OPERATION,
        &updating.phase,
        Align::Center,
        pack(colors.primary),
    );
    if !updating.detail.is_empty() {
        small(
            canvas,
            STARTUP_DETAIL,
            &updating.detail,
            Align::Center,
            pack(colors.secondary),
        );
    }
}

/// The waiting screen, and the fallback for any mode without a renderer.
pub fn idle(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot) {
    let colors = &snapshot.ui_colors;
    canvas.fill(BLACK);
    title(canvas, IDLE_TITLE, "PICO", pack(colors.primary));
    let mut region = canvas.slice(IDLE_SUBTITLE);
    font::draw_unscrolled(
        &mut region,
        "SCOREBOARD",
        Align::Center,
        Style::new(&generated::UNSCII_8, pack(colors.accent)),
    );
}

/// Nothing on the slate. Unlike [`idle`] this one carries toasts, because it is
/// a screen the user can be pressing buttons at.
pub fn no_games(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot, now: WallMs) {
    let colors = &snapshot.ui_colors;
    canvas.fill(BLACK);
    title(canvas, NO_GAMES_TITLE, "NO GAMES", pack(colors.primary));
    small(
        canvas,
        NO_GAMES_SUBTITLE,
        "scheduled",
        Align::Center,
        pack(colors.secondary),
    );
    toast::strip(canvas, snapshot, now);
    toast::overlay(canvas, snapshot, now);
}

/// The error card: a title and up to four pre-truncated detail lines.
pub fn error(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot) {
    let colors = &snapshot.ui_colors;
    let error = &snapshot.error;
    canvas.fill(BLACK);

    let heading = if error.title.is_empty() {
        "ERROR"
    } else {
        &error.title
    };
    title(canvas, ERROR_TITLE, heading, pack(colors.clock_warning));

    for (line, y) in error.lines.iter().zip(ERROR_LINE_Y) {
        small(
            canvas,
            rect(0, y, WIDTH, 8),
            line,
            Align::Center,
            pack(colors.primary),
        );
    }
}

/// AP-mode setup: what to join, where to go, and the QR that does both.
///
/// Text is drawn first and the QR blitted on top, so the QR stays readable even
/// where a long line ran under it. The lines whose vertical range meets the QR
/// are narrowed to stop four pixels short of its left edge; lines
/// entirely below it keep the full width. `display.Regions.update_for_qr` did
/// that narrowing on core 0 whenever the QR was regenerated — here the QR's size
/// is right there in the prepared view, so the widths are computed where they
/// are used.
///
/// The two upper lines scroll on the **wall rail**: they are low-stakes text on
/// a screen with no other motion, and the rail they ride is the one the
/// MicroPython screen used.
pub fn setup(
    canvas: &mut Canvas<'_>,
    snapshot: &ScoreboardSnapshot,
    prepared: &PreparedView,
    now: WallMs,
) {
    let colors = &snapshot.ui_colors;
    let setup = &snapshot.setup;
    canvas.fill(BLACK);

    let elapsed = now.since(snapshot.animation_start_ms);
    let failed = matches!(
        setup.reason,
        SetupReason::BadAuth | SetupReason::ConnectionFailed
    );
    let title_color = if failed {
        pack(colors.clock_warning)
    } else {
        pack(colors.accent)
    };

    let qr = prepared.qr();
    let width_for = |y: i32, height: i32| setup_line_width(y, height, qr.size());

    let mut heading = canvas.region(SETUP_PAD, 0, width_for(0, 16), 16);
    font::draw_unscrolled(
        &mut heading,
        &setup.title,
        Align::Left,
        Style::new(&generated::UNSCII_16, title_color),
    );

    let lines = [
        (18, &setup.line_18, pack(colors.primary), true),
        (28, &setup.line_28, pack(colors.secondary), true),
        (44, &setup.line_44, pack(colors.secondary), false),
        (54, &setup.line_54, pack(colors.accent), false),
    ];
    for (y, text, color, scrolls) in lines {
        let mut region = canvas.region(SETUP_PAD, y, width_for(y, 8), 8);
        font::draw(
            &mut region,
            text,
            Align::Left,
            if scrolls {
                elapsed.motion()
            } else {
                crate::time::Motion(0)
            },
            Style::new(&generated::SPLEEN_5X8, color),
            Scroll::DEFAULT,
        );
    }

    if !qr.is_empty() {
        canvas.blit(&qr.source(), WIDTH - qr.size() - QR_RIGHT_MARGIN, QR_TOP);
    }
}

/// How wide a setup line may be, given a QR of `qr_size` pixels in the top-right
/// corner. `qr_size` of 0 means no QR and full width for everything.
fn setup_line_width(y: i32, height: i32, qr_size: i32) -> i32 {
    let full = WIDTH - SETUP_PAD;
    if qr_size <= 0 {
        return full;
    }
    let qr_bottom = QR_TOP + qr_size;
    if y < qr_bottom && y + height > QR_TOP {
        let qr_x = WIDTH - qr_size - QR_RIGHT_MARGIN;
        (qr_x - QR_TEXT_GAP - SETUP_PAD).max(0)
    } else {
        full
    }
}

// -- Shared draws ------------------------------------------------------------

fn title(canvas: &mut Canvas<'_>, slot: Slice, text: &str, color: u16) {
    let mut region = canvas.slice(slot);
    font::draw_unscrolled(
        &mut region,
        text,
        Align::Center,
        Style::new(&generated::UNSCII_16, color),
    );
}

fn small(canvas: &mut Canvas<'_>, slot: Slice, text: &str, align: Align, color: u16) {
    let mut region = canvas.slice(slot);
    font::draw_unscrolled(
        &mut region,
        text,
        align,
        Style::new(&generated::SPLEEN_5X8, color),
    );
}

const fn rect(x: i32, y: i32, width: i32, height: i32) -> Slice {
    Slice {
        x,
        y,
        width,
        height,
    }
}

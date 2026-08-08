//! The league-select menu — a pure reader of the snapshot's menu view.
//!
//! `MenuController` (core 0) owns the whole session: the item list, the working
//! checkbox flags, the cursor, and the timeout. Everything drawn here was
//! decided there — the visible five-row window, the highlight index, and the
//! scrollbar thumb's y and height. This renderer draws two rectangles verbatim
//! and never computes scroll geometry.
//!
//! # The menu *is* the frame
//!
//! When the menu is active it preempts the mode dispatch entirely. Rotation,
//! poll commits and toasts continue underneath, invisible — toast drawing lives
//! inside the mode renderers the menu bypasses, so the suppression is
//! structural rather than a special case. Toast feedback under the menu is
//! therefore unavailable by design.
//!
//! # The marquee rides the wall rail
//!
//! The highlighted row's label scrolls from `menu.updated_ms`, which core 0
//! restamps only on open and on a highlight *change* — so toggling a checkbox
//! never jerks an in-progress scroll, and landing the cursor on the same item
//! at a different row does not restart it. Deriving the elapsed time
//! statelessly from that stamp is what lets the menu add no cross-frame state
//! to the render loop at all.

use crate::blit::Canvas;
use crate::font::{self, Align, Scroll, Style};
use crate::geometry::{HEIGHT, WIDTH};
use crate::time::WallMs;
use crate::{BLACK, DIM_GRAY, generated, pack};
use scoreboard_model::ScoreboardSnapshot;

// Geometry, inset 1 px from every panel edge: the edge pixels on this panel are
// unreliable, so nothing draws in row 0, row 63, column 0 or column 127.
// Five 10 px list rows (y 1..50), a rule at 52, and the DONE footer at 54..62.

/// First list row's top edge.
const TOP: i32 = 1;
const ROW_HEIGHT: i32 = 10;
const SEPARATOR_Y: i32 = 52;
/// Footer band 54..62; row 63 stays dark.
const DONE_Y: i32 = 54;
const CHECKBOX_X: i32 = 2;
const CHECKBOX_SIZE: i32 = 7;
/// The highlight bar starts after the checkbox (x 2..8) plus a 1 px gap, so a
/// highlighted row cannot invert the checkbox and make checked read as
/// unchecked.
const HIGHLIGHT_X: i32 = 10;
/// x 10..124 — stops before the scrollbar.
const HIGHLIGHT_WIDTH: i32 = 115;
/// 2 px scrollbar track at x 125..126.
const BAR_X: i32 = 125;

/// Label window: clear of the checkbox on the left, of the scrollbar on the
/// right.
///
/// 112 px is exactly fourteen `unscii_8` glyphs, which is exactly
/// "PREMIER LEAGUE" — zero margin. Narrowing this window makes the longest real
/// league label start marqueeing.
const LABEL_X: i32 = 12;
const LABEL_WIDTH: i32 = 112;

/// The cursor value that means the DONE footer rather than a list row.
const DONE_HIGHLIGHT: i8 = -1;

/// Draw the menu over the whole frame.
pub fn render(canvas: &mut Canvas<'_>, snapshot: &ScoreboardSnapshot, now: WallMs) {
    let menu = &snapshot.menu;
    let color = pack(snapshot.ui_colors.primary);
    let elapsed = now.since(menu.updated_ms);

    canvas.fill(BLACK);
    for (index, row) in menu.rows.iter().enumerate() {
        let y = TOP + index as i32 * ROW_HEIGHT;
        let selected = index as i8 == menu.highlight;
        if selected {
            canvas.fill_rect(HIGHLIGHT_X, y, HIGHLIGHT_WIDTH, ROW_HEIGHT, color);
        }
        let foreground = if selected { BLACK } else { color };

        canvas.rect(CHECKBOX_X, y + 1, CHECKBOX_SIZE, CHECKBOX_SIZE, color);
        if row.checked {
            canvas.fill_rect(CHECKBOX_X + 2, y + 3, 3, 3, color);
        }

        // Non-highlighted rows draw at offset 0 and clip inside their window:
        // the approved "truncate unless highlighted" behavior.
        let motion = if selected {
            elapsed.motion()
        } else {
            crate::time::Motion(0)
        };
        let mut label = canvas.region(LABEL_X, y + 1, LABEL_WIDTH, 8);
        font::draw(
            &mut label,
            &row.label,
            Align::Left,
            motion,
            Style::new(&generated::UNSCII_8, foreground),
            Scroll::DEFAULT,
        );
    }

    if menu.thumb_y >= 0 {
        canvas.fill_rect(BAR_X, TOP, 2, SEPARATOR_Y - TOP - 1, DIM_GRAY);
        canvas.fill_rect(BAR_X, menu.thumb_y as i32, 2, menu.thumb_h as i32, color);
    }

    canvas.hline(1, SEPARATOR_Y, WIDTH - 2, DIM_GRAY);
    let done_selected = menu.highlight == DONE_HIGHLIGHT;
    if done_selected {
        canvas.fill_rect(1, DONE_Y, WIDTH - 2, HEIGHT - 1 - DONE_Y, color);
    }
    // The footer's glyph cells are painted opaquely, so the label reads as a
    // knockout when the cursor is on it.
    let (ink, behind) = if done_selected {
        (BLACK, color)
    } else {
        (color, BLACK)
    };
    font::aligned_text(
        canvas,
        "DONE",
        0,
        DONE_Y + 1,
        WIDTH,
        Align::Center,
        Style::new(&generated::UNSCII_8, ink).on(behind),
    );
}

/// The scrollbar thumb.
///
/// `menu.py` computed this on core 0 and carried a comment saying its constants
/// "must mirror display.py's menu constants" — two copies of one geometry, kept
/// in step by hand. Here the track is drawn from the same constants the thumb is
/// computed from, so there is one copy: the menu controller (Phase 3) calls
/// [`thumb::compute`] and hands the result to `Store::set_menu`.
pub mod thumb {
    use super::{ROW_HEIGHT, SEPARATOR_Y, TOP};

    /// Top of the scrollbar track.
    pub const TRACK_Y0: i32 = TOP;
    /// Track height: the five list rows.
    pub const TRACK_H: i32 = 50;
    /// The thumb stops shrinking here and only slides.
    pub const MIN_THUMB_H: i32 = 4;
    /// Rows visible at once (`menu._VISIBLE_ROWS`).
    pub const VISIBLE_ROWS: usize = 5;

    const _: () = {
        assert!(TRACK_H == VISIBLE_ROWS as i32 * ROW_HEIGHT);
        assert!(TRACK_Y0 + TRACK_H <= SEPARATOR_Y);
    };

    /// Thumb position and height for a list of `item_count` scrolled to
    /// `scroll`, as `(thumb_y, thumb_h)`.
    ///
    /// A list that fits gets `(-1, 0)`, which is how the renderer knows to draw
    /// no scrollbar at all.
    pub fn compute(item_count: usize, scroll: usize) -> (i8, u8) {
        if item_count <= VISIBLE_ROWS {
            return (-1, 0);
        }
        let height = (TRACK_H * VISIBLE_ROWS as i32 / item_count as i32).max(MIN_THUMB_H);
        let travel = (TRACK_H - height) * scroll as i32 / (item_count - VISIBLE_ROWS) as i32;
        ((TRACK_Y0 + travel) as i8, height as u8)
    }
}

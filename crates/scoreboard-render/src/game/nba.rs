//! Live basketball: the quarter and clock ledger.
//!
//! The soccer-A silhouette adapted for a stop-clock sport. The identity column
//! is 4 px wider than soccer's because NBA scores reach three digits, and the
//! clock is the poll-time string drawn verbatim — never extrapolated, because a
//! stopped clock has no run signal to extrapolate from.
//!
//! The bottom strip carries only the shared claimants: NBA has no persistent
//! ticker, so between flashes it is empty, the way MLB's is between at-bats.

use super::Scene;
use crate::blit::Canvas;
use crate::font::{self, Align, Style};
use crate::geometry::NBA_LIVE;
use crate::{BLACK, WHITE, generated, pack, toast};
use scoreboard_model::UiColors;

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.nba_live;
    let table = NBA_LIVE;

    super::column_dividers(
        canvas,
        scene.settings,
        Some(table.divider_x),
        Some(table.separator_y),
    );
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    for (slot, score) in [
        (table.score_away, view.away_score),
        (table.score_home, view.home_score),
    ] {
        font::integer(
            canvas,
            score,
            slot.x,
            slot.y,
            slot.width,
            Align::Center,
            Style::new(&generated::UNSCII_16, WHITE),
        );
    }

    if !view.phase_text.is_empty() {
        let mut phase = canvas.slice(table.phase);
        font::draw_unscrolled(
            &mut phase,
            &view.phase_text,
            Align::Center,
            Style::new(&generated::UNSCII_8, WHITE),
        );
    }

    font::aligned_text(
        canvas,
        &view.clock_text,
        table.clock.x,
        table.clock.y,
        table.clock.width,
        Align::Center,
        Style::new(
            &generated::UNSCII_16,
            clock_color(&snapshot.ui_colors, view.clock_accent, view.clock_low),
        ),
    );

    super::bottom_strip(canvas, scene);
    toast::overlay(canvas, snapshot, scene.now);
}

/// The clock's color: accent while the game is between periods, warning when
/// the clock is about to run out, normal otherwise. Shared with football, which
/// follows the same conventions.
pub fn clock_color(colors: &UiColors, accent: bool, low: bool) -> u16 {
    if accent {
        pack(colors.accent)
    } else if low {
        pack(colors.clock_warning)
    } else {
        pack(colors.clock_normal)
    }
}

//! The line-score final screen, shared by baseball, basketball and football.
//!
//! Winner emphasis is by color and nothing else: the winning side's score and
//! total render in its team color, the loser's in gray, and neither side gets
//! an abbreviation.
//!
//! The three line-score rows are equal-length strings — three characters per
//! period, with `" X "` filling the columns a walk-off never played — so in a
//! fixed-width font they measure identically and scroll in lockstep off one
//! clock, with no extra mechanism. The totals column is pinned outside that
//! scroll.

use super::Scene;
use crate::blit::Canvas;
use crate::font::{self, Align, Style};
use crate::geometry::{EDGE_INSET, FINAL, FINAL_LINESCORE_SCROLL, HEIGHT};
use crate::{BLACK, generated, toast};
use scoreboard_model::Sport;

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.linescore_final;
    let table = FINAL;
    let secondary = snapshot.ui_colors.secondary;

    let (away_color, home_color) =
        super::winner_colors(view.home_won, view.away_color, view.home_color);

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    // The vertical rule separates the line score from the pinned totals, and
    // stops short of the panel's edge ring at both ends.
    canvas.vline(
        table.divider_x,
        EDGE_INSET,
        HEIGHT - 2 * EDGE_INSET,
        secondary,
    );

    super::chip(
        canvas,
        table.final_label,
        &view.final_text,
        snapshot.ui_colors.accent,
    );

    // One elapsed clock for all three rows: that is what keeps them locked.
    for (slot, row, color) in [
        (table.linescore_header, &view.header_row, secondary),
        (table.linescore_away, &view.away_row, away_color),
        (table.linescore_home, &view.home_row, home_color),
    ] {
        // Every row ends in its last column's trailing space. A row that fits
        // without it holds still: scrolling would only slide that blank into
        // view and push the first column's digits out. The three rows are
        // the same length in slots of the same width, so they decide alike.
        let trimmed = row.trim_end();
        let row = if font::measure(trimmed, &generated::SPLEEN_5X8) <= slot.width {
            trimmed
        } else {
            row.as_str()
        };
        let mut region = canvas.slice(slot);
        font::draw(
            &mut region,
            row,
            Align::Left,
            scene.view.motion(),
            Style::new(&generated::SPLEEN_5X8, color),
            FINAL_LINESCORE_SCROLL,
        );
    }

    let mut header = canvas.slice(table.total_header);
    font::draw_unscrolled(
        &mut header,
        total_label(view.sport),
        Align::Center,
        Style::new(&generated::SPLEEN_5X8, secondary),
    );
    // The totals column carries the headline score, so it takes the big-score
    // font.
    for (slot, score, color) in [
        (table.total_away, view.away_score, away_color),
        (table.total_home, view.home_score, home_color),
    ] {
        super::big_score(canvas, slot, score, color);
    }

    toast::overlay(canvas, snapshot, scene.now);
}

/// The pinned totals column's header: runs for baseball, points for the two
/// sports that score in points.
///
/// `state.py` passed this in as a literal beside a `variant_key` string; the
/// model replaced both with [`Sport`], so it is derived here.
const fn total_label(sport: Sport) -> &'static str {
    match sport {
        Sport::Mlb => "R",
        // Soccer and hockey never reach this screen — their finals are their
        // own shapes — but the enum has five values.
        Sport::Basketball | Sport::Hockey | Sport::Football | Sport::Soccer => "T",
    }
}

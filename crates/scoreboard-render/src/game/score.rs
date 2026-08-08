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
use crate::geometry::FINAL_LINESCORE_SCROLL;
use crate::{BLACK, DIM_GRAY, generated, pack, toast};
use scoreboard_model::Sport;

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.linescore_final;
    let table = scene.settings.final_table(view.sport);

    let (away_color, home_color) =
        super::winner_colors(view.home_won, pack(view.away_color), pack(view.home_color));

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    // The vertical rule separates the line score from the pinned totals; it
    // starts at the top-band separator when there is one, so it does not cut
    // through a logo sitting in a top corner.
    if scene.settings.show_dividers {
        let top = table.separator_y.unwrap_or(0);
        canvas.vline(
            table.divider_x,
            top,
            crate::geometry::HEIGHT - top,
            DIM_GRAY,
        );
        if let Some(y) = table.separator_y {
            canvas.hline(0, y, crate::geometry::WIDTH, DIM_GRAY);
        }
    }

    // The variants that carry big scores beside the logos; C puts the totals
    // in the pinned column instead.
    if let (Some(away_slot), Some(home_slot)) = (table.score_away, table.score_home) {
        super::big_score(canvas, away_slot, view.away_score, away_color);
        super::big_score(canvas, home_slot, view.home_score, home_color);
    }

    super::chip(
        canvas,
        table.final_label,
        &view.final_text,
        pack(snapshot.ui_colors.accent),
    );

    // One elapsed clock for all three rows: that is what keeps them locked.
    for (slot, row, color) in [
        (table.linescore_header, &view.header_row, DIM_GRAY),
        (table.linescore_away, &view.away_row, away_color),
        (table.linescore_home, &view.home_row, home_color),
    ] {
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
        Style::new(&generated::SPLEEN_5X8, DIM_GRAY),
    );
    for (slot, score, color) in [
        (table.total_away, view.away_score, away_color),
        (table.total_home, view.home_score, home_color),
    ] {
        // A 16 px slot is asking for the taller font: that is variant C, where
        // the totals column carries the headline score.
        let font = if slot.height >= 16 {
            &generated::UNSCII_16
        } else {
            &generated::SPLEEN_5X8
        };
        font::integer(
            canvas,
            score,
            slot.x,
            slot.y,
            slot.width,
            Align::Center,
            Style::new(font, color),
        );
    }

    toast::strip(canvas, snapshot, scene.now);
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
        // Soccer never reaches this screen — its full-time card is its own
        // shape — but the enum has four values.
        Sport::Nba | Sport::Football | Sport::Soccer => "T",
    }
}

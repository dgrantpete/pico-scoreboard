//! The upcoming-game screen — one design for every sport.
//!
//! Logos identify the teams, with stacked win/loss records beside them. The
//! right column carries the big first-pitch or kickoff time, one cycling info
//! line under it, and the per-team lines in team colors below that.
//!
//! Every phase of the cycle rides the **frame rail**, as a unit: a phase's
//! dwell is sized to one full scroll cycle of its own text
//! ([`PregameCycle`](crate::prepared::PregameCycle)), and keeping the dwell and
//! the scroll on the same clock matters more than either being exact. Two
//! different rails would let a line be swapped out mid-scroll after a stall.

use super::Scene;
use crate::blit::Canvas;
use crate::font::{self, Align, Style};
use crate::geometry::{PREGAME, PREGAME_INFO_DWELL_MS, PREGAME_SCROLL};
use crate::prepared::PregameLine;
use crate::{BLACK, DIM_GRAY, WHITE, generated, pack, toast};

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.pregame;
    let table = PREGAME;
    let elapsed = scene.view.0;

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);
    super::column_dividers(
        canvas,
        scene.settings,
        Some(table.divider_x),
        Some(table.separator_y),
    );

    // Records stack wins over losses. A side with no record advertised leaves
    // both slots blank rather than rendering a fake 0-0.
    for (side, wins_slot, losses_slot) in [
        (&view.away, table.record_away_wins, table.record_away_losses),
        (&view.home, table.record_home_wins, table.record_home_losses),
    ] {
        if let Some(record) = side.record {
            font::integer(
                canvas,
                record.wins,
                wins_slot.x,
                wins_slot.y,
                wins_slot.width,
                Align::Center,
                Style::new(&generated::SPLEEN_5X8, WHITE),
            );
            font::integer(
                canvas,
                record.losses,
                losses_slot.x,
                losses_slot.y,
                losses_slot.width,
                Align::Center,
                Style::new(&generated::SPLEEN_5X8, DIM_GRAY),
            );
        }
    }

    // The big slot: the start time, alternating one dwell each with the date
    // when the game is not today. The date leads, because it is the surprising
    // fact. An empty time means the device has no UTC offset, and a
    // wrong-timezone time is worse than none.
    if !view.time_text.is_empty() {
        let show_date =
            !view.date_text.is_empty() && (elapsed / PREGAME_INFO_DWELL_MS).is_multiple_of(2);
        let big = if show_date {
            &view.date_text
        } else {
            &view.time_text
        };
        let mut slot = canvas.slice(table.info_time);
        font::draw_unscrolled(
            &mut slot,
            big,
            Align::Center,
            Style::new(&generated::UNSCII_16, WHITE),
        );
    }

    // The cycling info line. Each phase scrolls from its own start, so the
    // scroll clock is the phase's elapsed time and not the view's.
    if let Some((line, phase_elapsed)) = scene.prepared.pregame().phase_at(elapsed) {
        let text = match line {
            PregameLine::Primary => view.info_primary.as_str(),
            PregameLine::Secondary => view.info_secondary.as_str(),
        };
        let mut slot = canvas.slice(table.info_cycle);
        font::draw(
            &mut slot,
            text,
            Align::Left,
            crate::time::Motion(phase_elapsed),
            Style::new(&generated::SPLEEN_5X8, WHITE),
            PREGAME_SCROLL,
        );
    }

    for (side, slot) in [
        (&view.away, table.team_line_away),
        (&view.home, table.team_line_home),
    ] {
        if side.line.is_empty() {
            continue;
        }
        let mut region = canvas.slice(slot);
        font::draw(
            &mut region,
            &side.line,
            Align::Left,
            scene.view.motion(),
            Style::new(&generated::SPLEEN_5X8, pack(side.color)),
            PREGAME_SCROLL,
        );
    }

    toast::strip(canvas, snapshot, scene.now);
    toast::overlay(canvas, snapshot, scene.now);
}

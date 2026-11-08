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
use crate::{BLACK, generated, scale, toast};
use scoreboard_model::Sport;

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.pregame;
    let table = PREGAME;
    let elapsed = scene.view.0;
    let colors = &snapshot.ui_colors;

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);
    super::column_dividers(canvas, scene, table.divider_x, table.separator_y);

    // Records stack wins over losses. A side with no record advertised leaves
    // both slots blank rather than rendering a fake 0-0. Hockey's records
    // have a third column, overtime losses, and its stack grows a third row
    // to fill the crest (NHL.md §3.9).
    let hockey = view.sport == Sport::Hockey;
    for (side, two_rows, three_rows) in [
        (
            &view.away,
            [table.record_away_wins, table.record_away_losses],
            table.hockey_record_away,
        ),
        (
            &view.home,
            [table.record_home_wins, table.record_home_losses],
            table.hockey_record_home,
        ),
    ] {
        let Some(record) = side.record else {
            continue;
        };
        let [wins_slot, losses_slot] = if hockey {
            [three_rows[0], three_rows[1]]
        } else {
            two_rows
        };
        let ot_losses = record.ot_losses.filter(|_| hockey);
        // Overtime losses take the losses' color at 5/8, a step dimmer: the
        // third column matters least, and the stack reads top-down by weight.
        for (slot, value, color) in [
            (wins_slot, Some(record.wins), colors.primary),
            (losses_slot, Some(record.losses), colors.secondary),
            (three_rows[2], ot_losses, scale(colors.secondary, 5, 8)),
        ] {
            let Some(value) = value else {
                continue;
            };
            font::integer(
                canvas,
                value,
                slot.x,
                slot.y,
                slot.width,
                Align::Center,
                Style::new(&generated::SPLEEN_5X8, color),
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
            Style::new(&generated::UNSCII_16, colors.accent),
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
            Style::new(&generated::SPLEEN_5X8, colors.secondary),
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
            Style::new(&generated::SPLEEN_5X8, side.color),
            PREGAME_SCROLL,
        );
    }

    toast::overlay(canvas, snapshot, scene.now);
}

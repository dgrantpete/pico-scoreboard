//! Live soccer and full time.
//!
//! The live screen shares the MLB silhouette; what changes is the data column,
//! because soccer is a running-clock sport. The full-time screen borrows the
//! final-C silhouette and puts goal scorers where a line score would be.

use super::{Scene, Strip};
use crate::blit::{Canvas, Slice};
use crate::font::{self, Align, Style};
use crate::geometry::{SOCCER_FINAL, SOCCER_SCROLL_PAUSE_MS};
use crate::time::WallMs;
use crate::{BLACK, DIM_GRAY, WHITE, generated, pack, toast};
use scoreboard_model::UiColors;
use scoreboard_model::snapshot::SoccerLiveView;

/// Every shipped font is fixed-width, so a composite clock ("45+6'") can be
/// centered with integer arithmetic instead of measuring it.
const CLOCK_GLYPH_WIDTH: i32 = 8;

/// The stoppage counter stops climbing here. Ninety-nine added minutes is
/// already impossible; the cap is against a bad anchor, not a long match.
const MAX_ADDED_MINUTES: u32 = 99;

pub fn render_live(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.soccer_live;
    let table = scene.settings.soccer_live_table();

    super::column_dividers(
        canvas,
        scene.settings,
        table.divider_x,
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

    // The period, in whichever form this variant asks for: a short chip in the
    // identity column, or spelled out under the clock.
    if let Some(slot) = table.phase.filter(|_| !view.phase_text.is_empty()) {
        let mut region = canvas.slice(slot);
        font::draw_unscrolled(
            &mut region,
            &view.phase_text,
            Align::Center,
            Style::new(&generated::UNSCII_8, WHITE),
        );
    }
    if let Some(slot) = table.phase_long.filter(|_| !view.phase_long.is_empty()) {
        let mut region = canvas.slice(slot);
        font::draw_unscrolled(
            &mut region,
            &view.phase_long,
            Align::Center,
            Style::new(&generated::UNSCII_8, DIM_GRAY),
        );
    }

    clock(canvas, table.clock, view, &snapshot.ui_colors, scene.now);

    if super::bottom_strip(canvas, scene) == Strip::Free {
        let scroll = scene.game_scroll(SOCCER_SCROLL_PAUSE_MS);
        if view.has_event {
            let color = pack(view.event_color);
            let mut top = canvas.slice(table.event_top);
            font::draw(
                &mut top,
                &view.event_top,
                Align::Center,
                scene.view.motion(),
                Style::new(&generated::SPLEEN_5X8, color),
                scroll,
            );
            if !view.event_name.is_empty() {
                let mut name = canvas.slice(table.event_name);
                font::draw(
                    &mut name,
                    &view.event_name,
                    Align::Center,
                    scene.view.motion(),
                    Style::new(&generated::UNSCII_8, color),
                    scroll,
                );
            }
        } else {
            // Nothing in the ticker yet: a dim placeholder, so the strip does
            // not read as a rendering hole.
            let mut empty = canvas.slice(table.event_empty);
            font::draw_unscrolled(
                &mut empty,
                "NO GOALS",
                Align::Center,
                Style::new(&generated::SPLEEN_5X8, DIM_GRAY),
            );
        }
    }

    toast::overlay(canvas, snapshot, scene.now);
}

/// The match clock, extrapolated from the poll-time anchor.
///
/// # Why this rides the wall rail
///
/// The match clock is real time, not motion. A stalled frame must *consume*
/// match time — the opposite of a scroll, which must hold position. Both the
/// anchor and `now` are wall stamps, so the clock ticks between polls with no
/// core-0 involvement, a core-0 stall can neither freeze nor jump it, and every
/// poll re-anchors whatever drift accumulated.
///
/// # Why the minutes floor
///
/// ESPN's `displayClock` floors, and the fixture evidence is decisive: "45'+6'"
/// with halftime immediately after means six *full* stoppage minutes were
/// played. So 23:30 elapsed reads "23'", and at the period's base minute — the
/// whole of 45:00 through 45:59 — it still reads "45'". Past that the clock
/// holds the base and counts the added minutes in the warning color.
fn clock(
    canvas: &mut Canvas<'_>,
    slot: Slice,
    view: &SoccerLiveView,
    colors: &UiColors,
    now: WallMs,
) {
    if view.on_break {
        // Classic halftime reads "HT"; later breaks — the extra-time interval,
        // the end of regulation — read "BREAK". The period's base minute is
        // what tells them apart.
        let label = if view.base_min == 45 { "HT" } else { "BREAK" };
        font::aligned_text(
            canvas,
            label,
            slot.x,
            slot.y,
            slot.width,
            Align::Center,
            Style::new(&generated::UNSCII_16, pack(colors.accent)),
        );
        return;
    }

    let mut elapsed_s = view.clock_anchor_s as u64;
    if view.clock_running {
        elapsed_s += now.since(view.clock_anchor_ms).0 / 1000;
    }
    let minute = (elapsed_s / 60) as u32;
    let base = view.base_min as u32;

    if minute <= base {
        let digits = if minute < 10 {
            1
        } else if minute < 100 {
            2
        } else {
            3
        };
        let mut cursor = slot.x + (slot.width - (digits + 1) * CLOCK_GLYPH_WIDTH) / 2;
        let style = Style::new(&generated::UNSCII_16, pack(colors.clock_normal));
        cursor = font::integer(canvas, minute as u16, cursor, slot.y, 0, Align::Left, style);
        font::text_into(canvas, "'", cursor, slot.y, style);
    } else {
        let added = (minute - base).min(MAX_ADDED_MINUTES);
        let digits = if base < 100 { 2 } else { 3 } + if added < 10 { 1 } else { 2 } + 2;
        let mut cursor = slot.x + (slot.width - digits * CLOCK_GLYPH_WIDTH) / 2;
        let style = Style::new(&generated::UNSCII_16, pack(colors.clock_warning));
        cursor = font::integer(canvas, base as u16, cursor, slot.y, 0, Align::Left, style);
        cursor = font::text_into(canvas, "+", cursor, slot.y, style);
        cursor = font::integer(canvas, added as u16, cursor, slot.y, 0, Align::Left, style);
        font::text_into(canvas, "'", cursor, slot.y, style);
    }
}

/// Full time: the score, the result label, and who scored.
///
/// A level score colors both sides — soccer draws are real results, not a
/// missing winner — so this screen cannot reuse the shared winner-emphasis
/// helper.
pub fn render_final(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.soccer_final;
    let table = SOCCER_FINAL;

    let (away_color, home_color) = if view.draw {
        (pack(view.away_color), pack(view.home_color))
    } else {
        super::winner_colors(view.home_won, pack(view.away_color), pack(view.home_color))
    };

    if scene.settings.show_dividers {
        canvas.vline(table.divider_x, 0, crate::geometry::HEIGHT, DIM_GRAY);
    }
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    for (slot, score, color) in [
        (table.score_away, view.away_score, away_color),
        (table.score_home, view.home_score, home_color),
    ] {
        font::integer(
            canvas,
            score,
            slot.x,
            slot.y,
            slot.width,
            Align::Center,
            Style::new(&generated::UNSCII_16, color),
        );
    }

    let mut label = canvas.slice(table.full_time_label);
    font::draw_unscrolled(
        &mut label,
        &view.ft_text,
        Align::Center,
        Style::new(&generated::UNSCII_8, pack(snapshot.ui_colors.accent)),
    );

    let scroll = scene.game_scroll(SOCCER_SCROLL_PAUSE_MS);
    for (slot, scorers, color) in [
        (table.scorers_away, &view.scorers_away, away_color),
        (table.scorers_home, &view.scorers_home, home_color),
    ] {
        if scorers.is_empty() {
            continue;
        }
        let mut region = canvas.slice(slot);
        font::draw(
            &mut region,
            scorers,
            Align::Center,
            scene.view.motion(),
            Style::new(&generated::SPLEEN_5X8, color),
            scroll,
        );
    }

    toast::strip(canvas, snapshot, scene.now);
    toast::overlay(canvas, snapshot, scene.now);
}

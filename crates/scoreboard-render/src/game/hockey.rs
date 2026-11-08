//! Live hockey and the three-stars final (NHL.md §6.5).
//!
//! The live screen is the rink (D1): football's broadcast corners over a
//! top-down rink that carries every goal where it was scored. It has a
//! shootout form, which takes over whenever the model carries a shootout, and
//! with it the clock's place for the tally (§3.2). The final is its own screen
//! (D2), soccer's full-time silhouette with the three stars where the scorers
//! would be.
//!
//! # Projection
//!
//! The model carries goal coordinates in feet — semantics — and [`crate::rink`]
//! owns the pixels: a 200 × 85 ft sheet squashed into the 124 × 15 px strip
//! under the shots bar. [`mark_at`] maps one to the other per frame, as
//! football's renderer maps a yardline, through the same `x_at` and `dy` that
//! place the rink's lines — a few integer operations, so a goal on the goal
//! line lands on the goal line.
//!
//! # No renderer state
//!
//! The goal lamp, the newest mark's blink and the power-play zone's glow are
//! pure functions of snapshot anchors on the wall rail — the lamp and blink of
//! `lamp_ms`, the glow of `animation_start_ms` — so nothing here remembers a
//! frame. The goal marks composite over the pixels under them (D9), read back
//! from the canvas inside the one draw that put them there.

use super::{Scene, Strip};
use crate::blit::{Canvas, Slice};
use crate::font::{self, Align, FontFace, Style};
use crate::geometry::{
    self, EDGE_INSET, HOCKEY_FINAL, HOCKEY_LIVE, HOCKEY_SCROLL_PAUSE_MS, HOCKEY_STARS_SCROLL,
    HockeyLiveGeometry, SHOOTOUT_MARK, SHOOTOUT_PITCH,
};
use crate::rink::{self, Rink};
use crate::time::WallMs;
use crate::widgets::{arrow, count_dots};
use crate::{BLACK, Rgb888, WHITE, count_dot, generated, mix, scale, star, toast};
use scoreboard_model::UiColors;
use scoreboard_model::snapshot::{
    BannerKind, GoalMark, HockeyLiveView, ShootoutRow, ShootoutView, ShotMark, Side,
};

/// Goal-lamp red, what the scoring side's number pulses toward (D4).
const LAMP: Rgb888 = Rgb888::new(255, 24, 24);
/// How long the lamp burns after a goal (D4). A duration, so it is measured
/// on the wall rail: a stalled frame spends lamp time like any other.
const LAMP_MS: u64 = 20_000;
/// One primary ↔ lamp-red cycle — MLB's critical-count pulse, re-aimed.
const LAMP_PULSE_MS: u64 = 1_000;
/// While the lamp burns the newest goal's mark blinks opaque white, on for
/// this long and off for as long.
const LAMP_BLINK_MS: u64 = 400;
/// One goal mark's opacity (D9): 141/256 over whatever is lit, so one goal
/// reads 55 % of the team color, two stacked 80 %, three 91 % — a heat map
/// rather than a blob.
const GOAL_MARK_ALPHA: u32 = 141;
/// The power-play zone's glow: one dim ↔ bright cycle.
const GLOW_PULSE_MS: u64 = 1_600;
/// A missed shootout attempt's X.
const MISS: Rgb888 = Rgb888::new(255, 10, 10);
/// Each side's count cell at the ends of the shots ledger: two digits and a
/// pixel either side.
const SHOTS_COUNT_WIDTH: i32 = 14;

pub fn render_live(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.hockey_live;
    let table = HOCKEY_LIVE;
    let primary = snapshot.ui_colors.primary;
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    if let Some(shootout) = &view.shootout {
        // The shootout's form (§3.2): the tally in the clock's slot, its label
        // on the situation line, each side's attempts level with its score,
        // and the rink squashed under them.
        identity(
            canvas,
            view,
            primary,
            table.score_away,
            table.score_home,
            table.period,
            None,
        );
        tally(
            canvas,
            table.clock,
            shootout,
            "-",
            Style::new(&generated::UNSCII_16, snapshot.ui_colors.accent),
        );
        line(
            canvas,
            table.situation,
            "SHOOTOUT",
            &generated::SPLEEN_5X8,
            snapshot.ui_colors.secondary,
        );
        shootout_row(
            canvas,
            table.shootout_away,
            &shootout.away,
            view.away_color,
            &snapshot.ui_colors,
        );
        shootout_row(
            canvas,
            table.shootout_home,
            &shootout.home,
            view.home_color,
            &snapshot.ui_colors,
        );
        short_rink(canvas, view, table.shootout_rink_y);
    } else {
        skater_dots(canvas, scene, view, &table);
        identity(
            canvas,
            view,
            primary,
            table.score_away,
            table.score_home,
            table.period,
            Some(scene.now),
        );
        clock(canvas, scene, table.clock);
        situation(canvas, scene, view, table.situation);
        shots(canvas, scene, table.shots, table.shots_bar_y);

        if super::bottom_strip(canvas, scene) == Strip::Free {
            rink(canvas, scene, view);
        }
    }

    toast::overlay(canvas, snapshot, scene.now);
}

/// Up to six dots under each crest, the first `n` filled in the team's color
/// and the row padded to five — so a short-handed side shows its empty slot —
/// with an extra attacker's sixth in the accent color. Unknown counts draw
/// nothing: no summary, or the backend's play-strength fallback.
fn skater_dots(
    canvas: &mut Canvas<'_>,
    scene: &Scene<'_>,
    view: &HockeyLiveView,
    table: &HockeyLiveGeometry,
) {
    let pitch = count_dot::SIZE + 1;
    let five = Slice {
        x: 0,
        y: table.skaters_y,
        width: 5 * pitch - 1,
        height: count_dot::SIZE,
    };
    let accent = scene.snapshot.ui_colors.accent;
    for (side, count, color) in [
        (Side::Away, view.away_skaters, view.away_color),
        (Side::Home, view.home_skaters, view.home_color),
    ] {
        if count == 0 {
            continue;
        }
        let count = count.min(6);
        let width = count.max(5) as i32 * pitch - 1;
        let x = match side {
            Side::Away => table.skaters_away_x,
            Side::Home => table.skaters_home_end - width + 1,
        };
        count_dots(canvas, Slice { x, ..five }, count.min(5), Some(color));
        if count > 5 {
            let extra = Slice {
                x: x + 5 * pitch,
                width: pitch - 1,
                ..five
            };
            count_dots(canvas, extra, 1, Some(accent));
        }
    }
}

/// The one line over the shots: the banner — its title and an arrow toward
/// the side it is about, or a matched count — else the newest goal and its
/// scorer.
fn situation(canvas: &mut Canvas<'_>, scene: &Scene<'_>, view: &HockeyLiveView, slot: Slice) {
    let face = &generated::SPLEEN_5X8;
    if let Some(banner) = view.banner {
        let (title, side) = match banner.kind {
            BannerKind::PowerPlay(side) => ("POWER PLAY", side),
            BannerKind::EmptyNet(side) => ("EMPTY NET", side),
            BannerKind::Matched => {
                skater_count(
                    canvas,
                    slot,
                    banner.attackers,
                    banner.defenders,
                    Style::new(face, scene.snapshot.ui_colors.primary),
                );
                return;
            }
        };
        let color = side_color(view, side);
        line(canvas, slot, title, face, color);
        let width = font::measure(title, face);
        let text_x = slot.x + (slot.width - width) / 2;
        let right = side == Side::Home;
        let x = if right {
            text_x + width + 3
        } else {
            text_x - 6
        };
        arrow(canvas, x, slot.y + 1, right, color);
        return;
    }
    if let Some(side) = view.last_goal {
        let mut region = canvas.slice(slot);
        font::draw(
            &mut region,
            &view.goal_line,
            Align::Center,
            scene.view.motion(),
            Style::new(face, side_color(view, side)),
            scene.game_scroll(HOCKEY_SCROLL_PAUSE_MS),
        );
    }
}

// -- The rink ------------------------------------------------------------------

/// The live screen's rink: its rectangle and its projection.
const RINK: Rink = rink::STRIP;

/// The top-left pixel of a goal's 2 × 2 mark, whose bottom-right pixel is the
/// projected point. The model's feet are from center ice with the away side
/// attacking +x (§3.5).
pub fn mark_at(mark: GoalMark) -> (i32, i32) {
    let x = RINK.x_at(rink::CENTER_FT + mark.x as i32) - 1;
    let y = RINK.mid() + RINK.dy(mark.y as i32) - 1;
    (x, y)
}

/// Each team's end, tinted a third of the team's color: enough to say whose
/// net is whose without outshining the goals plotted there.
fn end_tints(view: &HockeyLiveView) -> (Rgb888, Rgb888) {
    (scale(view.away_color, 1, 3), scale(view.home_color, 1, 3))
}

/// The rink strip: the ice and lines, the advantaged side's attacking zone
/// glowing during a power play or an empty net, then every goal's mark in
/// play order, the newest blinking while the lamp burns.
fn rink(canvas: &mut Canvas<'_>, scene: &Scene<'_>, view: &HockeyLiveView) {
    let (away_end, home_end) = end_tints(view);
    RINK.draw(canvas, away_end, home_end);

    let advantaged = view.banner.and_then(|banner| match banner.kind {
        BannerKind::PowerPlay(side) | BannerKind::EmptyNet(side) => Some(side),
        BannerKind::Matched => None,
    });
    if let Some(side) = advantaged {
        let since = scene.now.since(scene.snapshot.animation_start_ms).0;
        let glow = scale(
            side_color(view, side),
            64 + crate::pulse(since, GLOW_PULSE_MS) * 96 / 256,
            256,
        );
        // The zone the side attacks — away attacks right — from inside its
        // far blue line to just short of the goal line, a row in from the
        // boards.
        let (from, to) = match side {
            Side::Away => (
                rink::LENGTH_FT - rink::BLUE_LINE_FT,
                rink::LENGTH_FT - rink::GOAL_LINE_FT,
            ),
            Side::Home => (rink::GOAL_LINE_FT, rink::BLUE_LINE_FT),
        };
        let (left, right) = (RINK.x_at(from) + 2, RINK.x_at(to) - 1);
        canvas.fill_rect(left, RINK.y + 2, right - left + 1, RINK.height - 4, glow);
        // The lines and creases the glow covered come back on top of it.
        RINK.draw_lines(canvas);
    }

    for mark in &view.marks {
        let (x, y) = mark_at(*mark);
        let color = side_color(view, mark.side);
        for (px, py) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
            let under = canvas.pixel_at(px, py).unwrap_or(BLACK);
            canvas.pixel(px, py, mix(under, color, GOAL_MARK_ALPHA));
        }
    }
    if let (Some((_, since)), true, Some(newest)) =
        (lamp(view, scene.now), view.newest_marked, view.marks.last())
        && (since / LAMP_BLINK_MS).is_multiple_of(2)
    {
        let (x, y) = mark_at(*newest);
        canvas.fill_rect(x, y, 2, 2, WHITE);
    }
}

/// The rows of the rink that make the shootout's nine-row rink: the top three
/// and the bottom three, which carry the rounded corners, and the three
/// through the creases. The mockup's shorter rink is exactly these rows, so
/// the shootout draws the same rink, three rows at a time.
const SHORT_RINK_BANDS: [i32; 3] = [0, 6, 12];
const SHORT_RINK_BAND: i32 = 3;

const _: () = {
    assert!(SHORT_RINK_BANDS[2] + SHORT_RINK_BAND == RINK.height);
    assert!(SHORT_RINK_BANDS[1] + SHORT_RINK_BAND / 2 == RINK.height / 2);
};

/// Each band is a three-row region with the rink drawn into it shifted up by
/// the band's first row; the region clips the rest.
fn short_rink(canvas: &mut Canvas<'_>, view: &HockeyLiveView, y: i32) {
    let (away_end, home_end) = end_tints(view);
    for (index, from) in SHORT_RINK_BANDS.into_iter().enumerate() {
        let mut band = canvas.region(
            0,
            y + index as i32 * SHORT_RINK_BAND,
            geometry::WIDTH,
            SHORT_RINK_BAND,
        );
        Rink { y: -from, ..RINK }.draw(&mut band, away_end, home_end);
    }
}

// -- What the live screen's two forms share --------------------------------------

/// The scores and the period chip. A lamp is passed only where it may burn:
/// the shootout's tied score never lights.
fn identity(
    canvas: &mut Canvas<'_>,
    view: &HockeyLiveView,
    primary: Rgb888,
    away: Slice,
    home: Slice,
    period: Slice,
    lamp_at: Option<WallMs>,
) {
    for (side, slot, score) in [
        (Side::Away, away, view.away_score),
        (Side::Home, home, view.home_score),
    ] {
        let color = match lamp_at.and_then(|now| lamp(view, now)) {
            Some((lit, since)) if lit == side => {
                mix(primary, LAMP, crate::pulse(since, LAMP_PULSE_MS))
            }
            _ => primary,
        };
        super::big_score(canvas, slot, score, color);
    }
    if !view.period_text.is_empty() {
        super::chip(canvas, period, &view.period_text, primary);
    }
}

/// The side the lamp burns for and how long it has, while its 20 s run.
fn lamp(view: &HockeyLiveView, now: WallMs) -> Option<(Side, u64)> {
    let side = view.lamp_side?;
    let since = now.since(view.lamp_ms).0;
    (since < LAMP_MS).then_some((side, since))
}

fn clock(canvas: &mut Canvas<'_>, scene: &Scene<'_>, slot: Slice) {
    let view = &scene.snapshot.hockey_live;
    font::aligned_text(
        canvas,
        &view.clock_text,
        slot.x,
        slot.y,
        slot.width,
        Align::Center,
        Style::new(
            &generated::UNSCII_16,
            super::nba::clock_color(&scene.snapshot.ui_colors, view.clock_accent, view.clock_low),
        ),
    );
}

/// The shots ledger: each side's count at its end, the label between, and a
/// bar under them split in proportion. Absent shots hide the row (§3.3).
fn shots(canvas: &mut Canvas<'_>, scene: &Scene<'_>, slot: Slice, bar_y: i32) {
    let (view, colors) = (&scene.snapshot.hockey_live, &scene.snapshot.ui_colors);
    let Some(shots) = view.shots else {
        return;
    };
    let face = &generated::SPLEEN_5X8;
    for (x, count) in [
        (slot.x, shots.away),
        (slot.x + slot.width - SHOTS_COUNT_WIDTH, shots.home),
    ] {
        font::integer(
            canvas,
            count as u16,
            x,
            slot.y,
            SHOTS_COUNT_WIDTH,
            Align::Center,
            Style::new(face, colors.primary),
        );
    }
    line(canvas, slot, "SHOTS", face, colors.secondary);

    let (away, home) = (shots.away as i32, shots.home as i32);
    // A pixel's gap at the split; level shots — 0-0 included — split evenly.
    let split = if away + home == 0 {
        slot.x + (slot.width - 1) / 2
    } else {
        slot.x + (slot.width - 1) * away / (away + home)
    };
    canvas.fill_rect(slot.x, bar_y, split - slot.x, 2, view.away_color);
    canvas.fill_rect(
        split + 1,
        bar_y,
        slot.x + slot.width - split - 1,
        2,
        view.home_color,
    );
}

/// "5 ON 4": a banner's skaters, advantaged side first. A count the feed could
/// not take leaves the line out rather than print a 0.
fn skater_count(
    canvas: &mut Canvas<'_>,
    slot: Slice,
    attackers: u8,
    defenders: u8,
    style: Style<'_>,
) {
    if attackers == 0 || defenders == 0 {
        return;
    }
    numbers(
        canvas,
        slot,
        "",
        attackers as u16,
        " ON ",
        defenders as u16,
        style,
    );
}

/// The shootout's tally, each side's shootout goals: the number that moves
/// while the game score holds the tie.
fn tally(
    canvas: &mut Canvas<'_>,
    slot: Slice,
    shootout: &ShootoutView,
    between: &str,
    style: Style<'_>,
) {
    numbers(
        canvas,
        slot,
        "",
        shootout.away.tally as u16,
        between,
        shootout.home.tally as u16,
        style,
    );
}

/// One side's attempts, oldest first: a solid square for a goal, a red X for
/// a miss, a ring around a pip for the side shooting next, and a bare ring for
/// a round of the three still to come.
fn shootout_row(
    canvas: &mut Canvas<'_>,
    slot: Slice,
    row: &ShootoutRow,
    color: Rgb888,
    colors: &UiColors,
) {
    let size = SHOOTOUT_MARK;
    for (index, mark) in row.marks.iter().enumerate() {
        let (x, y) = (slot.x + index as i32 * SHOOTOUT_PITCH, slot.y);
        match mark {
            ShotMark::Scored => canvas.fill_rect(x, y, size, size, color),
            ShotMark::Missed => {
                for step in 0..size {
                    canvas.pixel(x + step, y + step, MISS);
                    canvas.pixel(x + size - 1 - step, y + step, MISS);
                }
            }
            ShotMark::Next => {
                let next = colors.primary;
                canvas.rect(x, y, size, size, next);
                canvas.fill_rect(x + 2, y + 2, size - 4, size - 4, next);
            }
            ShotMark::Pending => canvas.rect(x, y, size, size, colors.secondary),
        }
    }
}

// -- The final -------------------------------------------------------------------

/// Three stars (D2): crests and scores in winner color and loser gray, the
/// result label, each star's name in its team's color, a rule, then the
/// series and the shots.
///
/// The scores, colors, label and line score are the shared final's, which
/// the model commits beside the stars (§3.10). The stars arrive minutes after
/// the horn, and until they do their band carries that line score — so the
/// screen reads as a finished final throughout, never as a stars screen with
/// its stars missing.
pub fn render_final(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let shared = &snapshot.linescore_final;
    let view = &snapshot.hockey_final;
    let table = HOCKEY_FINAL;
    let colors = &snapshot.ui_colors;
    let (away_color, home_color) =
        super::winner_colors(shared.home_won, shared.away_color, shared.home_color);

    let rule = colors.secondary;
    canvas.vline(
        table.divider_x,
        EDGE_INSET,
        geometry::HEIGHT - 2 * EDGE_INSET,
        rule,
    );
    let start = table.divider_x + 1;
    canvas.hline(
        start,
        table.rule_y,
        geometry::WIDTH - EDGE_INSET - start,
        rule,
    );
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);
    super::big_score(canvas, table.score_away, shared.away_score, away_color);
    super::big_score(canvas, table.score_home, shared.home_score, home_color);
    super::chip(canvas, table.final_label, &shared.final_text, colors.accent);

    let face = &generated::SPLEEN_5X8;
    match &view.stars {
        Some(stars) => {
            for (rank, (line, slot)) in stars.iter().zip(table.star_rows).enumerate() {
                for count in 0..=rank as i32 {
                    star::draw(canvas, table.star_x + count * table.star_pitch, slot.y + 1);
                }
                // A star's name is in its own team's color, winner or not.
                let color = match line.side {
                    Some(Side::Away) => shared.away_color,
                    Some(Side::Home) => shared.home_color,
                    None => colors.primary,
                };
                let mut region = canvas.slice(slot);
                font::draw(
                    &mut region,
                    &line.name,
                    Align::Left,
                    scene.view.motion(),
                    Style::new(face, color),
                    HOCKEY_STARS_SCROLL,
                );
            }
        }
        None => {
            for (slot, row, color) in [
                (
                    table.linescore_rows[0],
                    &shared.header_row,
                    colors.secondary,
                ),
                (table.linescore_rows[1], &shared.away_row, away_color),
                (table.linescore_rows[2], &shared.home_row, home_color),
            ] {
                // Five columns at most, so the rows always fit; trimmed alike,
                // they center alike.
                line(canvas, slot, row.trim_end(), face, color);
            }
        }
    }

    let series = (!view.series.is_empty()).then_some(view.series.as_str());
    let (series_slot, shots_slot) = match (series, view.shots) {
        (Some(_), Some(_)) => (table.series, table.shots),
        _ => (table.single_line, table.single_line),
    };
    if let Some(series) = series {
        let mut region = canvas.slice(series_slot);
        font::draw(
            &mut region,
            series,
            Align::Center,
            scene.view.motion(),
            Style::new(face, colors.secondary),
            HOCKEY_STARS_SCROLL,
        );
    }
    if let Some(shots) = view.shots {
        numbers(
            canvas,
            shots_slot,
            "SHOTS ",
            shots.away as u16,
            "-",
            shots.home as u16,
            Style::new(face, colors.secondary),
        );
    }

    toast::overlay(canvas, snapshot, scene.now);
}

// -- Primitives ----------------------------------------------------------------

/// A static line centered in its slot, clipped to it.
fn line(canvas: &mut Canvas<'_>, slot: Slice, text: &str, face: &FontFace, color: Rgb888) {
    let mut region = canvas.slice(slot);
    font::draw_unscrolled(&mut region, text, Align::Center, Style::new(face, color));
}

/// `{prefix}{a}{between}{b}`, centered in `slot` as one line, drawn without
/// building a string: "5 ON 4", "0 - 1", "SHOTS 33-35".
fn numbers(
    canvas: &mut Canvas<'_>,
    slot: Slice,
    prefix: &str,
    a: u16,
    between: &str,
    b: u16,
    style: Style<'_>,
) {
    let face = style.font;
    let width = font::measure(prefix, face)
        + font::digits_width(a, face)
        + font::measure(between, face)
        + font::digits_width(b, face);
    let mut x = slot.x + (slot.width - width).div_euclid(2);
    x = font::text_into(canvas, prefix, x, slot.y, style);
    x = font::integer(canvas, a, x, slot.y, 0, Align::Left, style);
    x = font::text_into(canvas, between, x, slot.y, style);
    font::integer(canvas, b, x, slot.y, 0, Align::Left, style);
}

fn side_color(view: &HockeyLiveView, side: Side) -> Rgb888 {
    match side {
        Side::Away => view.away_color,
        Side::Home => view.home_color,
    }
}

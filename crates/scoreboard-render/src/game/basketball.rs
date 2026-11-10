//! Live basketball — the court (NBA.md D1, the B5 layout; NCAAB.md D5's
//! arena court) — and the game-leaders final (D2).
//!
//! # The live screen
//!
//! Football's broadcast corners as one 24-row band: crests, scores, the
//! period over the clock, and — in a league with a ledger — each score
//! column a team ledger, five foul dots over the score and seven timeout
//! blocks under it; a league without one (NCAAB.md D3) draws no tracks and
//! centres its scores on the crests. Below the band the court carries every
//! made field goal where it was made, translucent so the paint grades into a
//! heat map (D5), and one caption slot on the floor says the one thing worth
//! saying (D7): a run while it lasts, else the newest score while it is
//! news, else BONUS while a side is in it, else bare floor.
//!
//! # Projection
//!
//! The model carries a make in the wire's feet — `x` from the centre line,
//! the home side's already negated (§3.4), `y` from the backboard plane, the
//! rim at 1 in every league — and [`crate::court`] owns the pixels: `along =
//! y + ORIGIN`, `across = x`, at the side's own basket ([`End::from`]),
//! through the frame's [`Camera`]. Nothing here knows how many pixels a foot
//! is, or which rule book the lines follow: the view's court picks the
//! raster.
//!
//! # No renderer state
//!
//! The run glow, the newest mark's blink and the caption's news window are
//! pure functions of snapshot anchors on the wall rail — the glow of
//! `animation_start_ms`, the blink and the window of `score_ms` — so nothing
//! here remembers a frame. The marks composite over the pixels under them,
//! read back from the canvas inside the one draw that puts them there
//! (hockey's D9 arrangement).

use super::{Scene, Strip};
use crate::blit::{Canvas, Slice};
use crate::court::{self, Camera, CourtRaster, End, ORIGIN};
use crate::font::{self, Align, Style};
use crate::geometry::{
    self, BASKETBALL_CAPTION_PAUSE_MS, BASKETBALL_FINAL, BASKETBALL_LIVE, BasketballLiveGeometry,
    EDGE_INSET, FINAL_LINESCORE_SCROLL, STARS_SCROLL,
};
use crate::widgets::{arrow, count_dots, timeout_blocks};
use crate::{BLACK, Rgb888, WHITE, generated, mix, toast};
use scoreboard_model::UiColors;
use scoreboard_model::snapshot::{BasketballLiveView, CourtMark, Record, Side};

/// A side's timeouts at the tip, and so the blocks in its row.
const TIMEOUTS: u8 = 7;
/// A burned timeout's block.
const BURNED: Rgb888 = Rgb888::new(40, 40, 40);
/// The dots a foul row shows: the fifth team foul of a quarter is the one
/// that puts the other side in the bonus.
const FOUL_DOTS: u8 = 5;
/// A mark's opacity (D5): 96/256 over whatever is lit, so one make reads
/// 37 % of the team colour, two stacked 61 %, five 90 % — hockey's heat map
/// at a lower alpha, for forty makes a side.
const MARK_ALPHA: u32 = 96;
/// The run the running side's score breathes from (§3.5); the wire shows a
/// run from six.
const RUN_GLOW: u8 = 8;
/// One primary ↔ team-colour cycle of the breathing score, and how far
/// toward the team colour it reaches at the peak (200/256): the goal lamp's
/// mechanism, aimed at momentum instead of a goal.
const GLOW_PULSE_MS: u64 = 1_200;
const GLOW_DEPTH: u32 = 200;
/// While the newest score is news its mark blinks opaque white, on for this
/// long and off for as long.
const BLINK_MS: u64 = 400;
/// The leaders final's three rows, in the order the model commits them.
const LEADER_LABELS: [&str; 3] = ["PTS", "REB", "AST"];

pub fn render_live(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.basketball_live;
    let table = BASKETBALL_LIVE;
    let colors = &snapshot.ui_colors;
    let primary = colors.primary;

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);
    ledger(canvas, view, colors, &table);
    scores(canvas, scene, view, primary, &table);
    if !view.phase_text.is_empty() {
        super::chip(canvas, table.period, &view.phase_text, primary);
    }
    font::aligned_text(
        canvas,
        &view.clock_text,
        table.clock.x,
        table.clock.y,
        table.clock.width,
        Align::Center,
        Style::new(
            &generated::SPLEEN_5X8,
            clock_color(colors, view.clock_accent, view.clock_low),
        ),
    );

    if super::bottom_strip(canvas, scene) == Strip::Free {
        court(canvas, scene, view, &table);
    }
    toast::overlay(canvas, snapshot, scene.now);
}

/// The clock's color: accent while the game is between periods, warning when
/// the clock is about to run out, normal otherwise. Shared with football and
/// hockey, which follow the same conventions.
pub fn clock_color(colors: &UiColors, accent: bool, low: bool) -> Rgb888 {
    if accent {
        colors.accent
    } else if low {
        colors.clock_warning
    } else {
        colors.clock_normal
    }
}

/// Each side's foul dots and timeout blocks (§3.3). The dots count the
/// side's team fouls this quarter, the row in the warning colour when the
/// opponent is in the bonus — this side has no fouls to give — else the team
/// colour; the blocks count the timeouts left, burned ones dark. No ledger,
/// no dots and no blocks: a first look without its summary shows the band
/// bare rather than a fake seven. A league without tracks draws none at all.
fn ledger(
    canvas: &mut Canvas<'_>,
    view: &BasketballLiveView,
    colors: &UiColors,
    table: &BasketballLiveGeometry,
) {
    if !view.ledger_tracks {
        return;
    }
    let Some(ledger) = view.ledger else {
        return;
    };
    let warning = colors.clock_warning;
    for (side, opponent, dots, blocks, color) in [
        (
            ledger.away,
            ledger.home,
            table.fouls_away,
            table.timeouts_away,
            view.away_color,
        ),
        (
            ledger.home,
            ledger.away,
            table.fouls_home,
            table.timeouts_home,
            view.home_color,
        ),
    ] {
        let tint = if opponent.in_bonus { warning } else { color };
        count_dots(canvas, dots, side.fouls.min(FOUL_DOTS), Some(tint));
        timeout_blocks(
            canvas,
            blocks,
            TIMEOUTS,
            side.timeouts.min(TIMEOUTS),
            color,
            BURNED,
        );
    }
}

/// Both scores in the primary — or, for a side on a run of [`RUN_GLOW`] or
/// more, breathing between the primary and its team colour on the wall rail
/// from the view's anchor (§3.5), as the goal lamp pulses. Between the tracks
/// in a league with a ledger, else centred on the crests (NCAAB.md D3).
fn scores(
    canvas: &mut Canvas<'_>,
    scene: &Scene<'_>,
    view: &BasketballLiveView,
    primary: Rgb888,
    table: &BasketballLiveGeometry,
) {
    let since = scene.now.since(scene.snapshot.animation_start_ms).0;
    let (away_slot, home_slot) = if view.ledger_tracks {
        (table.score_away, table.score_home)
    } else {
        (table.score_away_centred, table.score_home_centred)
    };
    for (side, slot, score, team) in [
        (Side::Away, away_slot, view.away_score, view.away_color),
        (Side::Home, home_slot, view.home_score, view.home_color),
    ] {
        let color = match view.run {
            Some(run) if run.side == side && run.points >= RUN_GLOW => mix(
                primary,
                team,
                crate::pulse(since, GLOW_PULSE_MS) * GLOW_DEPTH / 256,
            ),
            _ => primary,
        };
        super::big_score(canvas, slot, score, color);
    }
}

/// The floor and everything on it, in §3.11's order: the league's court,
/// every mark in play order, the newest blinking while its score is news,
/// the caption, and the hoops over all of it.
fn court(
    canvas: &mut Canvas<'_>,
    scene: &Scene<'_>,
    view: &BasketballLiveView,
    table: &BasketballLiveGeometry,
) {
    let raster = court::raster(view.court);
    let camera = Camera::new(&table.court);
    let (away, home) = (view.away_color, view.home_color);
    raster.draw_floor(canvas, away, home);
    for mark in &view.marks {
        let color = match mark.side {
            Side::Away => away,
            Side::Home => home,
        };
        camera.plot_over(
            canvas,
            End::from(mark.side),
            along(mark),
            across(mark),
            color,
            MARK_ALPHA,
        );
    }
    let news = news(view, scene);
    if let (Some(since), true, Some(newest)) = (news, view.newest_marked, view.marks.last())
        && (since / BLINK_MS).is_multiple_of(2)
    {
        camera.plot(
            canvas,
            End::from(newest.side),
            along(newest),
            across(newest),
            WHITE,
        );
    }
    caption(canvas, scene, view, news.is_some(), raster, table);
    court::draw_hoops(canvas);
}

/// Feet along the court from the attacked baseline: the wire's `y = 1` is
/// the rim, [`ORIGIN`] feet in (§3.4).
fn along(mark: &CourtMark) -> f32 {
    mark.y as f32 + ORIGIN
}

/// Feet across from the centre line, as the model already rotated them.
fn across(mark: &CourtMark) -> f32 {
    mark.x as f32
}

/// How long the newest score has been news, while it is (§3.6): inside the
/// window the prepared view sized, on the wall rail from the news stamp. A
/// first look stamps none.
fn news(view: &BasketballLiveView, scene: &Scene<'_>) -> Option<u64> {
    view.last_score?;
    if view.score_ms == 0 {
        return None;
    }
    let since = scene.now.since(view.score_ms).0;
    (since < scene.prepared.news_window_ms()).then_some(since)
}

/// The one line on the floor (D7): the run while it lasts, else the newest
/// scorer while the score is news, else BONUS while a side is in it, else
/// nothing. Each is painted on bare floor first, which erases the half-court
/// line and any mark under it but stops at the three-point arcs; nothing is
/// painted when there is nothing to say.
///
/// The run and BONUS carry football's arrow at the slot's edge nearest the
/// side's crest; the scorer's line does not — it scrolls through the whole
/// slot, and reads its side from its colour.
fn caption(
    canvas: &mut Canvas<'_>,
    scene: &Scene<'_>,
    view: &BasketballLiveView,
    news: bool,
    raster: &CourtRaster,
    table: &BasketballLiveGeometry,
) {
    let slot = table.caption;
    let face = &generated::SPLEEN_5X8;
    let team = |side| match side {
        Side::Away => view.away_color,
        Side::Home => view.home_color,
    };
    if let Some(run) = view.run {
        raster.clear_caption(canvas);
        let color = team(run.side);
        let style = Style::new(face, color);
        // "N-0 RUN", the number drawn in place.
        let suffix = "-0 RUN";
        let width = font::digits_width(run.points as u16, face) + font::measure(suffix, face);
        let x = slot.x + (slot.width - width) / 2;
        let x = font::integer(canvas, run.points as u16, x, slot.y, 0, Align::Left, style);
        font::text_into(canvas, suffix, x, slot.y, style);
        edge_arrow(canvas, slot, run.side, color);
        return;
    }
    if news
        && !view.caption.is_empty()
        && let Some(side) = view.last_score
    {
        raster.clear_caption(canvas);
        let mut region = canvas.slice(slot);
        // The scroll counts from the news stamp, so the window the prepared
        // view sized — one full cycle at least — is the one the line scrolls
        // through, and a long name is read once in full.
        font::draw(
            &mut region,
            &view.caption,
            Align::Center,
            scene.now.since(view.score_ms).motion(),
            Style::new(face, team(side)),
            scene.game_scroll(BASKETBALL_CAPTION_PAUSE_MS),
        );
        return;
    }
    let Some(ledger) = view.ledger else {
        return;
    };
    let (away, home) = (ledger.away.in_bonus, ledger.home.in_bonus);
    if !(away || home) {
        return;
    }
    raster.clear_caption(canvas);
    // One side shooting takes its colour; both, the primary, with each
    // side's arrow in its own.
    let color = match (away, home) {
        (true, false) => team(Side::Away),
        (false, true) => team(Side::Home),
        _ => scene.snapshot.ui_colors.primary,
    };
    let mut region = canvas.slice(slot);
    font::draw_unscrolled(&mut region, "BONUS", Align::Center, Style::new(face, color));
    if away {
        edge_arrow(canvas, slot, Side::Away, team(Side::Away));
    }
    if home {
        edge_arrow(canvas, slot, Side::Home, team(Side::Home));
    }
}

/// Football's 3 × 5 triangle at the slot's edge nearest `side`'s crest,
/// pointing at it.
fn edge_arrow(canvas: &mut Canvas<'_>, slot: Slice, side: Side, color: Rgb888) {
    let (x, right) = match side {
        Side::Away => (slot.x, false),
        Side::Home => (slot.x + slot.width - 3, true),
    };
    arrow(canvas, x, slot.y + 1, right, color);
}

// -- The final -------------------------------------------------------------------

/// Game leaders (D2): crests and scores in winner colour and loser gray, the
/// result label, the points, rebounds and assists leaders each in their own
/// team's colour, a rule, then the series line in the playoffs and both
/// records.
///
/// The scores, colours, label and quarter rows are the shared final's, which
/// the model commits beside the leaders (§3.9). The leaders ride the final
/// body itself, but a handful of bodies never carry them; until they do the
/// band carries the quarter rows, so the screen reads as a finished final
/// throughout, never as a leaders screen with its leaders missing.
pub fn render_final(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let shared = &snapshot.linescore_final;
    let view = &snapshot.basketball_final;
    let table = BASKETBALL_FINAL;
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
    match &view.leaders {
        Some(leaders) => {
            for ((line, slot), label) in leaders.iter().zip(table.leader_rows).zip(LEADER_LABELS) {
                font::text_into(
                    canvas,
                    label,
                    table.leader_label_x,
                    slot.y,
                    Style::new(face, rule),
                );
                // A leader's line is in their own team's colour, winner or not.
                let color = match line.side {
                    Side::Away => shared.away_color,
                    Side::Home => shared.home_color,
                };
                let mut region = canvas.slice(slot);
                font::draw(
                    &mut region,
                    &line.text,
                    Align::Left,
                    scene.view.motion(),
                    Style::new(face, color),
                    STARS_SCROLL,
                );
            }
        }
        None => {
            // The quarter rows, as the line-score final draws them: held
            // still while they fit, else scrolled in lockstep.
            for (slot, row, color) in [
                (table.linescore_rows[0], &shared.header_row, rule),
                (table.linescore_rows[1], &shared.away_row, away_color),
                (table.linescore_rows[2], &shared.home_row, home_color),
            ] {
                let trimmed = row.trim_end();
                let row = if font::measure(trimmed, face) <= slot.width {
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
                    Style::new(face, color),
                    FINAL_LINESCORE_SCROLL,
                );
            }
        }
    }

    let series = (!view.series.is_empty()).then_some(view.series.as_str());
    let records_slot = if series.is_some() {
        table.records
    } else {
        table.records_alone
    };
    if let Some(series) = series {
        let mut region = canvas.slice(table.series);
        font::draw(
            &mut region,
            series,
            Align::Center,
            scene.view.motion(),
            Style::new(face, colors.primary),
            STARS_SCROLL,
        );
    }
    if let Some([away, home]) = view.records {
        record(canvas, records_slot, away, false, shared.away_color);
        record(canvas, records_slot, home, true, shared.home_color);
    }

    toast::overlay(canvas, snapshot, scene.now);
}

/// "W-L" at the start of the records' row, or at its end for the home side,
/// in the team's colour — the numbers drawn in place, no string built.
fn record(canvas: &mut Canvas<'_>, slot: Slice, record: Record, at_end: bool, color: Rgb888) {
    let face = &generated::SPLEEN_5X8;
    let style = Style::new(face, color);
    let dash = "-";
    let width = font::digits_width(record.wins, face)
        + font::measure(dash, face)
        + font::digits_width(record.losses, face);
    let mut x = if at_end {
        slot.x + slot.width - width
    } else {
        slot.x
    };
    x = font::integer(canvas, record.wins, x, slot.y, 0, Align::Left, style);
    x = font::text_into(canvas, dash, x, slot.y, style);
    font::integer(canvas, record.losses, x, slot.y, 0, Align::Left, style);
}

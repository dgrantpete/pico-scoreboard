//! Live hockey and the three-stars final (NHL.md §6.5).
//!
//! Two live designs (D1): B "Rink", the default — football's broadcast corners
//! over a top-down rink that carries every goal where it was scored — and A
//! "Period ledger", the MLB/NBA silhouette. Each has a shootout form, which
//! takes over whenever the model carries a shootout, and with it the clock's
//! place for the tally (§3.2). The final is its own screen (D2), soccer's
//! full-time silhouette with the three stars where the scorers would be.
//!
//! # Projection
//!
//! The model carries goal coordinates in feet — semantics — and the rink art
//! in `hockey_layout.aseprite` is a 200 × 85 ft sheet squashed into its
//! 124 × 15 px `POSITION`. [`mark_at`] maps one to the other here, per frame,
//! as football's renderer maps a yardline: a few integer operations against
//! the art's own rectangle.
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
use crate::generated::layout;
use crate::geometry::{
    self, EDGE_INSET, HOCKEY_FINAL, HOCKEY_LIVE_A, HOCKEY_LIVE_B, HOCKEY_SCROLL_PAUSE_MS,
    HOCKEY_STARS_SCROLL, HockeyLedgerGeometry, HockeyLiveVariant, HockeyRinkGeometry,
    SHOOTOUT_MARK, SHOOTOUT_PITCH,
};
use crate::time::WallMs;
use crate::widgets::count_dots;
use crate::{BLACK, DIM_GRAY, MAGENTA, WHITE, generated, pack, rgb565, toast};
use scoreboard_model::snapshot::{
    BannerKind, GoalMark, HockeyLiveView, ShootoutRow, ShootoutView, ShotMark, Side,
};

/// Goal-lamp red, what the scoring side's number pulses toward (D4).
const LAMP: u16 = rgb565(255, 24, 24);
/// How long the lamp burns after a goal (D4). A duration, so it is measured
/// on the wall rail: a stalled frame spends lamp time like any other.
const LAMP_MS: u64 = 20_000;
/// One white ↔ lamp-red cycle — MLB's critical-count pulse, re-aimed.
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
const MISS: u16 = rgb565(255, 10, 10);
/// Each side's count cell at the ends of the shots ledger: two digits and a
/// pixel either side.
const SHOTS_COUNT_WIDTH: i32 = 14;

pub fn render_live(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    match scene.settings.hockey_live {
        HockeyLiveVariant::A => ledger(canvas, scene, HOCKEY_LIVE_A),
        HockeyLiveVariant::B => rink_screen(canvas, scene, HOCKEY_LIVE_B),
    }
    toast::overlay(canvas, scene.snapshot, scene.now);
}

// -- A, "Period ledger" ---------------------------------------------------------

fn ledger(canvas: &mut Canvas<'_>, scene: &Scene<'_>, table: HockeyLedgerGeometry) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.hockey_live;

    if let Some(shootout) = &view.shootout {
        // No rule under the data column: the attempt rows sit level with the
        // crests, across where it would run.
        super::column_dividers(canvas, scene.settings, Some(table.divider_x), None);
        super::both_logos(canvas, scene, table.logo_away, table.logo_home);
        identity(
            canvas,
            view,
            table.score_away,
            table.score_home,
            table.period,
            None,
        );
        shootout_row(
            canvas,
            table.shootout_away,
            &shootout.away,
            pack(view.away_color),
        );
        shootout_row(
            canvas,
            table.shootout_home,
            &shootout.home,
            pack(view.home_color),
        );
        super::chip(
            canvas,
            table.shootout_label,
            "SHOOTOUT",
            pack(snapshot.ui_colors.accent),
        );
        tally(
            canvas,
            table.shootout_tally,
            shootout,
            " - ",
            Style::new(&generated::SPLEEN_5X8, WHITE),
        );
        return;
    }

    super::column_dividers(
        canvas,
        scene.settings,
        Some(table.divider_x),
        Some(table.separator_y),
    );
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);
    identity(
        canvas,
        view,
        table.score_away,
        table.score_home,
        table.period,
        Some(scene.now),
    );
    clock(canvas, scene, table.clock);
    shots(canvas, view, table.shots, table.shots_bar_y);

    if super::bottom_strip(canvas, scene) == Strip::Free {
        ledger_strip(canvas, scene, view, &table);
    }
}

/// The strength banner, else the newest goal over its scorer, else — only
/// while the game is scoreless — a placeholder. A scored game whose goals are
/// missing (the summary failed, ruling 3) leaves the strip dark rather than
/// claim there are none.
fn ledger_strip(
    canvas: &mut Canvas<'_>,
    scene: &Scene<'_>,
    view: &HockeyLiveView,
    table: &HockeyLedgerGeometry,
) {
    if let Some(banner) = view.banner {
        let (title, color, side) = match banner.kind {
            BannerKind::PowerPlay(side) => ("POWER PLAY", side_color(view, side), Some(side)),
            BannerKind::EmptyNet(side) => ("EMPTY NET", side_color(view, side), Some(side)),
            BannerKind::Matched => ("EVEN STRENGTH", DIM_GRAY, None),
        };
        line(
            canvas,
            table.strip_top,
            title,
            &generated::SPLEEN_5X8,
            color,
        );
        let count_color = if side.is_some() { color } else { WHITE };
        skater_count(
            canvas,
            table.strip_bottom,
            banner.attackers,
            banner.defenders,
            Style::new(&generated::UNSCII_8, count_color),
        );
        if let Some(side) = side {
            for x in table.banner_arrows_x {
                pointer(canvas, x, table.banner_arrows_y, side == Side::Away, color);
            }
        }
        return;
    }

    if let Some(side) = view.last_goal {
        let color = side_color(view, side);
        let scroll = scene.game_scroll(HOCKEY_SCROLL_PAUSE_MS);
        for (slot, text, face) in [
            (
                table.strip_top,
                view.goal_label.as_str(),
                &generated::SPLEEN_5X8,
            ),
            (table.strip_bottom, view.scorer(), &generated::UNSCII_8),
        ] {
            let mut region = canvas.slice(slot);
            font::draw(
                &mut region,
                text,
                Align::Center,
                scene.view.motion(),
                Style::new(face, color),
                scroll,
            );
        }
    } else if view.away_score == 0 && view.home_score == 0 {
        line(
            canvas,
            table.strip_empty,
            "NO GOALS",
            &generated::SPLEEN_5X8,
            DIM_GRAY,
        );
    }
}

/// One of the two arrows flanking a banner's count, pointing at the crest it
/// is about: up to the away side's, down to the home side's.
fn pointer(canvas: &mut Canvas<'_>, x: i32, y: i32, up: bool, color: u16) {
    let (sprite, mut palette) = if up {
        (layout::inning_top::SPRITE, layout::inning_top::PALETTE)
    } else {
        (
            layout::inning_bottom::SPRITE,
            layout::inning_bottom::PALETTE,
        )
    };
    palette[1] = color;
    canvas.blit(&sprite.tinted(&palette), x, y);
}

// -- B, "Rink" -----------------------------------------------------------------

fn rink_screen(canvas: &mut Canvas<'_>, scene: &Scene<'_>, table: HockeyRinkGeometry) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.hockey_live;
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    if let Some(shootout) = &view.shootout {
        identity(
            canvas,
            view,
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
            Style::new(&generated::UNSCII_16, pack(snapshot.ui_colors.accent)),
        );
        line(
            canvas,
            table.situation,
            "SHOOTOUT",
            &generated::SPLEEN_5X8,
            DIM_GRAY,
        );
        shootout_row(
            canvas,
            table.shootout_away,
            &shootout.away,
            pack(view.away_color),
        );
        shootout_row(
            canvas,
            table.shootout_home,
            &shootout.home,
            pack(view.home_color),
        );
        short_rink(canvas, view, table.shootout_rink_y);
        return;
    }

    skater_dots(canvas, scene, view, &table);
    identity(
        canvas,
        view,
        table.score_away,
        table.score_home,
        table.period,
        Some(scene.now),
    );
    clock(canvas, scene, table.clock);
    situation(canvas, scene, view, table.situation);
    shots(canvas, view, table.shots, table.shots_bar_y);

    if super::bottom_strip(canvas, scene) == Strip::Free {
        rink(canvas, scene, view);
    }
}

/// Up to six dots under each crest, the first `n` filled in the team's color
/// and the row padded to five — so a short-handed side shows its empty slot —
/// with an extra attacker's sixth in the accent color. Unknown counts draw
/// nothing: no summary, or the backend's play-strength fallback.
fn skater_dots(
    canvas: &mut Canvas<'_>,
    scene: &Scene<'_>,
    view: &HockeyLiveView,
    table: &HockeyRinkGeometry,
) {
    let pitch = layout::dot::SPRITE.width + 1;
    let five = Slice {
        x: 0,
        y: table.skaters_y,
        width: 5 * pitch - 1,
        height: layout::dot::SPRITE.height,
    };
    let accent = pack(scene.snapshot.ui_colors.accent);
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
        count_dots(canvas, Slice { x, ..five }, count.min(5), Some(pack(color)));
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
                    Style::new(face, WHITE),
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
        super::football::arrow(canvas, x, slot.y + 1, right, color);
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

const RINK: Slice = layout::rink::POSITION;

/// The rink's length and width in feet: the sheet the art squashes.
const RINK_LENGTH_FT: i32 = 200;
const RINK_WIDTH_FT: i32 = 85;

/// The art's placeholder ends, found by *value* — `compile_layout` assigns
/// palette indices in first-seen order, so an art edit can reorder them — and
/// at build time, so drifted art fails `cargo build`. Football's endzones work
/// the same way. The ice is found too: the glow redraws the art with it
/// see-through.
const AWAY_END: usize = rink_index(rgb565(255, 0, 0));
const HOME_END: usize = rink_index(rgb565(0, 0, 255));
const ICE: usize = rink_index(rgb565(14, 22, 34));

const fn rink_index(color: u16) -> usize {
    let palette = &layout::rink::PALETTE;
    let mut index = 0;
    while index < palette.len() {
        if palette[index] == color {
            return index;
        }
        index += 1;
    }
    panic!("rink palette: a placeholder color is missing");
}

/// x of a position along the rink, in feet from the left boards — the art's
/// own mapping (`tools/hockey_layout.lua`).
const fn rink_x(feet: i32) -> i32 {
    RINK.x + (RINK.width - 1) * feet / RINK_LENGTH_FT
}

/// The top-left pixel of a goal's 2 × 2 mark. The model's feet are from
/// center ice with the away side attacking +x (§3.5), and the y scale leaves
/// the boards' rows out of the sheet's width.
pub fn mark_at(mark: GoalMark) -> (i32, i32) {
    let x = rink_x(RINK_LENGTH_FT / 2 + mark.x as i32) - 1;
    let y = RINK.y + RINK.height / 2 - mark.y as i32 * (RINK.height - 3) / RINK_WIDTH_FT - 1;
    (x, y)
}

/// The art with each team's end tinted a third of the team's color: enough to
/// say whose net is whose without outshining the goals plotted there.
fn rink_palette(view: &HockeyLiveView) -> [u16; layout::rink::PALETTE.len()] {
    let mut palette = layout::rink::PALETTE;
    palette[AWAY_END] = scale(pack(view.away_color), 1, 3);
    palette[HOME_END] = scale(pack(view.home_color), 1, 3);
    palette
}

/// The rink strip: the art, the advantaged side's attacking zone glowing
/// during a power play or an empty net, then every goal's mark in play order,
/// the newest blinking while the lamp burns.
fn rink(canvas: &mut Canvas<'_>, scene: &Scene<'_>, view: &HockeyLiveView) {
    let sprite = layout::rink::SPRITE;
    let palette = rink_palette(view);
    canvas.blit(&sprite.tinted(&palette), RINK.x, RINK.y);

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
        let (blue_line, goal_line) = match side {
            Side::Away => (125, 189),
            Side::Home => (11, 75),
        };
        let (left, right) = (rink_x(blue_line) + 2, rink_x(goal_line) - 1);
        canvas.fill_rect(left, RINK.y + 2, right - left + 1, RINK.height - 4, glow);
        // The lines and creases the glow covered come back: the art again,
        // with its ice and ends see-through.
        let mut lines = palette;
        for index in [ICE, AWAY_END, HOME_END] {
            lines[index] = MAGENTA;
        }
        canvas.blit(&sprite.tinted(&lines), RINK.x, RINK.y);
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

/// The rows of the rink art that make the shootout's nine-row rink: the top
/// three and the bottom three, which carry the rounded corners, and the three
/// through the creases. The mockup's shorter rink is exactly these rows, so
/// the shootout costs no second sprite.
const SHORT_RINK_BANDS: [i32; 3] = [0, 6, 12];
const SHORT_RINK_BAND: i32 = 3;

const _: () = {
    assert!(SHORT_RINK_BANDS[2] + SHORT_RINK_BAND == RINK.height);
    assert!(SHORT_RINK_BANDS[1] + SHORT_RINK_BAND / 2 == RINK.height / 2);
};

fn short_rink(canvas: &mut Canvas<'_>, view: &HockeyLiveView, y: i32) {
    let palette = rink_palette(view);
    let source = layout::rink::SPRITE.tinted(&palette);
    for (index, from) in SHORT_RINK_BANDS.into_iter().enumerate() {
        let mut band = canvas.region(
            0,
            y + index as i32 * SHORT_RINK_BAND,
            geometry::WIDTH,
            SHORT_RINK_BAND,
        );
        band.blit(&source, RINK.x, -from);
    }
}

// -- What both layouts share ----------------------------------------------------

/// The scores and the period chip. A lamp is passed only where it may burn:
/// the shootout's tied score never lights.
fn identity(
    canvas: &mut Canvas<'_>,
    view: &HockeyLiveView,
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
                mix(WHITE, LAMP, crate::pulse(since, LAMP_PULSE_MS))
            }
            _ => WHITE,
        };
        super::big_score(canvas, slot, score, color);
    }
    if !view.period_text.is_empty() {
        super::chip(canvas, period, &view.period_text, WHITE);
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
fn shots(canvas: &mut Canvas<'_>, view: &HockeyLiveView, slot: Slice, bar_y: i32) {
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
            Style::new(face, WHITE),
        );
    }
    line(canvas, slot, "SHOTS", face, DIM_GRAY);

    let (away, home) = (shots.away as i32, shots.home as i32);
    // A pixel's gap at the split; level shots — 0-0 included — split evenly.
    let split = if away + home == 0 {
        slot.x + (slot.width - 1) / 2
    } else {
        slot.x + (slot.width - 1) * away / (away + home)
    };
    canvas.fill_rect(slot.x, bar_y, split - slot.x, 2, pack(view.away_color));
    canvas.fill_rect(
        split + 1,
        bar_y,
        slot.x + slot.width - split - 1,
        2,
        pack(view.home_color),
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
/// a miss, a white ring around a pip for the side shooting next, and a gray
/// ring for a round of the three still to come.
fn shootout_row(canvas: &mut Canvas<'_>, slot: Slice, row: &ShootoutRow, color: u16) {
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
                canvas.rect(x, y, size, size, WHITE);
                canvas.fill_rect(x + 2, y + 2, size - 4, size - 4, WHITE);
            }
            ShotMark::Pending => canvas.rect(x, y, size, size, DIM_GRAY),
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
    let (away_color, home_color) = super::winner_colors(
        shared.home_won,
        pack(shared.away_color),
        pack(shared.home_color),
    );

    if scene.settings.show_dividers {
        canvas.vline(
            table.divider_x,
            EDGE_INSET,
            geometry::HEIGHT - 2 * EDGE_INSET,
            DIM_GRAY,
        );
        let start = table.divider_x + 1;
        canvas.hline(
            start,
            table.rule_y,
            geometry::WIDTH - EDGE_INSET - start,
            DIM_GRAY,
        );
    }
    super::both_logos(canvas, scene, table.logo_away, table.logo_home);
    super::big_score(canvas, table.score_away, shared.away_score, away_color);
    super::big_score(canvas, table.score_home, shared.home_score, home_color);
    super::chip(
        canvas,
        table.final_label,
        &shared.final_text,
        pack(snapshot.ui_colors.accent),
    );

    let face = &generated::SPLEEN_5X8;
    match &view.stars {
        Some(stars) => {
            let star = layout::star::SPRITE.source();
            for (rank, (line, slot)) in stars.iter().zip(table.star_rows).enumerate() {
                for count in 0..=rank as i32 {
                    canvas.blit(&star, table.star_x + count * table.star_pitch, slot.y + 1);
                }
                // A star's name is in its own team's color, winner or not.
                let color = match line.side {
                    Some(Side::Away) => pack(shared.away_color),
                    Some(Side::Home) => pack(shared.home_color),
                    None => WHITE,
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
                (table.linescore_rows[0], &shared.header_row, DIM_GRAY),
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
            Style::new(face, WHITE),
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
            Style::new(face, DIM_GRAY),
        );
    }

    toast::overlay(canvas, snapshot, scene.now);
}

// -- Primitives ----------------------------------------------------------------

/// A static line centered in its slot, clipped to it.
fn line(canvas: &mut Canvas<'_>, slot: Slice, text: &str, face: &FontFace, color: u16) {
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
        + digits_width(a, face)
        + font::measure(between, face)
        + digits_width(b, face);
    let mut x = slot.x + (slot.width - width).div_euclid(2);
    x = font::text_into(canvas, prefix, x, slot.y, style);
    x = font::integer(canvas, a, x, slot.y, 0, Align::Left, style);
    x = font::text_into(canvas, between, x, slot.y, style);
    font::integer(canvas, b, x, slot.y, 0, Align::Left, style);
}

fn digits_width(value: u16, face: &FontFace) -> i32 {
    let digits = value.checked_ilog10().map_or(1, |log| log + 1) as i32;
    digits * face.digit(0).width
}

fn side_color(view: &HockeyLiveView, side: Side) -> u16 {
    pack(match side {
        Side::Away => view.away_color,
        Side::Home => view.home_color,
    })
}

/// `over` composited onto `under` at `alpha`/256, through 8-bit channels.
pub fn mix(under: u16, over: u16, alpha: u32) -> u16 {
    let ([under_r, under_g, under_b], [over_r, over_g, over_b]) = (expand(under), expand(over));
    let blend = |under: u32, over: u32| ((under * (256 - alpha) + over * alpha) / 256) as u8;
    rgb565(
        blend(under_r, over_r),
        blend(under_g, over_g),
        blend(under_b, over_b),
    )
}

fn expand(color: u16) -> [u32; 3] {
    let (red, green, blue) = (
        (color >> 11) as u32 & 0x1F,
        (color >> 5) as u32 & 0x3F,
        color as u32 & 0x1F,
    );
    [red * 255 / 31, green * 255 / 63, blue * 255 / 31]
}

/// A packed color's brightness scaled by `num`/`den`, per 565 channel.
fn scale(color: u16, num: u32, den: u32) -> u16 {
    let red = ((color >> 11) & 0x1F) as u32 * num / den;
    let green = ((color >> 5) & 0x3F) as u32 * num / den;
    let blue = (color & 0x1F) as u32 * num / den;
    ((red.min(0x1F) as u16) << 11) | ((green.min(0x3F) as u16) << 5) | blue.min(0x1F) as u16
}

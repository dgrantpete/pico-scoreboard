//! Live football: broadcast corners over a perspective field strip.
//!
//! # Where the projection happens
//!
//! `state.py` projected the yardlines on core 0 and shipped four x coordinates
//! in the snapshot, so core 1 "only draws precomputed segments". That was a
//! MicroPython budget decision, not a modelling one: the projection is a
//! handful of integer operations, and the model carries the *situation* —
//! down, distance, yardline, possession — as semantics. [`Field::project`] does
//! the arithmetic here, per frame, where the pixel format already lives.
//!
//! The mapping itself is unchanged: 100 yards at 1 px/yard between the goal
//! lines, with the sprite's endzone blocks outside that span, and vertical
//! lines leaning toward a vanishing point above the panel.

use super::{Scene, Strip};
use crate::blit::Canvas;
use crate::font::{self, Align, Style};
use crate::generated::art;
use crate::geometry::{self, FOOTBALL_LIVE};
use crate::widgets::{arrow, timeout_blocks};
use crate::{BLACK, DIM_GRAY, Rgb888, generated, toast};
use scoreboard_model::snapshot::Side;
use scoreboard_model::snapshot::{FieldSituation, FootballLiveView};

/// Scrimmage navy, from the pre-rewrite palette.
const SCRIMMAGE: Rgb888 = Rgb888::new(0, 0, 140);
/// First-down yellow. It wins where the two lines meet, by drawing second.
const FIRST_DOWN: Rgb888 = Rgb888::new(255, 255, 0);
/// A side's timeouts per half, and so the blocks in its row.
const TIMEOUTS: u8 = 3;

/// The endzone blocks ship as pure red (away) and pure blue (home) placeholders.
/// Their palette indices are found by *value*, because the art compiler numbers
/// colours in first-seen order and a PNG edit can reorder them. MicroPython
/// discovered them at import and raised there; a `const fn` moves that to build
/// time, so drifted art fails `cargo build` rather than a boot.
const AWAY_ENDZONE: usize = endzone_index(Rgb888::new(255, 0, 0));
const HOME_ENDZONE: usize = endzone_index(Rgb888::new(0, 0, 255));

const fn endzone_index(placeholder: Rgb888) -> usize {
    let palette = &art::football_field::PALETTE;
    let mut index = 0;
    while index < palette.len() {
        if let Some(color) = palette[index]
            && color.0 == placeholder.0
        {
            return index;
        }
        index += 1;
    }
    panic!("football field palette: an endzone placeholder color is missing");
}

const FIELD_TOP_Y: i32 = FOOTBALL_LIVE.field.y;
const FIELD_BOTTOM_Y: i32 = FIELD_TOP_Y + FOOTBALL_LIVE.field.height - 1;
const BALL_Y: i32 = FIELD_TOP_Y - art::football_ball::SPRITE.height - 2;
const BALL_HALF_WIDTH: i32 = art::football_ball::SPRITE.width / 2;

/// The table's field strip is the art's size: a PNG edit that resizes the
/// strip fails the build until the table follows.
const _: () = {
    let (at, sprite) = (FOOTBALL_LIVE.field, art::football_field::SPRITE);
    assert!(at.width == sprite.width && at.height == sprite.height);
};

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    let view = &snapshot.football_live;
    let table = FOOTBALL_LIVE;
    let colors = &snapshot.ui_colors;
    let primary = colors.primary;

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    for (row, timeouts, color) in [
        (table.timeouts_away, view.away_timeouts, view.away_color),
        (table.timeouts_home, view.home_timeouts, view.home_color),
    ] {
        // `None` means the feed did not advertise timeouts, so the bars stay
        // undrawn rather than showing a fake three.
        if let Some(remaining) = timeouts {
            timeout_blocks(canvas, row, TIMEOUTS, remaining, color, DIM_GRAY);
        }
    }

    super::big_score(canvas, table.score_away, view.away_score, primary);
    super::big_score(canvas, table.score_home, view.home_score, primary);

    if !view.phase_text.is_empty() {
        super::chip(canvas, table.phase, &view.phase_text, primary);
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
            super::nba::clock_color(colors, view.clock_accent, view.clock_low),
        ),
    );

    situation(canvas, scene, view);

    if super::bottom_strip(canvas, scene) == Strip::Free {
        field(canvas, view);
    }
    toast::overlay(canvas, snapshot, scene.now);
}

/// Down and distance, with the possession arrow beside it. Inside the red zone
/// both go to the warning color; outside it the arrow takes the possessing
/// side's color.
fn situation(canvas: &mut Canvas<'_>, scene: &Scene<'_>, view: &FootballLiveView) {
    if view.situation_text.is_empty() {
        return;
    }
    let table = FOOTBALL_LIVE;
    let colors = &scene.snapshot.ui_colors;
    let red_zone = view.situation.is_some_and(|situation| situation.red_zone);
    let text_color = if red_zone {
        colors.clock_warning
    } else {
        colors.primary
    };

    let mut region = canvas.slice(table.situation);
    font::draw_unscrolled(
        &mut region,
        &view.situation_text,
        Align::Center,
        Style::new(&generated::SPLEEN_5X8, text_color),
    );

    let Some(situation) = view.situation else {
        return;
    };
    // The arrow sits just outside the centered text, on the side the ball is
    // moving toward.
    let text_width = font::measure(&view.situation_text, &generated::SPLEEN_5X8);
    let text_x = table.situation.x + (table.situation.width - text_width) / 2;
    let points_right = situation.possession == Side::Home;
    let (x, color) = if points_right {
        (text_x + text_width + 3, view.home_color)
    } else {
        (text_x - 6, view.away_color)
    };
    let color = if red_zone { text_color } else { color };
    arrow(canvas, x, table.situation.y + 1, points_right, color);
}

/// The field strip: endzones tinted to the teams, then the two perspective
/// lines, then the ball riding the top end of the scrimmage line.
fn field(canvas: &mut Canvas<'_>, view: &FootballLiveView) {
    let sprite = art::football_field::SPRITE;
    let mut palette = art::football_field::PALETTE;
    palette[AWAY_ENDZONE] = Some(view.away_color);
    palette[HOME_ENDZONE] = Some(view.home_color);
    let at = FOOTBALL_LIVE.field;
    canvas.blit(&sprite.tinted(&palette), at.x, at.y);

    let Some(field) = view.situation.map(Field::project) else {
        return;
    };

    // Two pixels wide, drawn as two lines. First down goes second so it wins
    // where the two meet.
    for offset in 0..2 {
        canvas.line(
            field.scrimmage_x + offset,
            FIELD_BOTTOM_Y,
            field.scrimmage_top_x + offset,
            FIELD_TOP_Y,
            SCRIMMAGE,
        );
    }
    for offset in 0..2 {
        canvas.line(
            field.first_down_x + offset,
            FIELD_BOTTOM_Y,
            field.first_down_top_x + offset,
            FIELD_TOP_Y,
            FIRST_DOWN,
        );
    }

    let ball = art::football_ball::SPRITE;
    canvas.blit(
        &ball.source(),
        field.scrimmage_top_x - BALL_HALF_WIDTH,
        BALL_Y,
    );
    let arrow_x = if field.attacking_right {
        field.scrimmage_top_x + BALL_HALF_WIDTH + 3
    } else {
        field.scrimmage_top_x - BALL_HALF_WIDTH - 5
    };
    arrow(canvas, arrow_x, BALL_Y, field.attacking_right, SCRIMMAGE);
}

/// The drive, projected onto the field strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// Scrimmage line, at the field's bottom row and at its top row.
    pub scrimmage_x: i32,
    pub scrimmage_top_x: i32,
    pub first_down_x: i32,
    pub first_down_top_x: i32,
    /// Which way the possessing side is moving, and therefore which way the
    /// ball's arrow points.
    pub attacking_right: bool,
}

impl Field {
    /// Map a situation onto the strip.
    ///
    /// ESPN reports the yardline possession-relative, so the away side's
    /// yardline counts up from the left goal line and the home side's counts
    /// down from the right one. Both lines clamp at
    /// [`FOOTBALL_FIELD_LOS_MAX_X`](geometry::FOOTBALL_FIELD_LOS_MAX_X), which
    /// leaves room for the two-pixel width; the first-down line additionally
    /// clamps at the goal line, so goal-to-go puts it exactly there.
    pub fn project(situation: FieldSituation) -> Self {
        let yard_line = situation.yard_line as i32;
        let distance = situation.distance as i32;
        let (ball_yard, first_down_yard, attacking_right) = match situation.possession {
            Side::Away => (yard_line, (yard_line + distance).min(100), true),
            Side::Home => (
                100 - yard_line,
                (100 - (yard_line + distance)).max(0),
                false,
            ),
        };
        let to_x = |yard: i32| {
            (geometry::FOOTBALL_FIELD_YARD0_X + yard).min(geometry::FOOTBALL_FIELD_LOS_MAX_X)
        };
        let scrimmage_x = to_x(ball_yard);
        let first_down_x = to_x(first_down_yard);
        Field {
            scrimmage_x,
            scrimmage_top_x: geometry::football_top_x(scrimmage_x),
            first_down_x,
            first_down_top_x: geometry::football_top_x(first_down_x),
            attacking_right,
        }
    }
}

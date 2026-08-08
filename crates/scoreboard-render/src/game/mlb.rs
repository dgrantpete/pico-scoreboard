//! Live baseball: the field and count ledger.
//!
//! The identity column carries stacked logos and scores with the inning ordinal
//! below; the data column the diamond, its base markers, and the B/S/O count
//! block; the bottom strip the pitcher and batter, in their teams' colors, when
//! nothing outranks them.

use super::{Scene, Strip};
use crate::blit::{Canvas, Slice};
use crate::font::{self, Align, Style};
use crate::generated::layout;
use crate::geometry::{MLB_LIVE, PLAY_SCROLL_PAUSE_MS};
use crate::widgets::count_dots;
use crate::{BLACK, DIM_GRAY, WHITE, generated, pack, rgb565, screens, toast};
use scoreboard_model::Rgb888;
use scoreboard_model::snapshot::InningHalf;
use scoreboard_model::snapshot::MlbLiveView;

/// Brightness and saturation sweep for the critical-count pulse. The dots warm
/// from white toward a pale red at the peak, both channels driven off the same
/// triangle so they move in lockstep.
const PULSE_VALUE_BASE: u32 = 191;
const PULSE_VALUE_RANGE: u32 = 64;
const PULSE_SATURATION_MAX: u32 = 80;

pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    canvas.fill(BLACK);
    let snapshot = scene.snapshot;
    if snapshot.game_id.is_empty() {
        // No game latched yet: the idle card, still carrying toasts so button
        // feedback is not swallowed by the gap.
        screens::idle(canvas, snapshot);
        toast::strip(canvas, snapshot, scene.now);
        toast::overlay(canvas, snapshot, scene.now);
        return;
    }

    let view = &snapshot.mlb_live;
    let table = MLB_LIVE;

    super::column_dividers(
        canvas,
        scene.settings,
        Some(table.divider_x),
        Some(table.separator_y),
    );

    let field = layout::field::SPRITE;
    canvas.blit(
        &field.source(),
        layout::field::POSITION.x,
        layout::field::POSITION.y,
    );
    base_markers(canvas, view);

    super::both_logos(canvas, scene, table.logo_away, table.logo_home);

    // The inning arrow: up while the away side bats, down while the home side
    // does, nothing between halves.
    match view.half {
        InningHalf::Top => {
            let at = layout::inning_top::POSITION;
            canvas.blit(&layout::inning_top::SPRITE.source(), at.x, at.y);
        }
        InningHalf::Bottom => {
            let at = layout::inning_bottom::POSITION;
            canvas.blit(&layout::inning_bottom::SPRITE.source(), at.x, at.y);
        }
        InningHalf::Middle | InningHalf::End => {}
    }

    count_block(canvas, scene, view);

    super::big_score(canvas, table.score_away, view.away_score, WHITE);
    super::big_score(canvas, table.score_home, view.home_score, WHITE);
    super::chip(canvas, table.inning, &view.inning_text, WHITE);

    for (slot, label) in [
        (table.ball_label, "B"),
        (table.strike_label, "S"),
        (table.out_label, "O"),
    ] {
        let mut region = canvas.slice(slot);
        font::draw_unscrolled(
            &mut region,
            label,
            Align::Left,
            Style::new(&generated::UNSCII_8, DIM_GRAY),
        );
    }

    // Between halves there is no batting side, so both slots go gray.
    let pitch_color = view.pitch_color.map_or(DIM_GRAY, pack);
    let bat_color = view.bat_color.map_or(DIM_GRAY, pack);

    if super::bottom_strip(canvas, scene) == Strip::Free {
        if view.has_at_bat {
            let scroll = scene.game_scroll(PLAY_SCROLL_PAUSE_MS);
            for (slot, name, color) in [
                (table.pitcher_name, &view.pitcher, pitch_color),
                (table.batter_name, &view.batter, bat_color),
            ] {
                let mut region = canvas.slice(slot);
                font::draw(
                    &mut region,
                    name,
                    Align::Center,
                    scene.view.motion(),
                    Style::new(&generated::SPLEEN_5X8, color),
                    scroll,
                );
            }
        }
        for (slot, label, color) in [
            (table.pitcher_label, "PIT", pitch_color),
            (table.batter_label, "BAT", bat_color),
        ] {
            let mut region = canvas.slice(slot);
            font::draw_unscrolled(
                &mut region,
                label,
                Align::Left,
                Style::new(&generated::UNSCII_8, color),
            );
        }
    }

    toast::overlay(canvas, snapshot, scene.now);
}

/// Balls, strikes and outs, with the critical counts pulsing.
///
/// A 3-ball, 2-strike or 2-out count warms the whole row — outline included, so
/// the ring on an unfilled dot pulses too and the row reads as one color.
fn count_block(canvas: &mut Canvas<'_>, scene: &Scene<'_>, view: &MlbLiveView) {
    let table = MLB_LIVE;
    let critical = [view.balls == 3, view.strikes == 2, view.outs == 2];
    let pulsed = critical.iter().any(|hot| *hot).then(|| {
        let triangle = crate::pulse(scene.view.0, 1000);
        let value = PULSE_VALUE_BASE + ((triangle * PULSE_VALUE_RANGE) >> 8);
        let saturation = (triangle * PULSE_SATURATION_MAX) >> 8;
        warm_red(saturation, value)
    });

    let rows: [(Slice, u8); 3] = [
        (table.ball_dots, view.balls),
        (table.strike_dots, view.strikes),
        (table.out_dots, view.outs),
    ];
    for (index, (slot, filled)) in rows.into_iter().enumerate() {
        let tint = if critical[index] { pulsed } else { None };
        count_dots(canvas, slot, filled, tint);
    }
}

/// Hue 0 at the given saturation and value, packed for the panel.
///
/// The port of `hub75.native.pack_hsv_to_rgb565(0, s, v)`, which ships as an
/// opaque `.mpy`. At hue 0 the general conversion collapses: red keeps the full
/// value and the other two channels drop to `v * (255 - s) / 255`, rounded. The
/// rounding has no ties to break — the divisor is odd, so `2 * remainder` can
/// never equal it — which is why plain round-half-up is exact here rather than
/// merely close.
fn warm_red(saturation: u32, value: u32) -> u16 {
    let dim = (value * (255 - saturation) + 127) / 255;
    rgb565(value as u8, dim as u8, dim as u8)
}

/// Occupied-base markers, tinted to the batting side.
///
/// MicroPython wrote three entries into the sprite's shared palette and
/// restored them in a `finally` so a throwing blit could not leave later frames
/// tinted. The palette is copied into a local here, so there is nothing to
/// restore and nothing to leak.
fn base_markers(canvas: &mut Canvas<'_>, view: &MlbLiveView) {
    let sprite = layout::base_marker::SPRITE;
    let mut palette = layout::base_marker::PALETTE;
    if let Some(color) = view.bat_color {
        let (body, highlight, shade) = marker_shades(color);
        palette[1] = body;
        palette[2] = highlight;
        palette[3] = shade;
    }
    let source = sprite.tinted(&palette);

    for (occupied, at) in [
        (view.bases.first, layout::first_base::SLICE),
        (view.bases.second, layout::second_base::SLICE),
        (view.bases.third, layout::third_base::SLICE),
    ] {
        if occupied {
            canvas.blit(&source, at.x, at.y);
        }
    }
}

/// Ball body, highlight and edge shade from the batting side's color, in the
/// relationships the original gold sprite had: the highlight is 7/8 of the way
/// to white, the shade 7/8 of the way to black.
///
/// `display._base_marker_colors` re-applied the team-color brightening here,
/// because `state.py` kept a second unbrightened copy for exactly this. The
/// model brightens once and stores the result, so this starts from the color
/// the panel will actually show.
fn marker_shades(color: Rgb888) -> (u16, u16, u16) {
    let (red, green, blue) = (
        color.red() as u16,
        color.green() as u16,
        color.blue() as u16,
    );
    let toward_white = |channel: u16| (channel + (((255 - channel) * 7) >> 3)) as u8;
    let toward_black = |channel: u16| ((channel * 7) >> 3) as u8;
    (
        rgb565(red as u8, green as u8, blue as u8),
        rgb565(toward_white(red), toward_white(green), toward_white(blue)),
        rgb565(toward_black(red), toward_black(green), toward_black(blue)),
    )
}

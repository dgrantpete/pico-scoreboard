//! Screen geometry: the panel pin, variant selection, and the scroll-speed set.

use scoreboard_model::Sport;
use scoreboard_render::geometry::{
    self, FINAL_A, FINAL_B, FINAL_C, FinalVariant, RenderSettings, SCROLL_SPEEDS,
    SoccerLiveVariant, is_smooth,
};
use scoreboard_render::menu::thumb;

/// The tables are laid out for the panel the driver drives. This is the whole
/// reason `hub75` is a dev-dependency.
const _: () = {
    assert!(geometry::WIDTH == hub75::geometry::WIDTH as i32);
    assert!(geometry::HEIGHT == hub75::geometry::HEIGHT as i32);
};

#[test]
fn variant_selection_ignores_unknown_keys_and_letters() {
    let mut settings = RenderSettings::new();
    assert!(settings.apply_variant("mlb_final", "A"));
    assert_eq!(settings.mlb_final, FinalVariant::A);

    // A letter the table does not have leaves the selection alone — a
    // hand-edited config cannot select a design that does not exist.
    assert!(!settings.apply_variant("mlb_final", "Z"));
    assert_eq!(settings.mlb_final, FinalVariant::A);

    // Nor can a key that does not exist, including a pre-rename one.
    assert!(!settings.apply_variant("final", "B"));
    assert!(!settings.apply_variant("baseball_final", "B"));
    assert_eq!(settings.mlb_final, FinalVariant::A);
}

#[test]
fn each_sports_final_is_selected_independently() {
    let mut settings = RenderSettings::new();
    settings.apply_variant("mlb_final", "A");
    settings.apply_variant("nba_final", "B");
    assert_eq!(settings.final_table(Sport::Mlb), FINAL_A);
    assert_eq!(settings.final_table(Sport::Nba), FINAL_B);
    assert_eq!(
        settings.final_table(Sport::Football),
        FINAL_C,
        "untouched keys keep the default"
    );
}

#[test]
fn single_design_screens_accept_only_their_one_letter() {
    let mut settings = RenderSettings::new();
    assert!(settings.apply_variant("mlb_pregame", "C"));
    assert!(!settings.apply_variant("mlb_pregame", "A"));
    assert!(settings.apply_variant("nba_live", "A"));
    assert!(!settings.apply_variant("nba_live", "C"));
}

#[test]
fn soccer_live_defaults_to_a_and_can_move() {
    let mut settings = RenderSettings::new();
    assert_eq!(settings.soccer_live, SoccerLiveVariant::A);
    assert!(settings.apply_variant("soccer_live", "C"));
    let table = settings.soccer_live_table();
    assert!(
        table.divider_x.is_none(),
        "C deliberately breaks the column frame"
    );
    assert!(table.phase.is_some());
    assert!(table.phase_long.is_none());
}

#[test]
fn the_b_variant_spells_the_period_out_instead_of_chipping_it() {
    let mut settings = RenderSettings::new();
    settings.apply_variant("soccer_live", "B");
    let table = settings.soccer_live_table();
    assert!(table.phase.is_none());
    assert!(table.phase_long.is_some());
}

#[test]
fn illegal_scroll_speeds_degrade_to_twenty() {
    let mut settings = RenderSettings::new();
    for speed in SCROLL_SPEEDS {
        assert_eq!(settings.set_scroll_speed(speed), speed);
    }
    // 30 px/s is the documented failure case: 1.5 px per frame, realised as
    // alternating 1 and 2 px steps.
    for speed in [0, -5, 1, 3, 30, 25, 60, 1000] {
        assert_eq!(settings.set_scroll_speed(speed), 20, "{speed} px/s");
    }
}

#[test]
fn the_smoothness_test_rejects_non_divisors() {
    for speed in SCROLL_SPEEDS {
        assert!(is_smooth(speed), "{speed} px/s is in the accepted set");
    }
    for speed in [1, 2, 4, 5, 10, 20, 40, 60] {
        assert!(is_smooth(speed));
    }
    for speed in [0, 3, 6, 7, 12, 15, 30, 33] {
        assert!(!is_smooth(speed), "{speed} px/s should be rejected");
    }
}

#[test]
fn only_the_a_final_has_a_full_width_rule() {
    assert!(FINAL_A.separator_y.is_some());
    assert!(FINAL_B.separator_y.is_none());
    assert!(FINAL_C.separator_y.is_none());
}

#[test]
fn the_line_score_forward_final_puts_the_totals_in_the_r_column() {
    // C has no separate score slots: the pinned R column carries them, in the
    // taller font.
    assert!(FINAL_C.score_away.is_none());
    assert_eq!(FINAL_C.total_away.height, 16);
    assert_eq!(FINAL_A.total_away.height, 8);
    assert!(FINAL_A.score_away.is_some());
}

#[test]
fn every_slot_fits_on_the_panel() {
    // A rectangle that escapes the panel is a table bug that would panic the
    // first time its screen drew; catch it here instead.
    let mut slots = vec![];
    let settings = RenderSettings::new();
    let pregame = geometry::PREGAME;
    slots.extend([
        pregame.logo_away,
        pregame.logo_home,
        pregame.record_away_wins,
        pregame.record_home_losses,
        pregame.info_time,
        pregame.info_cycle,
        pregame.team_line_away,
        pregame.team_line_home,
    ]);
    for variant in [FinalVariant::A, FinalVariant::B, FinalVariant::C] {
        let table = variant.table();
        slots.extend([
            table.logo_away,
            table.logo_home,
            table.final_label,
            table.linescore_header,
            table.linescore_away,
            table.linescore_home,
            table.total_header,
            table.total_away,
            table.total_home,
        ]);
        slots.extend(table.score_away);
        slots.extend(table.score_home);
    }
    for variant in [
        SoccerLiveVariant::A,
        SoccerLiveVariant::B,
        SoccerLiveVariant::C,
    ] {
        let table = variant.table();
        slots.extend([
            table.logo_away,
            table.logo_home,
            table.score_away,
            table.score_home,
            table.clock,
            table.event_top,
            table.event_name,
            table.event_empty,
        ]);
        slots.extend(table.phase);
        slots.extend(table.phase_long);
    }
    let mlb = geometry::MLB_LIVE;
    slots.extend([
        mlb.logo_away,
        mlb.score_away,
        mlb.inning,
        mlb.ball_dots,
        mlb.strike_dots,
        mlb.out_dots,
        mlb.pitcher_name,
        mlb.batter_name,
    ]);
    let nba = geometry::NBA_LIVE;
    slots.extend([nba.score_home, nba.phase, nba.clock]);
    let football = geometry::FOOTBALL_LIVE;
    slots.extend([
        football.logo_home,
        football.score_home,
        football.phase,
        football.clock,
        football.situation,
    ]);
    let soccer_final = geometry::SOCCER_FINAL;
    slots.extend([
        soccer_final.scorers_away,
        soccer_final.scorers_home,
        soccer_final.full_time_label,
    ]);
    slots.push(geometry::PLAY_TEXT);

    for slot in slots {
        assert!(slot.x >= 0 && slot.y >= 0, "{slot:?} starts off-panel");
        assert!(
            slot.x + slot.width <= geometry::WIDTH,
            "{slot:?} runs past the right edge"
        );
        assert!(
            slot.y + slot.height <= geometry::HEIGHT,
            "{slot:?} runs past the bottom edge"
        );
    }
    assert!(settings.show_dividers);
}

#[test]
fn the_football_perspective_leans_toward_the_vanishing_point() {
    use geometry::{FOOTBALL_VP_X, football_top_x};
    // A line at the vanishing point's column stays vertical; lines either side
    // lean inward, never past it.
    assert_eq!(football_top_x(FOOTBALL_VP_X), FOOTBALL_VP_X);
    for x in [
        geometry::FOOTBALL_FIELD_YARD0_X,
        30,
        90,
        geometry::FOOTBALL_FIELD_LOS_MAX_X,
    ] {
        let top = football_top_x(x);
        assert!(
            (x.min(FOOTBALL_VP_X)..=x.max(FOOTBALL_VP_X)).contains(&top),
            "line at {x} leaned to {top}"
        );
    }
}

#[test]
fn the_menu_thumb_matches_menu_pys_arithmetic() {
    // Ported case by case from menu.py's `_publish`.
    assert_eq!(thumb::compute(3, 0), (-1, 0), "a list that fits has no bar");
    assert_eq!(thumb::compute(5, 0), (-1, 0));

    // 6 items: thumb 50 * 5 / 6 = 41, travel (50 - 41) * scroll / 1.
    assert_eq!(thumb::compute(6, 0), (1, 41));
    assert_eq!(thumb::compute(6, 1), (10, 41));

    // 20 items: thumb 12, travel (50 - 12) * scroll / 15.
    assert_eq!(thumb::compute(20, 0), (1, 12));
    assert_eq!(thumb::compute(20, 15), (39, 12));

    // 100 items: the thumb hits its floor and only slides.
    let (_, height) = thumb::compute(100, 0);
    assert_eq!(height as i32, thumb::MIN_THUMB_H);
}

#[test]
fn the_thumb_stays_inside_its_track() {
    for items in 6..40usize {
        for scroll in 0..=(items - thumb::VISIBLE_ROWS) {
            let (y, height) = thumb::compute(items, scroll);
            assert!(y >= thumb::TRACK_Y0 as i8, "{items}/{scroll}");
            assert!(
                y as i32 + height as i32 <= thumb::TRACK_Y0 + thumb::TRACK_H,
                "{items}/{scroll} overruns the track"
            );
        }
    }
}

//! Screen geometry: the panel pin, the slot tables, and the scroll-speed set.

use scoreboard_render::geometry::{
    self, DEFAULT_SCROLL_SPEED, FINAL, HOCKEY_FINAL, HOCKEY_LIVE, RenderSettings, SCROLL_SPEEDS,
    SHOOTOUT_ROW_WIDTH, SOCCER_LIVE, is_smooth,
};
use scoreboard_render::menu::thumb;

/// The tables are laid out for the panel the driver drives. This is the whole
/// reason `hub75` is a dev-dependency.
const _: () = {
    assert!(geometry::WIDTH == hub75::geometry::WIDTH as i32);
    assert!(geometry::HEIGHT == hub75::geometry::HEIGHT as i32);
};

#[test]
fn illegal_scroll_speeds_degrade_to_the_default() {
    let mut settings = RenderSettings::new();
    for speed in SCROLL_SPEEDS {
        assert_eq!(settings.set_scroll_speed(speed), speed);
    }
    // 40 is the one that matters: it is what the parity release accepted and
    // what a device upgraded from it has stored, and it is not smooth at 60 FPS.
    // It degrades like any other illegal value rather than being special-cased,
    // and `DEFAULT_SCROLL_SPEED` is chosen so that landing there is sensible.
    for speed in [0, -5, 7, 8, 25, 40, 45, 1000] {
        assert_eq!(
            settings.set_scroll_speed(speed),
            DEFAULT_SCROLL_SPEED,
            "{speed} px/s"
        );
    }
}

#[test]
fn the_degrade_target_is_the_legal_speed_nearest_the_one_it_replaces() {
    // Not arbitrary: a device carrying the parity release's 40 px/s has to land
    // somewhere, and the nearest legal speed is the least surprising answer.
    let nearest = SCROLL_SPEEDS
        .into_iter()
        .min_by_key(|speed| (speed - 40).abs())
        .expect("the accepted set is not empty");
    assert_eq!(DEFAULT_SCROLL_SPEED, nearest);
}

#[test]
fn the_smoothness_test_rejects_non_divisors() {
    for speed in SCROLL_SPEEDS {
        assert!(is_smooth(speed), "{speed} px/s is in the accepted set");
    }
    // The divisors of 60, plus its multiples.
    for speed in [1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30, 60, 120] {
        assert!(is_smooth(speed), "{speed} px/s divides 60 or is a multiple");
    }
    // 40 and 45 are the instructive rejections: both were plausible, neither
    // divides 60 in either direction.
    for speed in [0, 7, 8, 9, 11, 25, 40, 45, 70] {
        assert!(!is_smooth(speed), "{speed} px/s should be rejected");
    }
}

#[test]
fn the_final_puts_the_headline_totals_in_the_r_column() {
    // No score slots beside the crests: the pinned R column carries the
    // headline totals, in the taller font.
    assert_eq!(FINAL.total_away.height, 16);
    assert_eq!(FINAL.total_home.height, 16);
}

#[test]
fn every_slot_fits_on_the_panel() {
    // A rectangle that escapes the panel is a table bug that would panic the
    // first time its screen drew; catch it here instead.
    let mut slots = vec![];
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
    let fin = FINAL;
    slots.extend([
        fin.logo_away,
        fin.logo_home,
        fin.final_label,
        fin.linescore_header,
        fin.linescore_away,
        fin.linescore_home,
        fin.total_header,
        fin.total_away,
        fin.total_home,
    ]);
    let soccer = SOCCER_LIVE;
    slots.extend([
        soccer.logo_away,
        soccer.logo_home,
        soccer.score_away,
        soccer.score_home,
        soccer.phase,
        soccer.clock,
        soccer.event_top,
        soccer.event_name,
        soccer.event_empty,
    ]);
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
    slots.extend([
        nba.logo_away,
        nba.logo_home,
        nba.fouls_away,
        nba.fouls_home,
        nba.score_away,
        nba.score_home,
        nba.timeouts_away,
        nba.timeouts_home,
        nba.period,
        nba.clock,
        nba.caption,
        nba.caption_box,
    ]);
    let nba_final = geometry::NBA_FINAL;
    slots.extend([
        nba_final.logo_away,
        nba_final.logo_home,
        nba_final.score_away,
        nba_final.score_home,
        nba_final.final_label,
        nba_final.series,
        nba_final.records,
        nba_final.records_alone,
    ]);
    slots.extend(nba_final.leader_rows);
    slots.extend(nba_final.linescore_rows);
    let football = geometry::FOOTBALL_LIVE;
    slots.extend([
        football.logo_home,
        football.score_home,
        football.phase,
        football.clock,
        football.situation,
    ]);
    let (hockey, hockey_final) = (HOCKEY_LIVE, HOCKEY_FINAL);
    slots.extend([
        hockey.logo_away,
        hockey.logo_home,
        hockey.score_away,
        hockey.score_home,
        hockey.period,
        hockey.clock,
        hockey.situation,
        hockey.shots,
        hockey.shootout_away,
        hockey.shootout_home,
        hockey.strip,
        hockey_final.logo_away,
        hockey_final.logo_home,
        hockey_final.score_away,
        hockey_final.score_home,
        hockey_final.final_label,
        hockey_final.series,
        hockey_final.shots,
        hockey_final.single_line,
    ]);
    slots.extend(hockey_final.star_rows);
    slots.extend(hockey_final.linescore_rows);
    let pregame_rows = [
        geometry::PREGAME.hockey_record_away,
        geometry::PREGAME.hockey_record_home,
    ];
    slots.extend(pregame_rows.into_iter().flatten());
    let soccer_final = geometry::SOCCER_FINAL;
    slots.extend([
        soccer_final.scorers_away,
        soccer_final.scorers_home,
        soccer_final.full_time_label,
    ]);
    slots.extend([
        geometry::COLUMN_STRIP,
        mlb.strip,
        nba.strip,
        football.strip,
        soccer.strip,
    ]);

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
}

/// Whether `slot` keeps off the unreliable outer ring: row 0, row 63, column 0
/// and column 127 (BACKLOG 56).
fn inside_the_edge_ring(slot: scoreboard_render::Slice) -> bool {
    // Exclusive right/bottom ends, so "ends before the last column/row".
    slot.x > 0
        && slot.y > 0
        && slot.x + slot.width < geometry::WIDTH
        && slot.y + slot.height < geometry::HEIGHT
}

/// Football is the edge-rule-compliant corner screen; this keeps it so. The
/// column screens are not yet (BACKLOG 56), so they are not listed, and the
/// hockey screens have their own test below.
#[test]
fn the_football_screen_keeps_off_the_edge_ring() {
    let football = geometry::FOOTBALL_LIVE;
    for slot in [
        football.logo_away,
        football.logo_home,
        football.score_away,
        football.score_home,
        football.phase,
        football.clock,
        football.situation,
        football.strip,
        scoreboard_render::generated::layout::football_field::POSITION,
    ] {
        assert!(inside_the_edge_ring(slot), "{slot:?} touches the edge ring");
    }
}

/// Every hockey screen was born under the edge rule (NHL.md §6.5).
#[test]
fn the_hockey_screens_keep_off_the_edge_ring() {
    let (live, fin) = (HOCKEY_LIVE, HOCKEY_FINAL);
    let mut slots = vec![
        live.logo_away,
        live.logo_home,
        live.score_away,
        live.score_home,
        live.period,
        live.clock,
        live.situation,
        live.shots,
        live.shootout_away,
        live.shootout_home,
        live.strip,
        scoreboard_render::generated::layout::rink::POSITION,
        fin.logo_away,
        fin.logo_home,
        fin.score_away,
        fin.score_home,
        fin.final_label,
        fin.series,
        fin.shots,
        fin.single_line,
    ];
    slots.extend(fin.star_rows);
    slots.extend(fin.linescore_rows);
    for slot in slots {
        assert!(inside_the_edge_ring(slot), "{slot:?} touches the edge ring");
    }
    // The skater rows end at the last legal column, and the shootout's short
    // rink stops above the bottom row.
    assert_eq!(live.skaters_home_end, geometry::WIDTH - 2);
    assert!(live.shootout_rink_y + 9 < geometry::HEIGHT - 1);
    // Seven attempts fit between the crests.
    assert!(live.shootout_away.x + SHOOTOUT_ROW_WIDTH < live.logo_home.x);
}

/// Both NBA screens were born under the edge rule too (NBA.md §6.5): the
/// court's lines are its outer ring, so the court itself must keep off the
/// panel's.
#[test]
fn the_nba_screens_keep_off_the_edge_ring() {
    let (live, fin) = (geometry::NBA_LIVE, geometry::NBA_FINAL);
    let court = scoreboard_render::Slice {
        x: live.court.x,
        y: live.court.y,
        width: live.court.width,
        height: live.court.height,
    };
    let mut slots = vec![
        live.logo_away,
        live.logo_home,
        live.fouls_away,
        live.fouls_home,
        live.score_away,
        live.score_home,
        live.timeouts_away,
        live.timeouts_home,
        live.period,
        live.clock,
        live.caption,
        live.caption_box,
        live.strip,
        court,
        fin.logo_away,
        fin.logo_home,
        fin.score_away,
        fin.score_home,
        fin.final_label,
        fin.series,
        fin.records,
        fin.records_alone,
    ];
    slots.extend(fin.leader_rows);
    slots.extend(fin.linescore_rows);
    for slot in slots {
        assert!(inside_the_edge_ring(slot), "{slot:?} touches the edge ring");
    }
    // The caption and its box sit on the floor, inside the court's lines.
    assert!(live.caption_box.y > live.court.y);
    assert!(live.caption_box.y + live.caption_box.height < live.court.y + live.court.height);
    // The flash's 16 px line is centred in the court's rows: unscii_16 inks
    // rows 1–12 of its cell, so the cell starts six rows above the middle.
    assert_eq!(live.strip.y, live.court.y + (live.court.height - 1) / 2 - 6);
    // The flash slot is the one the prepared view sizes the window by.
    assert_eq!(
        geometry::bottom_strip(scoreboard_model::Mode::NbaLive),
        live.strip
    );
}

/// Both claimants of the bottom strip draw into the slot the selector names,
/// and every live screen's strip sits in the bottom band of the panel.
#[test]
fn every_live_screen_has_its_own_bottom_strip() {
    use geometry::bottom_strip;
    use scoreboard_model::Mode;
    assert_eq!(bottom_strip(Mode::MlbLive), geometry::MLB_LIVE.strip);
    assert_eq!(bottom_strip(Mode::NbaLive), geometry::NBA_LIVE.strip);
    assert_eq!(
        bottom_strip(Mode::FootballLive),
        geometry::FOOTBALL_LIVE.strip
    );
    // Soccer keeps its column: the strip holds one `unscii_16` line, below the
    // rule.
    let soccer = SOCCER_LIVE;
    assert_eq!(bottom_strip(Mode::SoccerLive), soccer.strip);
    assert_eq!(soccer.strip.height, 16);
    assert!(soccer.strip.y > soccer.separator_y);
    // Hockey's rink gives up the full width under the shots bar. `unscii_16`
    // inks from its cell's second row, so four dark rows separate the bar's
    // last row from the text.
    let strip = bottom_strip(Mode::HockeyLive);
    assert_eq!(strip, HOCKEY_LIVE.strip);
    assert_eq!((strip.x, strip.width), (1, geometry::WIDTH - 2));
    let (bar_bottom, ink_top) = (HOCKEY_LIVE.shots_bar_y + 1, strip.y + 1);
    assert_eq!(ink_top - bar_bottom - 1, 4);
    assert_eq!(bottom_strip(Mode::Final), geometry::COLUMN_STRIP);
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

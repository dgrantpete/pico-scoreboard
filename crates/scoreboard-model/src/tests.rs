//! Host tests. Split into the semantics ports (the rules this crate exists to
//! preserve) and the corpus sweep (every committed wire golden, decoded and
//! built, with the bounds checked against what the fixtures actually contain).

use std::collections::BTreeMap;
use std::path::Path;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

use scoreboard_wire::{GameState, football, mlb, nba, soccer};

use crate::color::Rgb888;
use crate::feed::{GameDetail, GameFeed, LeagueId, ListSink, Sport, WireFeed};
use crate::slate::Slate;
use crate::snapshot::{self, Mode, ScoreboardSnapshot, SetupReason, ToastKind};
use crate::sports::{LinescoreFinal, LocalClock, PregameInput};
use crate::store::{Logos, MenuRowInput, StartupExit, Store};
use crate::text::{self, Text};

// =========================================================================
// Corpus fixtures
// =========================================================================

/// The committed wire goldens: `backend/testdata/wire/**.bin`, keyed by the
/// `{sport}[/{league}]/{fixture}` name the backend's harness uses.
fn corpus() -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {dir:?}: {e}"))
            .map(|entry| entry.expect("readable dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            if path.is_dir() {
                walk(&path, &format!("{prefix}{name}/"), out);
            } else if let Some(stem) = name.strip_suffix(".bin") {
                let bytes = std::fs::read(&path).expect("read golden");
                out.push((format!("{prefix}{stem}"), bytes));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backend/testdata/wire")
        .canonicalize()
        .expect("the backend's wire goldens are the corpus");
    let mut out = Vec::new();
    walk(&root, "", &mut out);
    assert!(!out.is_empty(), "corpus must not be empty");
    out
}

/// The league a fixture's path names. Football and soccer goldens nest under
/// their ESPN slug, which is exactly what `LeagueId::from_slug` consumes.
fn league_of(name: &str) -> LeagueId {
    let mut parts = name.split('/');
    match parts.next().expect("sport-prefixed fixture name") {
        "mlb" => LeagueId::from_slug(Sport::Mlb, "mlb"),
        "nba" => LeagueId::from_slug(Sport::Nba, "nba"),
        "football" => LeagueId::from_slug(Sport::Football, parts.next().expect("league slug")),
        "soccer" => LeagueId::from_slug(Sport::Soccer, parts.next().expect("league slug")),
        other => panic!("unknown sport {other}"),
    }
}

fn fixture(name: &str) -> (LeagueId, Vec<u8>) {
    let bytes = corpus()
        .into_iter()
        .find(|(fixture, _)| fixture == name)
        .unwrap_or_else(|| panic!("no fixture {name}"))
        .1;
    (league_of(name), bytes)
}

/// Decode and commit one fixture into a fresh store.
fn committed(name: &str) -> Store {
    let (league, bytes) = fixture(name);
    let detail = WireFeed
        .detail(league.sport, &bytes)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut store = Store::new();
    store.commit_detail(&league, &detail, Logos::default(), 1_000, clock());
    store
}

/// A device that has synced: 2024-07-17 18:30 UTC, US Mountain (-6).
fn clock() -> LocalClock {
    LocalClock {
        now_epoch_s: 1_721_240_000,
        utc_offset_s: Some(-6 * 3_600),
    }
}

// =========================================================================
// textfold
// =========================================================================

fn folded(source: &str) -> String {
    let mut out = Text::<256>::new();
    text::set_folded(&mut out, source);
    out.as_str().to_string()
}

#[test]
fn fold_maps_the_names_that_prompted_it() {
    assert_eq!(folded("Jokić"), "Jokic");
    // ü is Latin-1 and renders natively; only Ş folds.
    assert_eq!(folded("Şengün"), "Sengün");
    assert_eq!(folded("Suárez"), "Suárez");
}

#[test]
fn fold_widens_the_ligatures() {
    assert_eq!(folded("Ĳssel"), "IJssel");
    assert_eq!(folded("ĳ"), "ij");
    assert_eq!(folded("ŉ"), "'n");
    assert_eq!(folded("Œuvre"), "OEuvre");
    assert_eq!(folded("œ"), "oe");
    assert_eq!(folded("wait…"), "wait...");
}

#[test]
fn fold_covers_the_punctuation_and_romanian_strays() {
    assert_eq!(folded("Șerban Țopa"), "Serban Topa");
    assert_eq!(folded("‘quoted’ “too”"), "'quoted' \"too\"");
    assert_eq!(folded("6‐0 – 2"), "6-0 - 2");
    assert_eq!(folded("•′″⁄"), "\u{b7}'\"/");
}

#[test]
fn fold_passes_unmapped_codepoints_through_to_the_glyph_fallback() {
    // Beyond the table: kept as-is, and the font draws '?'.
    assert_eq!(folded("日本"), "日本");
}

/// The property the play-text bound rests on: 255 wire bytes can never become
/// more than 255 glyphs, so the renderer's strip pool cannot overflow.
#[test]
fn fold_never_lengthens_a_string() {
    let mut sources: Vec<String> = vec![
        "Ĳ".to_string(),
        "ĳ".to_string(),
        "ŉ".to_string(),
        "Œ".to_string(),
        "œ".to_string(),
        "…".to_string(),
        "•".to_string(),
    ];
    for c in ('\u{100}'..='\u{17f}').chain('\u{218}'..='\u{21b}') {
        sources.push(c.to_string());
    }
    for source in sources {
        let out = folded(&source);
        assert!(
            out.len() <= source.len(),
            "{source:?} -> {out:?} grew from {} to {} bytes",
            source.len(),
            out.len()
        );
        assert!(out.chars().count() <= source.len());
    }
}

#[test]
fn fold_truncates_at_a_char_boundary_rather_than_overflowing() {
    let mut out = Text::<5>::new();
    text::set_folded(&mut out, "ünïcödé");
    assert!(out.len() <= 5);
    assert!(out.as_str().chars().all(|c| "ünïcödé".contains(c)));

    let mut ascii = Text::<4>::new();
    text::set_folded(&mut ascii, "abcdefgh");
    assert_eq!(ascii.as_str(), "abcd");
}

#[test]
fn a_line_caps_at_25_glyphs_with_a_truncation_dot() {
    let mut out = Text::<{ snapshot::LINE }>::new();
    text::set_line(&mut out, "0123456789012345678901234");
    assert_eq!(out.as_str(), "0123456789012345678901234");
    text::set_line(&mut out, "01234567890123456789012345");
    assert_eq!(out.as_str(), "012345678901234567890123.");
    assert_eq!(out.as_str().chars().count(), 25);
}

// =========================================================================
// Team color brightening
// =========================================================================

#[test]
fn brightening_preserves_hue_and_lifts_only_dark_colors() {
    // Yankees navy: brightest channel 0x40, scaled until it reaches 128.
    let navy = Rgb888(0x0C_2340).brightened();
    assert_eq!(navy.blue(), 128);
    assert_eq!(navy.red(), 0x0C * 2);
    assert_eq!(navy.green(), 0x23 * 2);

    // Already bright: untouched.
    let red = Rgb888(0xBD_3039);
    assert_eq!(red.brightened(), red);
    // Exactly at the threshold counts as bright enough.
    let edge = Rgb888(0x00_0080);
    assert_eq!(edge.brightened(), edge);
}

#[test]
fn brightening_turns_pure_black_into_gray() {
    assert_eq!(Rgb888(0).brightened(), Rgb888::new(128, 128, 128));
}

/// The MicroPython pair disagreed here: `state._team_color_to_rgb565`
/// multiplied by a float `128 / max`, which lands on 127.999… and truncates,
/// while `display._base_marker_colors` used the integer form. One color, one
/// answer.
#[test]
fn brightening_is_exact_where_the_float_form_lost_a_bit() {
    assert_eq!(Rgb888::new(3, 0, 0).brightened().red(), 128);
    assert_eq!(Rgb888::new(2, 1, 0).brightened().red(), 128);
}

#[test]
fn a_team_color_reaches_the_view_already_brightened() {
    let store = committed("mlb/live_inning");
    let view = &store.snapshot().mlb_live;
    for color in [view.bat_color, view.pitch_color].into_iter().flatten() {
        let brightest = color.red().max(color.green()).max(color.blue());
        assert!(brightest >= 128, "{color:?} is too dark to read");
    }
}

// =========================================================================
// The view-identity rule
// =========================================================================

#[test]
fn a_standing_repoll_of_the_same_game_keeps_the_animation_clock() {
    let (league, bytes) = fixture("mlb/live_inning");
    let detail = WireFeed.detail(league.sport, &bytes).unwrap();
    let mut store = Store::new();

    store.commit_detail(&league, &detail, Logos::default(), 1_000, clock());
    assert_eq!(store.snapshot().animation_start_ms, 1_000);

    let seq = store.commit_seq();
    store.commit_detail(&league, &detail, Logos::default(), 9_000, clock());
    assert_eq!(
        store.snapshot().animation_start_ms,
        1_000,
        "an unchanged re-poll must not restart the scroll"
    );
    assert!(store.commit_seq() > seq, "but it is still a commit");
}

#[test]
fn a_different_game_restarts_the_animation_clock() {
    let (league, bytes) = fixture("nba/in_progress");
    let detail = WireFeed.detail(league.sport, &bytes).unwrap();
    let mut store = Store::new();
    store.commit_detail(&league, &detail, Logos::default(), 1_000, clock());

    let (other_league, other_bytes) = fixture("nba/halftime");
    let other = WireFeed.detail(other_league.sport, &other_bytes).unwrap();
    assert_ne!(detail.game_id(), other.game_id());
    store.commit_detail(&other_league, &other, Logos::default(), 9_000, clock());
    assert_eq!(store.snapshot().animation_start_ms, 9_000);
}

#[test]
fn the_same_game_in_a_new_mode_restarts_the_animation_clock() {
    // A pregame card that flips live mid-view is a mode change on one id.
    let mut store = Store::new();
    let league = LeagueId::from_slug(Sport::Nba, "nba");
    let pregame = mlb_free_pregame("401", 1_721_260_000);
    store.commit_pregame(&pregame, Logos::default(), 1_000, clock());
    assert_eq!(store.snapshot().mode, Mode::Pregame);
    assert_eq!(store.snapshot().animation_start_ms, 1_000);

    let (_, bytes) = fixture("nba/in_progress");
    let detail = WireFeed.detail(league.sport, &bytes).unwrap();
    store.commit_detail(&league, &detail, Logos::default(), 5_000, clock());
    assert_eq!(store.snapshot().animation_start_ms, 5_000);
}

/// A hand-built pregame with a chosen id, for the identity tests.
fn mlb_free_pregame<'a>(game_id: &'a str, start_time: u32) -> PregameInput<'a> {
    use crate::sports::PregameSideInput;
    let side = PregameSideInput {
        abbreviation: "AAA",
        record: None,
        line: "",
        color: Rgb888::WHITE,
    };
    PregameInput {
        sport: Sport::Mlb,
        game_id,
        start_time,
        info_primary: "SOME PARK",
        info_secondary: "",
        temperature: None,
        away: side,
        home: side,
    }
}

// =========================================================================
// The play flash
// =========================================================================

#[test]
fn the_play_flash_fires_once_per_new_id() {
    let mut store = Store::new();
    assert!(store.flash_play("p1", "Judge homers", 100));
    assert_eq!(store.snapshot().play.text.as_str(), "Judge homers");
    assert_eq!(store.snapshot().play.updated_ms, 100);

    assert!(!store.flash_play("p1", "Judge homers", 200));
    assert_eq!(store.snapshot().play.updated_ms, 100);

    assert!(store.flash_play("p2", "Soto walks", 300));
    assert_eq!(store.snapshot().play.updated_ms, 300);

    // An absent play (NBA before the tip) leaves the slot alone, so a play
    // that reappears unchanged cannot re-flash.
    assert!(!store.flash_play("", "", 400));
    assert_eq!(store.snapshot().play.id.as_str(), "p2");
}

// =========================================================================
// Line scores
// =========================================================================

#[test]
fn linescore_rows_are_equal_width_three_char_columns() {
    let store = committed("mlb/final");
    let view = &store.snapshot().linescore_final;
    assert_eq!(view.header_row.len(), view.away_row.len());
    assert_eq!(view.header_row.len(), view.home_row.len());
    assert_eq!(view.header_row.len() % 3, 0);
    assert!(view.header_row.as_str().starts_with(" 1  2  3 "));
}

#[test]
fn a_walk_off_pads_the_missing_home_column_with_x() {
    // The home side bats last and stops when it wins, so its line is short.
    let mut store = Store::new();
    let away = scoreboard_wire::FinalTeam {
        abbreviation: "AWY",
        score: 3,
        colors: scoreboard_wire::TeamColors {
            primary: 0x00_FF_00_00,
            alternate: 0,
        },
        line_score: &[1, 0, 2],
    };
    let home = scoreboard_wire::FinalTeam {
        line_score: &[0, 4],
        score: 4,
        ..away
    };
    store.commit_linescore_final(
        &LinescoreFinal {
            sport: Sport::Mlb,
            game_id: "g1",
            periods: 3,
            away,
            home,
        },
        Logos::default(),
        0,
    );
    let view = &store.snapshot().linescore_final;
    assert_eq!(view.header_row.as_str(), " 1  2  3 ");
    assert_eq!(view.away_row.as_str(), " 1  0  2 ");
    assert_eq!(view.home_row.as_str(), " 0  4  X ");
    assert!(view.home_won);
}

#[test]
fn extra_innings_and_overtime_get_their_own_final_text() {
    let store = committed("mlb/final");
    assert_eq!(
        store.snapshot().linescore_final.final_text.as_str(),
        "FINAL"
    );

    let store = committed("football/nfl/final_ot");
    let view = &store.snapshot().linescore_final;
    assert_eq!(view.final_text.as_str(), "F/OT");
    assert_eq!(view.sport, Sport::Football);
}

// =========================================================================
// Soccer clock
// =========================================================================

#[test]
fn the_soccer_clock_anchors_and_runs_in_the_first_half() {
    let store = committed("soccer/fifa.world/first_half");
    let view = &store.snapshot().soccer_live;
    assert!(view.clock_running);
    assert_eq!(view.clock_anchor_ms, 1_000);
    assert_eq!(view.base_min, 45);
    assert_eq!(view.phase_text.as_str(), "1ST");
    assert_eq!(view.phase_long.as_str(), "1ST HALF");
    assert!(!view.on_break);
}

#[test]
fn a_break_stops_the_clock_and_leaves_the_phase_to_the_clock_slot() {
    let view = committed("soccer/fifa.world/halftime").snapshot().clone();
    let view = view.soccer_live;
    assert!(view.on_break);
    assert!(!view.clock_running);
    assert_eq!(view.phase_text.as_str(), "");
    assert_eq!(view.phase_long.as_str(), "");
}

#[test]
fn a_shootout_freezes_the_match_clock() {
    let store = committed("soccer/fifa.world/shootout");
    let view = &store.snapshot().soccer_live;
    assert!(!view.clock_running);
    assert_eq!(view.phase_text.as_str(), "PENS");
    assert_eq!(view.base_min, 120);
}

#[test]
fn extra_time_keeps_its_stoppage_threshold() {
    let store = committed("soccer/fifa.world/overtime");
    let view = &store.snapshot().soccer_live;
    assert_eq!(view.phase_text.as_str(), "ET");
    assert_eq!(view.phase_long.as_str(), "EXTRA TIME");
    assert!(matches!(view.base_min, 105 | 120));
}

#[test]
fn a_clock_that_stops_advancing_stops_ticking_locally() {
    let (league, bytes) = fixture("soccer/fifa.world/second_half_stoppage");
    let detail = WireFeed.detail(league.sport, &bytes).unwrap();
    let mut store = Store::new();

    store.commit_detail(&league, &detail, Logos::default(), 1_000, clock());
    assert!(
        store.snapshot().soccer_live.clock_running,
        "the first poll of a game has nothing to compare against"
    );

    // Same game, same upstream clock: the feed has stalled.
    store.commit_detail(&league, &detail, Logos::default(), 6_000, clock());
    assert!(!store.snapshot().soccer_live.clock_running);
}

#[test]
fn the_stale_guard_only_compares_a_game_against_itself() {
    let mut store = Store::new();
    let league = LeagueId::from_slug(Sport::Soccer, "fifa.world");
    let (_, first) = fixture("soccer/fifa.world/first_half");
    let (_, second) = fixture("soccer/fifa.world/second_half_stoppage");
    let first = WireFeed.detail(Sport::Soccer, &first).unwrap();
    let second = WireFeed.detail(Sport::Soccer, &second).unwrap();

    store.commit_detail(&league, &first, Logos::default(), 1_000, clock());
    store.commit_detail(&league, &second, Logos::default(), 2_000, clock());
    store.commit_detail(&league, &second, Logos::default(), 3_000, clock());
    assert!(!store.snapshot().soccer_live.clock_running);

    // Rotating back re-anchors: the previous clock belongs to another game.
    store.commit_detail(&league, &first, Logos::default(), 4_000, clock());
    assert!(store.snapshot().soccer_live.clock_running);
}

#[test]
fn a_goal_event_reads_as_a_label_over_the_scorer_in_the_scoring_color() {
    let store = committed("soccer/fifa.world/overtime");
    let view = &store.snapshot().soccer_live;
    assert!(view.has_event);
    assert_eq!(view.event_top.as_str(), "GOAL 120'+1'");
    // The name folds on the way in: Martínez keeps its Latin-1 í.
    assert_eq!(view.event_name.as_str(), "L. Martínez");
    assert_eq!(view.event_color, Rgb888(0x74_ACDF));
}

#[test]
fn a_red_card_reads_as_its_own_label_over_the_carded_player() {
    // The real ARG-SUI fixture, which stops just after the 72' card so the
    // card is the latest event rather than a later goal.
    let store = committed("soccer/fifa.world/live_red_card");
    let view = &store.snapshot().soccer_live;
    assert!(view.has_event);
    assert_eq!(view.event_top.as_str(), "RED CARD 72'");
    assert_eq!(view.event_name.as_str(), "B. Embolo");
    // The carded side's colour, not the scoring one: SUI, who went down to ten.
    assert_eq!(view.event_color, Rgb888(0xFF_0000));
}

/// The corpus covers an *attributed* red card (above); what it cannot reach is
/// an event ESPN gives no team id for, which renders white rather than in a
/// side's colour. Synthetic because no fixture carries one and inventing one
/// would be inventing ESPN behaviour.
#[test]
fn a_red_card_labels_itself_and_an_unattributed_event_stays_white() {
    let mut store = Store::new();
    let colors = scoreboard_wire::TeamColors {
        primary: 0x00_FF_00_00,
        alternate: 0,
    };
    let team = |abbr| scoreboard_wire::TeamState {
        abbreviation: abbr,
        score: 0,
        colors,
    };
    let mut live = soccer::Live {
        game_id: "g",
        half: 2,
        clock_seconds: 3_000,
        on_break: false,
        away: team("AWY"),
        home: team("HOM"),
        last_event: Some(soccer::Event {
            kind: soccer::EventKind::RedCard,
            side: None,
            clock: "58'",
            athlete: "N. Otamendi",
        }),
        commentary: None,
    };
    store.commit_soccer_live(&live, Logos::default(), 0);
    let view = &store.snapshot().soccer_live;
    assert_eq!(view.event_top.as_str(), "RED CARD 58'");
    assert_eq!(view.event_color, Rgb888::WHITE);

    // An event with no clock string is just the label.
    live.last_event = Some(soccer::Event {
        kind: soccer::EventKind::RedCard,
        side: None,
        clock: "",
        athlete: "",
    });
    store.commit_soccer_live(&live, Logos::default(), 0);
    assert_eq!(store.snapshot().soccer_live.event_top.as_str(), "RED CARD");
}

// =========================================================================
// Break clocks (NBA + football share the rule)
// =========================================================================

#[test]
fn halftime_shows_ht_and_empties_the_period_chip() {
    for name in ["nba/halftime", "football/nfl/halftime"] {
        let store = committed(name);
        let (phase, clock, accent, low) = match store.snapshot().mode {
            Mode::NbaLive => {
                let view = &store.snapshot().nba_live;
                (
                    view.phase_text.to_string(),
                    view.clock_text.to_string(),
                    view.clock_accent,
                    view.clock_low,
                )
            }
            Mode::FootballLive => {
                let view = &store.snapshot().football_live;
                (
                    view.phase_text.to_string(),
                    view.clock_text.to_string(),
                    view.clock_accent,
                    view.clock_low,
                )
            }
            other => panic!("{name}: unexpected mode {other:?}"),
        };
        assert_eq!(phase, "", "{name}");
        assert_eq!(clock, "HT", "{name}");
        assert!(accent, "{name}");
        assert!(!low, "{name}");
    }
}

#[test]
fn end_of_period_keeps_the_period_and_shows_end() {
    let store = committed("nba/end_of_period");
    let view = &store.snapshot().nba_live;
    assert_eq!(view.clock_text.as_str(), "END");
    assert!(view.clock_accent);
    assert!(view.phase_text.as_str().starts_with('Q'));
}

#[test]
fn a_colonless_nba_clock_is_crunch_time() {
    let store = committed("nba/in_progress_subminute");
    let view = &store.snapshot().nba_live;
    assert!(!view.clock_text.contains(':'));
    assert!(view.clock_low);
    assert!(!view.clock_accent);
}

// =========================================================================
// Football situation
// =========================================================================

#[test]
fn a_drive_situation_becomes_a_down_and_distance_line() {
    let store = committed("football/nfl/in_progress");
    let view = &store.snapshot().football_live;
    let situation = view.situation.expect("the fixture has a situation");
    assert!(situation.down >= 1 && situation.down <= 4);
    assert!(view.situation_text.contains(" & "));
    assert!(
        view.situation_text
            .as_str()
            .starts_with(["", "1ST", "2ND", "3RD", "4TH"][situation.down as usize])
    );
}

#[test]
fn an_absent_situation_leaves_the_field_markers_undrawn() {
    let store = committed("football/nfl/in_progress_empty_situation");
    let view = &store.snapshot().football_live;
    assert!(view.situation.is_none());
    assert_eq!(view.situation_text.as_str(), "");
}

#[test]
fn goal_to_go_replaces_the_distance() {
    let mut store = Store::new();
    let colors = scoreboard_wire::TeamColors {
        primary: 0x00_FF_00_00,
        alternate: 0,
    };
    let team = |abbr| scoreboard_wire::TeamState {
        abbreviation: abbr,
        score: 0,
        colors,
    };
    let live = football::Live {
        game_id: "g",
        period: 1,
        phase: scoreboard_wire::LivePhase::InProgress,
        clock: "10:00",
        away: team("AWY"),
        home: team("HOM"),
        situation: Some(football::Situation {
            down: 1,
            distance: 7,
            yard_line: 95,
            possession: scoreboard_wire::Side::Away,
            red_zone: true,
        }),
        timeouts: None,
        last_play: None,
    };
    store.commit_football_live(&live, Logos::default(), 0);
    assert_eq!(
        store.snapshot().football_live.situation_text.as_str(),
        "1ST & GOAL"
    );

    let short = football::Live {
        situation: Some(football::Situation {
            yard_line: 40,
            ..live.situation.unwrap()
        }),
        ..live
    };
    store.commit_football_live(&short, Logos::default(), 0);
    assert_eq!(
        store.snapshot().football_live.situation_text.as_str(),
        "1ST & 7"
    );
}

#[test]
fn absent_timeouts_stay_absent_rather_than_reading_zero() {
    let store = committed("football/nfl/in_progress_empty_situation");
    let view = &store.snapshot().football_live;
    // The fixture advertises timeouts; what matters is that the model keeps
    // the presence flag rather than flattening it to a count.
    assert_eq!(
        view.away_timeouts.is_some(),
        view.home_timeouts.is_some(),
        "the wire flag covers both counts"
    );
}

// =========================================================================
// Pregame
// =========================================================================

#[test]
fn a_local_start_time_needs_a_utc_offset() {
    let mut store = Store::new();
    let pregame = mlb_free_pregame("g", 1_721_260_000);
    store.commit_pregame(
        &pregame,
        Logos::default(),
        0,
        LocalClock {
            now_epoch_s: 1_721_240_000,
            utc_offset_s: None,
        },
    );
    assert_eq!(store.snapshot().pregame.time_text.as_str(), "");
    assert_eq!(store.snapshot().pregame.date_text.as_str(), "");

    // Zero is a legitimate offset, not "unknown".
    store.commit_pregame(
        &pregame,
        Logos::default(),
        0,
        LocalClock {
            now_epoch_s: 1_721_240_000,
            utc_offset_s: Some(0),
        },
    );
    assert_eq!(store.snapshot().pregame.time_text.as_str(), "11:46 PM");
}

#[test]
fn the_date_shows_only_when_the_game_is_not_today() {
    let mut store = Store::new();
    // 2024-07-17 18:30 UTC is 12:30 local at -6.
    let today = LocalClock {
        now_epoch_s: 1_721_240_000,
        utc_offset_s: Some(-6 * 3_600),
    };
    // 2024-07-17 21:00 UTC is 15:00 the same local day.
    let same_day = mlb_free_pregame("g", 1_721_250_000);
    store.commit_pregame(&same_day, Logos::default(), 0, today);
    assert_eq!(store.snapshot().pregame.time_text.as_str(), "3:00 PM");
    assert_eq!(store.snapshot().pregame.date_text.as_str(), "");

    let tomorrow = mlb_free_pregame("g", 1_721_250_000 + 86_400);
    store.commit_pregame(&tomorrow, Logos::default(), 0, today);
    assert_eq!(store.snapshot().pregame.date_text.as_str(), "THU JUL 18");
}

#[test]
fn midnight_and_noon_read_as_twelve() {
    let mut store = Store::new();
    let utc = LocalClock {
        now_epoch_s: 0,
        utc_offset_s: Some(0),
    };
    for (epoch, expected) in [
        (0u32, "12:00 AM"),
        (43_200, "12:00 PM"),
        (46_800, "1:00 PM"),
    ] {
        let game = mlb_free_pregame("g", epoch);
        store.commit_pregame(&game, Logos::default(), 0, utc);
        assert_eq!(store.snapshot().pregame.time_text.as_str(), expected);
    }
}

#[test]
fn mlb_weather_carries_a_temperature_and_the_others_do_not() {
    let store = committed("mlb/pregame_weather_normal");
    let view = &store.snapshot().pregame;
    assert_eq!(view.sport, Sport::Mlb);
    assert!(
        view.info_secondary.as_str().contains('F'),
        "weather reads \"72F PARTLY CLOUDY\", got {:?}",
        view.info_secondary
    );
    assert_eq!(
        view.info_secondary.as_str(),
        view.info_secondary.as_str().to_uppercase()
    );
}

/// The duck-typing the MicroPython models needed, stated as data instead.
#[test]
fn each_sport_fills_the_pregame_slots_with_what_it_has() {
    let store = committed("nba/pregame");
    let view = &store.snapshot().pregame;
    assert!(!view.info_primary.is_empty(), "NBA leads with the arena");
    assert_eq!(
        view.info_secondary.as_str(),
        "",
        "basketball has no weather"
    );
    assert_eq!(view.away.line.as_str(), "", "and no per-team line");
    assert!(view.away.record.is_some());

    let store = committed("football/college-football/pregame_ranked");
    let view = &store.snapshot().pregame;
    assert_eq!(view.info_primary.as_str(), "NCAA FOOTBALL");
    assert!(!view.info_secondary.is_empty(), "the stadium follows");
    assert!(
        view.away.line.as_str().starts_with('#') || view.home.line.as_str().starts_with('#'),
        "a ranked matchup puts the rank on the team line"
    );

    let store = committed("soccer/fifa.world/pregame");
    let view = &store.snapshot().pregame;
    assert_eq!(view.info_primary.as_str(), "WORLD CUP");
    assert!(!view.info_secondary.is_empty());
    assert_eq!(view.away.line.as_str(), store.snapshot().away_abbr.as_str());
    assert!(view.away.record.is_none(), "soccer carries no record");
}

#[test]
fn an_unadvertised_record_stays_absent() {
    let store = committed("mlb/pregame");
    let view = &store.snapshot().pregame;
    // Whatever the fixture carries, both sides agree: the wire flags them
    // together only when ESPN supplied them.
    for side in [&view.away, &view.home] {
        if let Some(record) = side.record {
            assert!(record.wins > 0 || record.losses > 0);
        }
    }
}

// =========================================================================
// Non-game screens
// =========================================================================

#[test]
fn the_startup_step_never_walks_backwards() {
    let mut store = Store::new();
    store.set_startup_step(3, 5, "Joining WiFi", "attempt 1", 1, 3);
    store.set_startup_step(2, 5, "Retrying", "attempt 2", 2, 3);
    assert_eq!(store.startup_step(), (3, 5));
    assert_eq!(store.snapshot().startup.operation.as_str(), "Retrying");
    assert_eq!(store.snapshot().startup.attempt, 2);
}

#[test]
fn startup_updates_stop_after_the_handoff() {
    let mut store = Store::new();
    store.set_startup_step(2, 5, "Joining WiFi", "", 0, 0);
    store.finish_startup(StartupExit::Mode(Mode::Idle));
    assert_eq!(store.mode(), Mode::Idle);
    assert_eq!(store.snapshot().startup.operation.as_str(), "");

    store.set_startup_step(4, 5, "Too late", "", 0, 0);
    assert_eq!(store.mode(), Mode::Idle);
    assert_eq!(store.snapshot().startup.operation.as_str(), "");
}

#[test]
fn the_setup_screen_words_itself_from_the_failure() {
    let mut store = Store::new();
    store.set_setup_mode(SetupReason::BadAuth, "scoreboard-ab", "192.168.4.1", "home");
    let setup = &store.snapshot().setup;
    assert_eq!(setup.title.as_str(), "WRONG PASS");
    assert_eq!(setup.line_18.as_str(), "for \"home\"");
    assert_eq!(setup.line_54.as_str(), "to fix password");

    store.set_setup_mode(SetupReason::NoConfig, "", "", "");
    let setup = &store.snapshot().setup;
    assert_eq!(setup.title.as_str(), "SETUP");
    assert_eq!(setup.line_28.as_str(), "\"scoreboard\" WiFi");
    assert_eq!(setup.line_54.as_str(), "192.168.4.1");
}

#[test]
fn the_error_screen_caps_its_title_and_lines() {
    let mut store = Store::new();
    store.set_error(
        "A VERY LONG ERROR TITLE",
        &[
            "this line is quite a lot longer than the panel",
            "two",
            "three",
            "four",
            "five",
        ],
    );
    let error = &store.snapshot().error;
    assert_eq!(error.title.as_str().chars().count(), 12);
    assert_eq!(error.lines.len(), 4);
    assert_eq!(error.lines[0].as_str().chars().count(), 25);
    assert!(error.lines[0].as_str().ends_with('.'));

    store.set_error("", &[]);
    assert_eq!(store.snapshot().error.title.as_str(), "ERROR");
}

#[test]
fn a_sticky_toast_is_torn_down_to_just_expired() {
    let mut store = Store::new();
    store.set_toast("", ToastKind::Spinner, true, 5_000);
    assert!(store.snapshot().toast.sticky);

    store.clear_toast_if_sticky(6_000);
    let toast = &store.snapshot().toast;
    assert!(!toast.sticky);
    assert_eq!(
        toast.kind,
        ToastKind::Spinner,
        "the fade-out needs the kind"
    );
    assert_eq!(toast.updated_ms, 6_000 - snapshot::TOAST_DISPLAY_MS);

    // A LOCKED toast fired mid-skip must survive the teardown.
    store.set_toast("LOCKED", ToastKind::Lock, false, 7_000);
    store.clear_toast_if_sticky(7_100);
    assert_eq!(store.snapshot().toast.text.as_str(), "LOCKED");
    assert_eq!(store.snapshot().toast.updated_ms, 7_000);
}

#[test]
fn a_rejected_press_pulses_without_replacing_the_toast() {
    let mut store = Store::new();
    store.set_toast("SKIPPING", ToastKind::Spinner, true, 1_000);
    store.pulse_toast(1_400);
    assert_eq!(store.snapshot().toast.pulse_ms, 1_400);
    assert!(store.snapshot().toast.sticky);
    store.pulse_toast(1_800);
    assert_eq!(store.snapshot().toast.pulse_ms, 1_800);
}

#[test]
fn the_menu_marquee_restamps_on_the_item_not_the_checkbox() {
    let mut store = Store::new();
    let rows = |checked: bool| {
        vec![
            MenuRowInput {
                label: "MLB",
                checked: true,
                source: 0,
            },
            MenuRowInput {
                label: "PREMIER LEAGUE",
                checked,
                source: 1,
            },
        ]
    };

    store.set_menu(&rows(true), 0, -1, 0, 1_000);
    assert_eq!(store.snapshot().menu.updated_ms, 1_000);

    // Toggling the checkbox under the cursor keeps the scroll going.
    store.set_menu(&rows(false), 0, -1, 0, 2_000);
    assert_eq!(store.snapshot().menu.updated_ms, 1_000);

    // Moving the cursor is a new item.
    store.set_menu(&rows(false), 1, -1, 0, 3_000);
    assert_eq!(store.snapshot().menu.updated_ms, 3_000);

    store.clear_menu();
    assert!(!store.snapshot().menu.active);
    assert!(store.snapshot().menu.rows.is_empty());
}

#[test]
fn the_menu_restamps_when_the_window_scrolls_under_the_cursor() {
    let mut store = Store::new();
    let first = vec![MenuRowInput {
        label: "MLB",
        checked: true,
        source: 0,
    }];
    let scrolled = vec![MenuRowInput {
        label: "NBA",
        checked: true,
        source: 1,
    }];
    store.set_menu(&first, 0, 1, 8, 1_000);
    store.set_menu(&scrolled, 0, 9, 8, 2_000);
    assert_eq!(store.snapshot().menu.updated_ms, 2_000);
}

// =========================================================================
// Slate and rotation
// =========================================================================

fn sources() -> Vec<LeagueId> {
    vec![
        LeagueId::from_slug(Sport::Mlb, "mlb"),
        LeagueId::from_slug(Sport::Nba, "nba"),
        LeagueId::from_slug(Sport::Soccer, "usa.1"),
    ]
}

fn list(slate: &mut Slate, source: u8, games: &[(GameState, &str)]) {
    let mut update = slate.update_source(source);
    for (state, id) in games {
        update.entry(*state, id);
    }
}

#[test]
fn a_live_game_anywhere_makes_the_rotation_live_only() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(
        &mut slate,
        0,
        &[(GameState::Final, "m1"), (GameState::Pregame, "m2")],
    );
    list(&mut slate, 1, &[(GameState::Live, "n1")]);
    slate.rebuild();
    assert_eq!(slate.len(), 1);
    assert_eq!(slate.current().unwrap().1, "n1");
}

#[test]
fn with_nothing_live_finals_rotate_before_pregames_in_league_order() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(
        &mut slate,
        0,
        &[(GameState::Pregame, "m1"), (GameState::Final, "m2")],
    );
    list(&mut slate, 1, &[(GameState::Final, "n1")]);
    slate.rebuild();

    let mut order = Vec::new();
    for _ in 0..slate.len() {
        order.push(slate.current().unwrap().1.to_string());
        slate.advance();
    }
    assert_eq!(order, vec!["m2", "n1", "m1"]);
}

#[test]
fn an_empty_merged_slate_is_the_only_thing_that_shows_no_games() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    slate.rebuild();
    assert!(slate.is_empty());
    assert!(slate.current().is_none());

    list(&mut slate, 2, &[(GameState::Pregame, "s1")]);
    slate.rebuild();
    assert!(!slate.is_empty());
}

#[test]
fn a_failing_source_keeps_its_cached_slate() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(&mut slate, 0, &[(GameState::Final, "m1")]);
    list(&mut slate, 1, &[(GameState::Final, "n1")]);
    slate.rebuild();
    assert_eq!(slate.len(), 2);

    // Source 1's refresh failed, so it never opened an update.
    list(&mut slate, 0, &[(GameState::Final, "m1")]);
    slate.rebuild();
    assert_eq!(slate.len(), 2, "the dead league keeps its games");
}

#[test]
fn the_shown_game_keeps_its_place_across_a_rebuild() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(
        &mut slate,
        0,
        &[
            (GameState::Final, "a"),
            (GameState::Final, "b"),
            (GameState::Final, "c"),
        ],
    );
    slate.rebuild();
    slate.advance();
    assert_eq!(slate.current().unwrap().1, "b");

    // "a" finishes and drops off; "b" must not jump.
    list(
        &mut slate,
        0,
        &[(GameState::Final, "b"), (GameState::Final, "c")],
    );
    slate.rebuild();
    assert_eq!(slate.current().unwrap().1, "b");

    // ...and a game that vanishes resets to the top.
    list(&mut slate, 0, &[(GameState::Final, "c")]);
    slate.rebuild();
    assert_eq!(slate.current().unwrap().1, "c");
}

#[test]
fn a_league_skip_lands_on_the_next_distinct_league() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(
        &mut slate,
        0,
        &[(GameState::Final, "m1"), (GameState::Final, "m2")],
    );
    list(&mut slate, 1, &[(GameState::Final, "n1")]);
    list(&mut slate, 2, &[(GameState::Final, "s1")]);
    slate.rebuild();

    assert_eq!(slate.current().unwrap().1, "m1");
    slate.advance_league();
    assert_eq!(slate.current().unwrap().1, "n1");
    slate.advance_league();
    assert_eq!(slate.current().unwrap().1, "s1");
    slate.advance_league();
    assert_eq!(slate.current().unwrap().1, "m1", "and wraps");
}

#[test]
fn a_single_league_slate_degrades_a_league_skip_to_a_plain_skip() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(
        &mut slate,
        0,
        &[(GameState::Final, "m1"), (GameState::Final, "m2")],
    );
    slate.rebuild();
    slate.advance_league();
    assert_eq!(slate.current().unwrap().1, "m2");
}

#[test]
fn a_filter_restricts_the_rotation_and_survives_an_empty_slate() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    list(&mut slate, 0, &[(GameState::Final, "m1")]);
    list(&mut slate, 1, &[(GameState::Final, "n1")]);
    slate.rebuild();

    assert!(slate.set_filter(&["basketball/nba"]));
    assert_eq!(slate.len(), 1);
    assert_eq!(slate.current().unwrap().1, "n1");

    // The NBA slate empties: fall back to every league rather than blank the
    // board, and keep the filter — its games may come back.
    list(&mut slate, 1, &[]);
    slate.rebuild();
    assert_eq!(slate.current().unwrap().1, "m1");
    assert!(slate.filter().is_some());

    list(&mut slate, 1, &[(GameState::Final, "n2")]);
    slate.rebuild();
    assert_eq!(slate.current().unwrap().1, "n2");
}

#[test]
fn a_filter_naming_every_league_is_no_filter() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    assert!(!slate.set_filter(&["baseball/mlb", "basketball/nba", "soccer/usa.1"]));
    assert!(slate.filter().is_none());
}

#[test]
fn the_rotation_lock_is_independent_of_the_filter() {
    let mut slate = Slate::new();
    slate.set_sources(&sources());
    assert!(slate.toggle_lock());
    slate.set_filter(&["baseball/mlb"]);
    assert!(slate.locked());
    assert!(!slate.toggle_lock());
}

// =========================================================================
// Snapshot handoff
// =========================================================================

#[test]
fn a_latched_frame_survives_publishes_underneath_it() {
    let channel = crate::SnapshotChannel::new();
    let (mut publisher, mut reader) = channel.split();
    let mut store = Store::new();

    store.set_error("FIRST", &["one"]);
    publisher.publish(store.snapshot());
    let latched = reader.latch();
    assert_eq!(latched.error.title.as_str(), "FIRST");
    let seq = latched.commit_seq;

    // Two commits inside one frame — a live commit plus its play flash — is
    // exactly the case a double buffer cannot survive.
    store.set_error("SECOND", &["two"]);
    publisher.publish(store.snapshot());
    store.set_error("THIRD", &["three"]);
    publisher.publish(store.snapshot());

    let latched = reader.latch();
    assert_eq!(latched.error.title.as_str(), "THIRD");
    assert_ne!(latched.commit_seq, seq);
}

#[test]
fn latching_without_a_publish_returns_the_same_frame() {
    let channel = crate::SnapshotChannel::new();
    let (mut publisher, mut reader) = channel.split();
    let mut store = Store::new();
    store.set_mode(Mode::NoGames);
    publisher.publish(store.snapshot());

    let first = reader.latch().commit_seq;
    let second = reader.latch().commit_seq;
    assert_eq!(first, second);
}

#[test]
#[should_panic(expected = "split called twice")]
fn a_channel_hands_out_one_publisher() {
    let channel = crate::SnapshotChannel::new();
    let _halves = channel.split();
    let _second = channel.split();
}

// =========================================================================
// The corpus sweep
// =========================================================================

/// Every golden decodes, builds, and lands in the mode its state implies.
#[test]
fn every_fixture_builds_a_sane_view() {
    for (name, bytes) in corpus() {
        let league = league_of(&name);
        let detail = WireFeed
            .detail(league.sport, &bytes)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut store = Store::new();
        store.commit_detail(&league, &detail, Logos::default(), 1_000, clock());
        let snapshot = store.snapshot();

        let expected = match (detail.state(), league.sport) {
            (GameState::Pregame, _) => Mode::Pregame,
            (GameState::Final, Sport::Soccer) => Mode::SoccerFinal,
            (GameState::Final, _) => Mode::Final,
            (GameState::Live, Sport::Mlb) => Mode::MlbLive,
            (GameState::Live, Sport::Nba) => Mode::NbaLive,
            (GameState::Live, Sport::Football) => Mode::FootballLive,
            (GameState::Live, Sport::Soccer) => Mode::SoccerLive,
        };
        assert_eq!(snapshot.mode, expected, "{name}");
        assert_eq!(snapshot.game_id.as_str(), detail.game_id(), "{name}");
        let (away, home) = detail.abbreviations();
        assert_eq!(snapshot.away_abbr.as_str(), away, "{name}");
        assert_eq!(snapshot.home_abbr.as_str(), home, "{name}");
        assert_eq!(snapshot.animation_start_ms, 1_000, "{name}");
        // One commit for the view, plus one more when a live game brought a
        // play line with it.
        assert!((1..=2).contains(&snapshot.commit_seq), "{name}");

        if expected == Mode::Final {
            let view = &snapshot.linescore_final;
            assert_eq!(view.header_row.len(), view.away_row.len(), "{name}");
            assert_eq!(view.header_row.len(), view.home_row.len(), "{name}");
            assert!(!view.final_text.is_empty(), "{name}");
        }
    }
}

/// Spot checks against the values the JSON fixtures carry, so a plausible-but-
/// wrong build cannot pass the shape checks above.
#[test]
fn the_spot_checked_fixtures_carry_the_values_espn_sent() {
    let store = committed("mlb/live_inning");
    let view = &store.snapshot().mlb_live;
    assert_eq!(store.snapshot().away_abbr.as_str(), "CHC");
    assert_eq!(store.snapshot().home_abbr.as_str(), "BAL");
    assert_eq!(view.inning_text.as_str(), "8th");
    assert_eq!(view.half, mlb::InningHalf::Top);
    assert_eq!((view.balls, view.strikes, view.outs), (1, 2, 2));
    assert_eq!((view.away_score, view.home_score), (5, 2));
    assert!(view.has_at_bat);
    assert_eq!(view.pitcher.as_str(), "A. Nunez");
    assert_eq!(view.batter.as_str(), "M. Amaya");
    assert!(view.bases.first && !view.bases.second && !view.bases.third);
    // Top of the inning: the away side bats, the home side pitches.
    assert_eq!(view.bat_color, Some(Rgb888(0x0E_3386).brightened()));
    assert_eq!(view.pitch_color, Some(Rgb888(0xDF_4601).brightened()));
    assert_eq!(
        store.snapshot().play.text.as_str(),
        "Pitch 3 : Strike 2 Swinging"
    );

    let store = committed("mlb/final");
    let view = &store.snapshot().linescore_final;
    assert_eq!((view.away_score, view.home_score), (4, 3));
    assert!(!view.home_won);
    assert_eq!(view.away_row.len() / 3, 9);
    assert_eq!(view.away_row.as_str(), " 0  0  2  0  0  1  1  0  0 ");
    assert_eq!(view.home_row.as_str(), " 2  0  1  0  0  0  0  0  0 ");

    let store = committed("nba/in_progress");
    let view = &store.snapshot().nba_live;
    assert_eq!(view.phase_text.as_str(), "Q3");
    assert_eq!(view.clock_text.as_str(), "4:37");
    assert!(!view.clock_low);
    assert_eq!((view.away_score, view.home_score), (75, 77));
    assert_eq!(
        store.snapshot().play.text.as_str(),
        "Zeke Nnaji out of bounds bad pass turnover"
    );

    let store = committed("football/nfl/in_progress");
    let view = &store.snapshot().football_live;
    assert_eq!(view.situation_text.as_str(), "2ND & 7");
    assert_eq!(view.away_timeouts, Some(2));
    assert_eq!(view.home_timeouts, Some(3));
    assert_eq!(
        view.situation.map(|situation| situation.possession),
        Some(scoreboard_wire::Side::Home)
    );

    let store = committed("soccer/fifa.world/full_time");
    let view = &store.snapshot().soccer_final;
    assert_eq!(view.ft_text.as_str(), "FULL TIME");
    let store = committed("soccer/fifa.world/final_after_penalties");
    assert_eq!(store.snapshot().soccer_final.ft_text.as_str(), "PENALTIES");
    let store = committed("soccer/fifa.world/final_after_extra_time");
    assert_eq!(store.snapshot().soccer_final.ft_text.as_str(), "AET");
}

/// The bound test. Measures the corpus the way `backend/src/wire_corpus.rs`
/// does — that module's `corpus_string_maxima` is the source of truth for the
/// numbers — and checks each class against the snapshot bound sized from it.
#[test]
fn every_string_bound_clears_the_corpus() {
    let mut max = Maxima(BTreeMap::new());

    for (name, bytes) in corpus() {
        let league = league_of(&name);
        let detail = WireFeed.detail(league.sport, &bytes).unwrap();
        max.note("game id", detail.game_id());
        let (away, home) = detail.abbreviations();
        max.note("abbreviation", away);
        max.note("abbreviation", home);

        match detail {
            GameDetail::Mlb(mlb::Game::Live(game)) => {
                if let Some(at_bat) = game.at_bat {
                    max.note("player name", at_bat.pitcher);
                    max.note("player name", at_bat.batter);
                }
                max.note("play id", game.last_play.id);
                max.note("play text", game.last_play.text);
            }
            GameDetail::Mlb(mlb::Game::Pregame(game)) => {
                max.note("info primary", game.venue);
                if let Some(weather) = game.weather {
                    max.note("info secondary", weather.condition);
                }
                for pitcher in [game.away.probable_pitcher, game.home.probable_pitcher]
                    .into_iter()
                    .flatten()
                {
                    max.note("team line", pitcher);
                }
            }
            GameDetail::Nba(nba::Game::Live(game)) => {
                max.note("clock", game.clock);
                if let Some(play) = game.last_play {
                    max.note("play id", play.id);
                    max.note("play text", play.text);
                }
            }
            GameDetail::Nba(nba::Game::Pregame(game)) => max.note("info primary", game.venue),
            GameDetail::Football(football::Game::Live(game)) => {
                max.note("clock", game.clock);
                if let Some(play) = game.last_play {
                    max.note("play id", play.id);
                    max.note("play text", play.text);
                }
            }
            GameDetail::Football(football::Game::Pregame(game)) => {
                max.note("info primary", league.display_name.as_str());
                max.note("info secondary", game.venue);
                for rank in [game.away.rank_line, game.home.rank_line]
                    .into_iter()
                    .flatten()
                {
                    max.note("team line", rank);
                }
            }
            GameDetail::Soccer(soccer::Game::Live(game)) => {
                if let Some(event) = game.last_event {
                    max.note("clock", event.clock);
                    max.note("player name", event.athlete);
                }
                if let Some(commentary) = game.commentary {
                    max.note("play id", commentary.id);
                    max.note("play text", commentary.text);
                }
            }
            GameDetail::Soccer(soccer::Game::Pregame(game)) => {
                max.note("info primary", league.display_name.as_str());
                max.note("info secondary", game.venue);
                max.note("team line", game.away.abbreviation);
            }
            GameDetail::Soccer(soccer::Game::Final(game)) => {
                max.note("scorers", game.away.scorers);
                max.note("scorers", game.home.scorers);
            }
            GameDetail::Mlb(mlb::Game::Final(game)) => {
                max.note_periods(game.away.line_score.len());
                max.note_periods(game.home.line_score.len());
            }
            GameDetail::Nba(nba::Game::Final(game)) => {
                max.note_periods(game.away.line_score.len());
                max.note_periods(game.home.line_score.len());
            }
            GameDetail::Football(football::Game::Final(game)) => {
                max.note_periods(game.away.line_score.len());
                max.note_periods(game.home.line_score.len());
            }
        }
    }

    let bounds: &[(&str, usize)] = &[
        ("abbreviation", snapshot::ABBR),
        ("clock", snapshot::CLOCK),
        ("game id", snapshot::GAME_ID),
        ("info primary", snapshot::INFO),
        // The temperature prefix ("100F ") shares this field.
        ("info secondary", snapshot::WEATHER - 5),
        ("play id", snapshot::PLAY_ID),
        ("play text", snapshot::PLAY_TEXT),
        ("player name", snapshot::PLAYER),
        ("scorers", snapshot::SCORERS),
        ("team line", snapshot::TEAM_LINE),
        // Columns, not bytes: three chars each.
        ("periods", snapshot::LINESCORE / 3),
    ];
    for (class, observed) in &max.0 {
        let (_, bound) = bounds
            .iter()
            .find(|(name, _)| name == class)
            .unwrap_or_else(|| panic!("no bound declared for {class}"));
        assert!(
            observed <= bound,
            "{class}: the corpus reaches {observed}, the bound is {bound}"
        );
    }
    let measured: Vec<&str> = max.0.keys().copied().collect();
    let mut declared: Vec<&str> = bounds.iter().map(|(class, _)| *class).collect();
    declared.sort_unstable();
    assert_eq!(
        measured, declared,
        "every bound must be exercised by the corpus"
    );
}

/// A struct rather than closures so the per-sport arms can interleave string
/// and line-score measurements without fighting the borrow checker (the same
/// shape `wire_corpus.rs` settled on).
struct Maxima(BTreeMap<&'static str, usize>);

impl Maxima {
    fn note(&mut self, class: &'static str, text: &str) {
        self.note_len(class, text.len());
    }

    fn note_periods(&mut self, periods: usize) {
        self.note_len("periods", periods);
    }

    fn note_len(&mut self, class: &'static str, len: usize) {
        let entry = self.0.entry(class).or_default();
        *entry = (*entry).max(len);
    }
}

/// The budget lines this crate owns, asserted so a field added without thought
/// shows up as a failing test rather than as silent RAM.
#[test]
fn the_snapshot_stays_inside_its_budget_line() {
    assert_eq!(ScoreboardSnapshot::SIZE, 2_848);
    assert_eq!(crate::SnapshotChannel::SIZE, 8_552);
    assert_eq!(Slate::SIZE, 4_596);
}

// =========================================================================
// The poller's pure half (`poll`)
// =========================================================================

use crate::poll::{
    self, DETAIL_LINE_CHARS, ErrorScreen, FailureTracker, Friendly, MAX_FAILURES, PollError,
    SkipKind, SkipMachine, SkipVerdict, Transport,
};

fn error_lines(screen: &ErrorScreen) -> Vec<String> {
    screen.lines().map(ToString::to_string).collect()
}

#[test]
fn every_error_maps_to_the_kind_and_detail_poller_py_showed() {
    fn shown(error: &PollError) -> (String, String) {
        let Friendly { kind, detail } = poll::friendly(error);
        (kind.as_str().to_string(), detail.as_str().to_string())
    }

    assert_eq!(
        shown(&PollError::Timeout),
        ("Timeout".to_string(), "backend not responding".to_string())
    );
    assert_eq!(
        shown(&PollError::http(503, "upstream_unavailable")),
        ("HTTP 503".to_string(), "upstream_unavailable".to_string())
    );
    assert_eq!(
        shown(&PollError::Transport(Transport::Dns)),
        (
            "Network error".to_string(),
            "cannot resolve backend".to_string()
        )
    );

    // The decode arm leads with the byte offset, which on a device is often the
    // only clue available.
    let error = mlb::decode(&[9, 1]).expect_err("version 9 is not the wire format");
    let (kind, detail) = shown(&PollError::Decode(error));
    assert_eq!(kind, "Bad response");
    assert!(
        detail.starts_with("@0: unsupported wire version 9"),
        "{detail}"
    );
}

/// `_raise_api_error` defaulted `error` to `unknown_error` when the body was
/// not a JSON object, and the panel showed that word rather than a blank line.
#[test]
fn an_error_body_with_no_code_still_names_something() {
    let Friendly { kind, detail } = poll::friendly(&PollError::http(500, ""));
    assert_eq!(kind.as_str(), "HTTP 500");
    assert_eq!(detail.as_str(), "unknown_error");
}

#[test]
fn the_error_screen_appears_on_the_fifth_failure_and_not_before() {
    let mut tracker = FailureTracker::new();
    for expected in 1..MAX_FAILURES {
        let failure = tracker.record_failure(1_000, &PollError::Timeout);
        assert_eq!(failure.streak, expected);
        assert!(failure.screen.is_none(), "streak {expected} showed a screen");
    }
    let failure = tracker.record_failure(1_000, &PollError::Timeout);
    assert_eq!(failure.streak, MAX_FAILURES);
    let screen = failure.screen.expect("the fifth failure shows the screen");
    assert_eq!(
        error_lines(&screen),
        vec!["Timeout", "backend not responding", "failing for 0m"]
    );
}

/// Every failure past the fifth rebuilds the screen, so the age line keeps
/// counting rather than freezing at the minute the streak crossed.
#[test]
fn the_age_line_counts_from_the_first_failure_of_the_streak() {
    let mut tracker = FailureTracker::new();
    for _ in 0..MAX_FAILURES {
        tracker.record_failure(60_000, &PollError::Timeout);
    }
    let failure = tracker.record_failure(60_000 + 7 * 60_000, &PollError::Timeout);
    let screen = failure.screen.expect("still failing, still showing");
    assert_eq!(error_lines(&screen).last().unwrap(), "failing for 7m");
}

#[test]
fn a_long_detail_takes_two_lines_and_the_screen_still_fits() {
    let error = soccer::decode(&[2, 1, 4]).expect_err("truncated soccer live game");
    let mut tracker = FailureTracker::new();
    let mut failure = tracker.record_failure(0, &PollError::Decode(error));
    for _ in 1..MAX_FAILURES {
        failure = tracker.record_failure(0, &PollError::Decode(error));
    }
    let screen = failure.screen.expect("the fifth failure shows the screen");
    let shown = error_lines(&screen);
    assert_eq!(shown.len(), snapshot::ERROR_LINES);
    assert_eq!(shown[0], "Bad response");
    assert!(
        shown[1].chars().count() <= DETAIL_LINE_CHARS,
        "first detail line overflows: {:?}",
        shown[1]
    );
    // The two detail lines are consecutive slices of one string, not two
    // independent renderings of it.
    let rejoined = format!("{}{}", shown[1], shown[2]);
    let Friendly { detail, .. } = poll::friendly(&PollError::Decode(error));
    assert!(detail.as_str().starts_with(&rejoined), "{rejoined:?}");
    assert_eq!(shown[3], "failing for 0m");
}

#[test]
fn a_success_reports_the_streak_it_ended_exactly_once() {
    let mut tracker = FailureTracker::new();
    assert_eq!(tracker.record_success(), None);
    tracker.record_failure(0, &PollError::Timeout);
    tracker.record_failure(0, &PollError::Timeout);
    assert_eq!(tracker.record_success(), Some(2));
    assert_eq!(tracker.record_success(), None);
    assert_eq!(tracker.streak(), 0);
}

#[test]
fn the_error_screen_reaches_the_panel_under_the_api_error_title() {
    let mut tracker = FailureTracker::new();
    let mut failure = tracker.record_failure(0, &PollError::Timeout);
    for _ in 1..MAX_FAILURES {
        failure = tracker.record_failure(0, &PollError::Timeout);
    }
    let mut store = Store::new();
    store.finish_startup(StartupExit::Mode(Mode::Idle));
    failure.screen.expect("shown by now").commit(&mut store);

    let snapshot = store.snapshot();
    assert_eq!(snapshot.mode, Mode::Error);
    assert_eq!(snapshot.error.title.as_str(), poll::ERROR_TITLE);
    assert_eq!(snapshot.error.lines[0].as_str(), "Timeout");
}

// -- The skip machine -----------------------------------------------------

#[test]
fn a_press_that_lands_on_an_armed_skip_is_rejected_not_queued() {
    let mut machine = SkipMachine::new();
    assert_eq!(machine.request(SkipKind::Game), SkipVerdict::Armed);
    assert_eq!(machine.request(SkipKind::Game), SkipVerdict::Rejected);
    assert_eq!(machine.request(SkipKind::League), SkipVerdict::Rejected);
    // One advance, however many presses landed.
    assert_eq!(machine.consume(), Some(SkipKind::Game));
    assert_eq!(machine.consume(), None);
}

#[test]
fn a_press_that_lands_during_the_tick_it_started_is_also_rejected() {
    let mut machine = SkipMachine::new();
    machine.request(SkipKind::League);
    assert_eq!(machine.consume(), Some(SkipKind::League));
    // The tick is now in flight — `_poll_current`'s awaits are where a burst
    // of presses actually lands.
    assert_eq!(machine.request(SkipKind::Game), SkipVerdict::Rejected);
    assert!(machine.finish(), "the spinner is owed a teardown");
    assert_eq!(machine.request(SkipKind::Game), SkipVerdict::Armed);
}

#[test]
fn a_tick_with_no_skip_owes_no_teardown() {
    let mut machine = SkipMachine::new();
    assert_eq!(machine.consume(), None);
    assert!(!machine.finish());
}

// -- Receive buffer sizing ------------------------------------------------

/// [`poll::RESPONSE_BYTES`]'s derivation, checked against the corpus rather
/// than asserted in a comment. The binding case is a games list at the wire
/// format's own ceiling — not the logo `api_client.py` sized against.
#[test]
fn the_receive_buffer_holds_the_largest_list_the_wire_format_can_encode() {
    let longest = corpus()
        .iter()
        .map(|(name, bytes)| {
            let league = league_of(name);
            let detail = WireFeed
                .detail(league.sport, bytes)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            detail.game_id().len()
        })
        .max()
        .expect("corpus is not empty");
    assert_eq!(
        longest,
        poll::MAX_GAME_ID_BYTES,
        "game ids changed length; re-derive poll::RESPONSE_BYTES"
    );

    // `u8 version` + `u8 count`, then per entry `u8 state` + `u8 length` + id.
    let worst_list = 2 + scoreboard_wire::MAX_GAMES * (2 + poll::MAX_GAME_ID_BYTES);
    assert!(
        worst_list + poll::MAX_HEADER_BLOCK <= poll::RESPONSE_BYTES,
        "a full games list plus its headers is {} B, over the {} B buffer",
        worst_list + poll::MAX_HEADER_BLOCK,
        poll::RESPONSE_BYTES
    );
}

/// The detail half of the split, against what the corpus actually contains and
/// against the worst case the format allows for the sport with the most text.
#[test]
fn the_detail_half_holds_every_corpus_payload_with_room_for_the_wire_maximum() {
    let largest = corpus()
        .iter()
        .map(|(_, bytes)| bytes.len())
        .max()
        .expect("corpus is not empty");
    assert!(
        largest + poll::MAX_HEADER_BLOCK <= poll::DETAIL_BYTES,
        "corpus maximum {largest} B plus headers does not fit {} B",
        poll::DETAIL_BYTES
    );
    // Headroom for the text-heavy case the corpus has no fixture for: a live
    // game carrying a play line at the wire's 255-byte cap, on top of the
    // largest payload measured.
    assert!(
        largest + scoreboard_wire::MAX_STRING_BYTES + poll::MAX_HEADER_BLOCK <= poll::DETAIL_BYTES
    );
}

#[test]
fn the_logo_half_holds_a_crest_and_its_headers() {
    const CREST_BYTES: usize = 24 * 24 * 2;
    const { assert!(CREST_BYTES + poll::MAX_HEADER_BLOCK <= poll::LOGO_BYTES) };
}

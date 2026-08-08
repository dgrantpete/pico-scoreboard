//! The game-facing screens, driven through the hub75 simulator, plus the
//! derivations they read out of the prepared view.

mod goldens;

use goldens::PULSE_CASES;
use hub75::display::{FrameBytes, Hub75Display};
use hub75::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};
use hub75::sim::SimulatorSink;
use scoreboard_model::snapshot::{
    Bases, FieldSituation, InningHalf, LogoRef, Record, Side, ToastView,
};
use scoreboard_model::{Mode, Rgb888, ScoreboardSnapshot, Sport, Text, ToastKind};
use scoreboard_render::blit::Canvas;
use scoreboard_render::game::football::Field;
use scoreboard_render::game::{LOGO_BYTES, LogoSlot, Logos, Scene};
use scoreboard_render::geometry::{self, RenderSettings};
use scoreboard_render::prepared::{PregameLine, PreparedView};
use scoreboard_render::time::{FrameElapsed, WallMs};
use scoreboard_render::{BLACK, DIM_GRAY, SkipMemo, frame, pack};

const AWAY_TEAM: Rgb888 = Rgb888::new(255, 0, 0);
const HOME_TEAM: Rgb888 = Rgb888::new(0, 0, 255);
/// A crest color no screen uses for anything else, so finding it proves the
/// pool was consulted.
const CREST: u16 = 0x07E0;

struct Frame(Vec<u8>);

impl Frame {
    fn pixel(&self, x: i32, y: i32) -> u16 {
        let index = (y as usize * WIDTH + x as usize) * 2;
        u16::from_le_bytes([self.0[index], self.0[index + 1]])
    }

    fn lit(&self, x: i32, y: i32) -> bool {
        self.pixel(x, y) != BLACK
    }

    fn count(&self, x: core::ops::Range<i32>, y: core::ops::Range<i32>, color: u16) -> usize {
        y.flat_map(|row| x.clone().map(move |col| (col, row)))
            .filter(|(col, row)| self.pixel(*col, *row) == color)
            .count()
    }

    fn lit_in(&self, x: core::ops::Range<i32>, y: core::ops::Range<i32>) -> usize {
        y.flat_map(|row| x.clone().map(move |col| (col, row)))
            .filter(|(col, row)| self.lit(*col, *row))
            .count()
    }
}

/// One crest slot, filled with a single flat color.
fn crest_pool() -> [LogoSlot; 1] {
    let mut slot: LogoSlot = [0; LOGO_BYTES];
    for pixel in slot.chunks_exact_mut(2) {
        pixel.copy_from_slice(&CREST.to_le_bytes());
    }
    [slot]
}

struct Harness {
    snapshot: ScoreboardSnapshot,
    settings: RenderSettings,
    prepared: PreparedView,
    pool: [LogoSlot; 1],
    now: WallMs,
    view: FrameElapsed,
    play: FrameElapsed,
}

impl Harness {
    fn new(mode: Mode) -> Self {
        let mut snapshot = ScoreboardSnapshot::new();
        snapshot.mode = mode;
        snapshot.commit_seq = 1;
        snapshot.ui_colors.primary = Rgb888::new(255, 255, 255);
        snapshot.ui_colors.accent = Rgb888::new(0, 255, 0);
        snapshot.ui_colors.secondary = Rgb888::new(0, 0, 255);
        snapshot.ui_colors.clock_normal = Rgb888::new(200, 200, 200);
        snapshot.ui_colors.clock_warning = Rgb888::new(255, 255, 0);
        snapshot.game_id = text("401581000");
        snapshot.away_logo = Some(LogoRef(0));
        snapshot.home_logo = Some(LogoRef(0));
        Harness {
            snapshot,
            settings: RenderSettings::new(),
            prepared: PreparedView::new(),
            pool: crest_pool(),
            now: WallMs(10_000),
            view: FrameElapsed(0),
            play: FrameElapsed(0),
        }
    }

    fn draw(&mut self) -> Frame {
        self.prepared.sync(&self.snapshot, &self.settings);
        let scene = Scene {
            snapshot: &self.snapshot,
            prepared: &self.prepared,
            settings: &self.settings,
            logos: Logos::new(&self.pool),
            now: self.now,
            view: self.view,
            play: self.play,
        };
        let mut buffer: Box<FrameBytes> = Box::new([0; RGB565_FRAME_BYTES]);
        let mut display = Hub75Display::new(&mut buffer, SimulatorSink::new());
        {
            let mut canvas = Canvas::new(display.buffer_mut(), WIDTH as i32, HEIGHT as i32);
            frame::render(&mut canvas, &scene);
        }
        display.show();
        Frame(display.sink_mut().front().to_vec())
    }
}

fn text<const N: usize>(value: &str) -> Text<N> {
    let mut out = Text::new();
    out.push_str(value).expect("test text fits");
    out
}

// -- MLB ---------------------------------------------------------------------

fn mlb() -> Harness {
    let mut harness = Harness::new(Mode::MlbLive);
    let view = &mut harness.snapshot.mlb_live;
    view.half = InningHalf::Top;
    view.inning_text = text("7th");
    view.away_score = 3;
    view.home_score = 5;
    view.balls = 1;
    view.strikes = 1;
    view.outs = 1;
    view.bases = Bases {
        first: true,
        second: false,
        third: true,
    };
    view.pitch_color = Some(HOME_TEAM);
    view.bat_color = Some(AWAY_TEAM);
    view.pitcher = text("G. MARQUEZ");
    view.batter = text("R. LUKAKU");
    view.has_at_bat = true;
    harness
}

#[test]
fn the_mlb_screen_draws_its_frame_sprites_and_crests() {
    let frame = mlb().draw();
    let table = geometry::MLB_LIVE;

    assert_eq!(
        frame.pixel(table.divider_x, 30),
        DIM_GRAY,
        "the column rule"
    );
    assert_eq!(
        frame.pixel(table.divider_x + 4, table.separator_y),
        DIM_GRAY,
        "the rule under the data column"
    );
    assert_eq!(frame.pixel(0, 0), CREST, "the away crest");
    assert_eq!(frame.pixel(0, 40), CREST, "the home crest");
    assert!(
        frame.lit_in(46..128, 0..36) > 0,
        "the diamond and count block"
    );
}

#[test]
fn an_mlb_screen_without_a_game_falls_back_to_idle() {
    let mut harness = mlb();
    harness.snapshot.game_id = Text::new();
    let empty = harness.draw();

    let mut idle = Harness::new(Mode::Idle);
    idle.snapshot.ui_colors = harness.snapshot.ui_colors;
    assert_eq!(empty.0, idle.draw().0);
}

#[test]
fn the_base_markers_take_the_batting_sides_color() {
    let mut harness = mlb();
    let occupied = harness.draw();

    // The same frame with the bases empty: the difference is the markers.
    harness.snapshot.mlb_live.bases = Bases::default();
    let empty = harness.draw();
    let marked: Vec<(i32, i32)> = (0..WIDTH as i32)
        .flat_map(|x| (0..HEIGHT as i32).map(move |y| (x, y)))
        .filter(|(x, y)| occupied.pixel(*x, *y) != empty.pixel(*x, *y))
        .collect();
    assert!(!marked.is_empty(), "two occupied bases drew nothing");

    // The ball body is the batting side's color exactly; the other two palette
    // entries are its highlight and shade, so every marker pixel is reddish.
    let body = pack(AWAY_TEAM);
    assert!(
        marked.iter().any(|(x, y)| occupied.pixel(*x, *y) == body),
        "no marker pixel carries the batting side's own color"
    );
}

#[test]
fn between_halves_the_markers_and_labels_go_gray() {
    let mut harness = mlb();
    harness.snapshot.mlb_live.half = InningHalf::Middle;
    harness.snapshot.mlb_live.pitch_color = None;
    harness.snapshot.mlb_live.bat_color = None;
    harness.snapshot.mlb_live.has_at_bat = false;
    let frame = harness.draw();

    let table = geometry::MLB_LIVE;
    assert!(
        frame.count(
            table.pitcher_label.x..table.pitcher_label.x + table.pitcher_label.width,
            table.pitcher_label.y..table.pitcher_label.y + table.pitcher_label.height,
            DIM_GRAY,
        ) > 0,
        "the PIT label falls back to gray"
    );
    // The inning arrow is drawn for neither half.
    let arrow = scoreboard_render::generated::layout::inning_top::POSITION;
    assert_eq!(
        frame.lit_in(
            arrow.x..arrow.x + arrow.width,
            arrow.y..arrow.y + arrow.height
        ),
        0
    );
}

#[test]
fn a_critical_count_pulses_the_whole_row() {
    let mut harness = mlb();
    harness.snapshot.mlb_live.balls = 3;
    let table = geometry::MLB_LIVE;
    let row = |frame: &Frame| {
        (table.ball_dots.x..table.ball_dots.x + table.ball_dots.width)
            .flat_map(|x| {
                (table.ball_dots.y..table.ball_dots.y + table.ball_dots.height).map(move |y| (x, y))
            })
            .filter(|(x, y)| frame.lit(*x, *y))
            .count()
    };

    // At the triangle's trough the tint is a plain bright white-ish; at its peak
    // it has warmed. The row must change between the two.
    harness.view = FrameElapsed(0);
    let trough = harness.draw();
    harness.view = FrameElapsed(500);
    let peak = harness.draw();
    assert!(row(&trough) > 0 && row(&peak) > 0);
    assert_ne!(
        trough.pixel(table.ball_dots.x + 1, table.ball_dots.y + 1),
        peak.pixel(table.ball_dots.x + 1, table.ball_dots.y + 1),
        "the pulse should have moved between the trough and the peak"
    );

    // A non-critical row is untouched by it.
    let strikes = table.strike_dots;
    assert_eq!(
        trough.pixel(strikes.x + 1, strikes.y + 1),
        peak.pixel(strikes.x + 1, strikes.y + 1)
    );
}

#[test]
fn the_pulse_tint_matches_the_native_packer() {
    // `hub75.native.pack_hsv_to_rgb565` ships as an opaque .mpy; the goldens
    // come from the preview's stand-in for it, which is verified against the
    // real module on device. Hue 0 collapses the conversion to "red keeps the
    // value, the other channels scale by (255 - saturation)".
    for case in PULSE_CASES {
        let value = 191 + ((case.step * 64) >> 8);
        let saturation = (case.step * 80) >> 8;
        let dim = (value * (255 - saturation) + 127) / 255;
        let packed = scoreboard_render::rgb565(value as u8, dim as u8, dim as u8);
        assert_eq!(packed, case.packed, "pulse step {}", case.step);
    }
}

// -- Bottom strip ------------------------------------------------------------

#[test]
fn the_play_flash_outranks_sport_content_and_a_toast_outranks_both() {
    let mut harness = mlb();

    let quiet = harness.draw();
    assert!(
        sport_content(&quiet) > 0,
        "the pitcher/batter view is the resting state"
    );

    harness.snapshot.play.text = text("MARQUEZ STRIKES OUT LUKAKU SWINGING");
    harness.snapshot.play.updated_ms = 10_000;
    harness.snapshot.commit_seq += 1;
    let flashing = harness.draw();
    assert_eq!(sport_content(&flashing), 0, "the flash takes the strip");
    assert!(flashing.lit_in(51..127, 43..59) > 0);

    harness.snapshot.toast = ToastView {
        text: text("LOCKED"),
        kind: ToastKind::Text,
        updated_ms: 10_000,
        sticky: false,
        pulse_ms: 0,
    };
    harness.snapshot.commit_seq += 1;
    let toasted = harness.draw();
    assert_ne!(
        toasted.0, flashing.0,
        "a toast should displace the play flash"
    );
}

#[test]
fn the_play_flash_expires_on_the_wall_rail() {
    let mut harness = mlb();
    harness.snapshot.play.text = text("SINGLE TO LEFT");
    harness.snapshot.play.updated_ms = 10_000;
    harness.snapshot.commit_seq += 1;
    harness.prepared.sync(&harness.snapshot, &harness.settings);

    // Fourteen unscii_16 glyphs is 112 px in a 76 px window, so 36 px of travel
    // at the configured speed, between the two one-second dwells.
    let speed = harness.settings.scroll_px_per_second as u64;
    let window = harness.prepared.play_window_ms();
    assert_eq!(window, 1_000 + 36 * 1_000 / speed + 1_000);

    harness.now = WallMs(10_000 + window - 1);
    assert_eq!(sport_content(&harness.draw()), 0, "still flashing");
    harness.now = WallMs(10_000 + window);
    assert!(
        sport_content(&harness.draw()) > 0,
        "back to the pitcher and batter"
    );
}

/// Whether the MLB screen's own bottom-strip content drew.
///
/// The pitcher and batter *name* slots overlap the shared flash strip, so they
/// cannot tell the two apart. The PIT label's top two rows sit just above the
/// strip's first row, and only the resting view draws there.
fn sport_content(frame: &Frame) -> usize {
    let label = geometry::MLB_LIVE.pitcher_label;
    frame.lit_in(
        label.x..label.x + label.width,
        label.y..geometry::PLAY_TEXT.y,
    )
}

// -- Pregame -----------------------------------------------------------------

fn pregame() -> Harness {
    let mut harness = Harness::new(Mode::Pregame);
    let view = &mut harness.snapshot.pregame;
    view.sport = Sport::Mlb;
    view.away.record = Some(Record {
        wins: 51,
        losses: 44,
    });
    view.home.record = Some(Record {
        wins: 60,
        losses: 35,
    });
    view.away.color = AWAY_TEAM;
    view.home.color = HOME_TEAM;
    view.away.line = text("G. MARQUEZ");
    view.home.line = text("Y. DARVISH");
    view.info_primary = text("CITIZENS BANK PARK");
    view.info_secondary = text("72F PARTLY CLOUDY");
    view.time_text = text("7:05 PM");
    harness
}

#[test]
fn the_pregame_screen_stacks_records_beside_the_crests() {
    let frame = pregame().draw();
    let table = geometry::PREGAME;
    for slot in [
        table.record_away_wins,
        table.record_away_losses,
        table.record_home_wins,
        table.record_home_losses,
    ] {
        assert!(
            frame.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height) > 0,
            "{slot:?} drew nothing"
        );
    }
}

#[test]
fn a_side_without_a_record_leaves_its_slots_blank() {
    let mut harness = pregame();
    harness.snapshot.pregame.away.record = None;
    let frame = harness.draw();
    let slot = geometry::PREGAME.record_away_wins;
    assert_eq!(
        frame.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height),
        0,
        "no record means no digits, not a fake 0-0"
    );
}

#[test]
fn the_pregame_cycle_alternates_its_two_lines() {
    let mut harness = pregame();
    harness.prepared.sync(&harness.snapshot, &harness.settings);
    let cycle = *harness.prepared.pregame();

    // Neither line overflows the 80 px slot badly enough to outrun the floor,
    // so both dwell for the minimum.
    let (first, elapsed) = cycle.phase_at(0).expect("a cycle");
    assert_eq!(first, PregameLine::Primary);
    assert_eq!(elapsed, 0);
    let (second, elapsed) = cycle
        .phase_at(geometry::PREGAME_INFO_DWELL_MS + 10)
        .unwrap();
    assert_eq!(second, PregameLine::Secondary);
    assert_eq!(elapsed, 10, "the phase's own clock restarts from its start");

    // And it wraps.
    let (wrapped, _) = cycle.phase_at(geometry::PREGAME_INFO_DWELL_MS * 2).unwrap();
    assert_eq!(wrapped, PregameLine::Primary);
}

#[test]
fn a_single_info_line_never_yields_to_an_empty_one() {
    // NBA fills only the first line. The skipped line must not become a blank
    // phase — and when only the *second* is filled, the phase must map to it.
    let mut harness = pregame();
    harness.snapshot.pregame.info_secondary = Text::new();
    harness.prepared.sync(&harness.snapshot, &harness.settings);
    let cycle = *harness.prepared.pregame();
    for elapsed in [0, 3_000, 9_999, 100_000] {
        assert_eq!(cycle.phase_at(elapsed).unwrap().0, PregameLine::Primary);
    }

    let mut harness = pregame();
    harness.snapshot.pregame.info_primary = Text::new();
    harness.snapshot.commit_seq += 1;
    harness.prepared.sync(&harness.snapshot, &harness.settings);
    assert_eq!(
        harness.prepared.pregame().phase_at(0).unwrap().0,
        PregameLine::Secondary,
        "the surviving line is what shows"
    );
}

#[test]
fn the_date_leads_the_time_when_the_game_is_not_today() {
    let mut harness = pregame();
    harness.snapshot.pregame.date_text = text("WED JUL 16");
    let slot = geometry::PREGAME.info_time;

    harness.view = FrameElapsed(0);
    let leading = harness.draw();
    harness.view = FrameElapsed(geometry::PREGAME_INFO_DWELL_MS);
    let following = harness.draw();
    let band =
        |frame: &Frame| frame.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height);
    assert!(band(&leading) > 0 && band(&following) > 0);
    assert_ne!(
        band(&leading),
        band(&following),
        "ten glyphs of date and seven of time cannot light the same pixels"
    );

    // With no date there is nothing to alternate with.
    harness.snapshot.pregame.date_text = Text::new();
    harness.snapshot.commit_seq += 1;
    harness.view = FrameElapsed(0);
    let only_time = harness.draw();
    harness.view = FrameElapsed(geometry::PREGAME_INFO_DWELL_MS);
    let still_time = harness.draw();
    assert_eq!(
        only_time.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height),
        still_time.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height)
    );
}

// -- Finals ------------------------------------------------------------------

fn linescore_final(sport: Sport) -> Harness {
    let mut harness = Harness::new(Mode::Final);
    let view = &mut harness.snapshot.linescore_final;
    view.sport = sport;
    view.away_score = 4;
    view.home_score = 7;
    view.final_text = text("FINAL");
    view.header_row = text("  1  2  3  4  5  6  7  8  9");
    view.away_row = text("  0  1  0  2  0  0  1  0  0");
    view.home_row = text("  3  0  0  0  4  0  0  0  X");
    view.home_won = true;
    view.away_color = AWAY_TEAM;
    view.home_color = HOME_TEAM;
    harness
}

#[test]
fn the_final_screen_colors_the_winner_and_grays_the_loser() {
    let frame = linescore_final(Sport::Mlb).draw();
    let table = RenderSettings::new().final_table(Sport::Mlb);
    let count_in = |slot: scoreboard_render::Slice, color| {
        frame.count(
            slot.x..slot.x + slot.width,
            slot.y..slot.y + slot.height,
            color,
        )
    };
    assert!(
        count_in(table.total_home, pack(HOME_TEAM)) > 0,
        "the winner"
    );
    assert!(count_in(table.total_away, DIM_GRAY) > 0, "the loser");
    assert_eq!(count_in(table.total_away, pack(AWAY_TEAM)), 0);
}

#[test]
fn the_totals_header_names_the_sports_unit() {
    // "R" for runs, "T" for points. Same glyph count, different glyph.
    let mlb = linescore_final(Sport::Mlb).draw();
    let nba = linescore_final(Sport::Nba).draw();
    let slot = RenderSettings::new().final_table(Sport::Mlb).total_header;
    assert_ne!(
        mlb.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height),
        nba.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height),
    );
}

#[test]
fn the_line_score_rows_scroll_in_lockstep() {
    let mut harness = linescore_final(Sport::Mlb);
    // A row wide enough to overflow its 75 px window, so it actually scrolls.
    let long = "  1  2  3  4  5  6  7  8  9 10 11 12";
    harness.snapshot.linescore_final.header_row = text(long);
    harness.snapshot.linescore_final.away_row = text(long);
    harness.snapshot.linescore_final.home_row = text(long);

    let table = RenderSettings::new().final_table(Sport::Mlb);
    let column_of = |frame: &Frame, slot: scoreboard_render::Slice| {
        (slot.x..slot.x + slot.width)
            .find(|x| (slot.y..slot.y + slot.height).any(|y| frame.lit(*x, y)))
    };

    // Past the opening dwell the rows have moved, and identical text at one
    // clock must put all three at the same offset.
    harness.view = FrameElapsed(geometry::FINAL_LINESCORE_SCROLL.pause_ms + 1_000);
    let frame = harness.draw();
    let header = column_of(&frame, table.linescore_header);
    let away = column_of(&frame, table.linescore_away);
    let home = column_of(&frame, table.linescore_home);
    assert!(header.is_some());
    assert_eq!(
        (header.map(|x| x - table.linescore_header.x)),
        (away.map(|x| x - table.linescore_away.x))
    );
    assert_eq!(
        (away.map(|x| x - table.linescore_away.x)),
        (home.map(|x| x - table.linescore_home.x))
    );
}

#[test]
fn a_soccer_draw_colors_both_sides() {
    let mut harness = Harness::new(Mode::SoccerFinal);
    let view = &mut harness.snapshot.soccer_final;
    view.away_score = 2;
    view.home_score = 2;
    view.draw = true;
    view.away_color = AWAY_TEAM;
    view.home_color = HOME_TEAM;
    view.ft_text = text("FULL TIME");
    let frame = harness.draw();

    let table = geometry::SOCCER_FINAL;
    let count_in = |slot: scoreboard_render::Slice, color| {
        frame.count(
            slot.x..slot.x + slot.width,
            slot.y..slot.y + slot.height,
            color,
        )
    };
    assert!(count_in(table.score_away, pack(AWAY_TEAM)) > 0);
    assert!(count_in(table.score_home, pack(HOME_TEAM)) > 0);
    assert_eq!(
        count_in(table.score_away, DIM_GRAY),
        0,
        "a draw has no loser"
    );
}

// -- Soccer live -------------------------------------------------------------

fn soccer_live() -> Harness {
    let mut harness = Harness::new(Mode::SoccerLive);
    let view = &mut harness.snapshot.soccer_live;
    view.away_score = 1;
    view.home_score = 0;
    view.clock_anchor_s = 23 * 60;
    view.clock_anchor_ms = 10_000;
    view.clock_running = true;
    view.base_min = 45;
    view.phase_text = text("1ST");
    view.phase_long = text("1ST HALF");
    harness
}

/// Which pixels the clock slot lit, as a compact signature.
fn clock_signature(frame: &Frame) -> Vec<(i32, i32)> {
    let slot = geometry::SOCCER_LIVE_A.clock;
    (slot.x..slot.x + slot.width)
        .flat_map(|x| (slot.y..slot.y + slot.height).map(move |y| (x, y)))
        .filter(|(x, y)| frame.lit(*x, *y))
        .collect()
}

#[test]
fn the_match_clock_ticks_between_polls_without_a_commit() {
    let mut harness = soccer_live();
    harness.now = WallMs(10_000);
    let at_anchor = clock_signature(&harness.draw());

    // 37 seconds later, still inside the same minute: the panel must not move.
    harness.now = WallMs(10_000 + 37_000);
    assert_eq!(clock_signature(&harness.draw()), at_anchor, "23:37 is 23'");

    // Past the minute boundary it advances, with no new commit involved.
    harness.now = WallMs(10_000 + 60_000);
    assert_ne!(clock_signature(&harness.draw()), at_anchor, "24' now");
}

#[test]
fn a_stopped_clock_holds_its_anchor() {
    let mut harness = soccer_live();
    harness.snapshot.soccer_live.clock_running = false;
    harness.now = WallMs(10_000);
    let anchored = clock_signature(&harness.draw());
    harness.now = WallMs(10_000 + 600_000);
    assert_eq!(clock_signature(&harness.draw()), anchored);
}

#[test]
fn stoppage_time_holds_the_base_minute_and_counts_up() {
    let mut harness = soccer_live();
    // 47:30 in the first half: "45+2'", in the warning color.
    harness.snapshot.soccer_live.clock_anchor_s = 47 * 60 + 30;
    harness.snapshot.soccer_live.clock_running = false;
    let frame = harness.draw();
    let slot = geometry::SOCCER_LIVE_A.clock;
    let warning = pack(harness.snapshot.ui_colors.clock_warning);
    assert!(
        frame.count(
            slot.x..slot.x + slot.width,
            slot.y..slot.y + slot.height,
            warning
        ) > 0,
        "added minutes draw in the warning color"
    );

    // At exactly the base minute ESPN still shows "45'", in the normal color.
    harness.snapshot.soccer_live.clock_anchor_s = 45 * 60 + 59;
    harness.snapshot.commit_seq += 1;
    let at_base = harness.draw();
    let normal = pack(harness.snapshot.ui_colors.clock_normal);
    assert!(
        at_base.count(
            slot.x..slot.x + slot.width,
            slot.y..slot.y + slot.height,
            normal
        ) > 0
    );
}

#[test]
fn a_break_announces_itself_in_the_clock_slot() {
    let mut harness = soccer_live();
    harness.snapshot.soccer_live.on_break = true;
    let halftime = harness.draw();

    harness.snapshot.soccer_live.base_min = 105;
    harness.snapshot.commit_seq += 1;
    let later_break = harness.draw();
    assert_ne!(
        clock_signature(&halftime),
        clock_signature(&later_break),
        "HT and BREAK are different words"
    );
}

#[test]
fn an_empty_ticker_shows_a_placeholder_rather_than_a_hole() {
    let mut harness = soccer_live();
    let table = geometry::SOCCER_LIVE_A;
    let empty_slot = table.event_empty;
    let frame = harness.draw();
    assert!(
        frame.lit_in(
            empty_slot.x..empty_slot.x + empty_slot.width,
            empty_slot.y..empty_slot.y + empty_slot.height,
        ) > 0
    );

    harness.snapshot.soccer_live.has_event = true;
    harness.snapshot.soccer_live.event_top = text("GOAL 23'");
    harness.snapshot.soccer_live.event_name = text("HAALAND");
    harness.snapshot.soccer_live.event_color = AWAY_TEAM;
    harness.snapshot.commit_seq += 1;
    let with_event = harness.draw();
    assert!(
        with_event.count(
            table.event_top.x..table.event_top.x + table.event_top.width,
            table.event_top.y..table.event_top.y + table.event_top.height,
            pack(AWAY_TEAM),
        ) > 0,
        "the event draws in the scoring side's color"
    );
}

#[test]
fn the_soccer_variants_place_the_period_differently() {
    let mut harness = soccer_live();
    let chip = geometry::SOCCER_LIVE_A.phase.unwrap();
    let a = harness.draw();
    assert!(chip.width > 0 && a.lit_in(chip.x..chip.x + chip.width, chip.y..chip.y + 8) > 0);

    harness.settings.apply_variant("soccer_live", "B");
    let b = harness.draw();
    let long = geometry::SOCCER_LIVE_B.phase_long.unwrap();
    assert_eq!(
        b.lit_in(chip.x..chip.x + chip.width, chip.y..chip.y + 8),
        0,
        "B has no chip in the identity column"
    );
    assert!(b.lit_in(long.x..long.x + long.width, long.y..long.y + 8) > 0);
}

// -- NBA ---------------------------------------------------------------------

#[test]
fn the_nba_clock_changes_color_with_its_state() {
    let mut harness = Harness::new(Mode::NbaLive);
    harness.snapshot.nba_live.away_score = 101;
    harness.snapshot.nba_live.home_score = 99;
    harness.snapshot.nba_live.phase_text = text("Q4");
    harness.snapshot.nba_live.clock_text = text("4:37");
    let slot = geometry::NBA_LIVE.clock;
    let count = |frame: &Frame, color| {
        frame.count(
            slot.x..slot.x + slot.width,
            slot.y..slot.y + slot.height,
            color,
        )
    };

    let normal = harness.draw();
    assert!(count(&normal, pack(harness.snapshot.ui_colors.clock_normal)) > 0);

    harness.snapshot.nba_live.clock_low = true;
    let low = harness.draw();
    assert!(count(&low, pack(harness.snapshot.ui_colors.clock_warning)) > 0);

    // Accent wins over low: a break is a break even at 0:00.
    harness.snapshot.nba_live.clock_accent = true;
    let accent = harness.draw();
    assert!(count(&accent, pack(harness.snapshot.ui_colors.accent)) > 0);
}

// -- Football ----------------------------------------------------------------

fn football() -> Harness {
    let mut harness = Harness::new(Mode::FootballLive);
    let view = &mut harness.snapshot.football_live;
    view.away_score = 17;
    view.home_score = 14;
    view.phase_text = text("Q3");
    view.clock_text = text("10:42");
    view.situation_text = text("3RD & 7");
    view.situation = Some(FieldSituation {
        down: 3,
        distance: 7,
        yard_line: 35,
        possession: Side::Away,
        red_zone: false,
    });
    view.away_timeouts = Some(2);
    view.home_timeouts = Some(3);
    view.away_color = AWAY_TEAM;
    view.home_color = HOME_TEAM;
    harness
}

#[test]
fn the_field_projection_maps_yardlines_to_columns() {
    // Away possession counts up from the left goal line; home counts down from
    // the right one.
    let away = Field::project(FieldSituation {
        down: 1,
        distance: 10,
        yard_line: 25,
        possession: Side::Away,
        red_zone: false,
    });
    assert_eq!(away.scrimmage_x, geometry::FOOTBALL_FIELD_YARD0_X + 25);
    assert_eq!(away.first_down_x, geometry::FOOTBALL_FIELD_YARD0_X + 35);
    assert!(away.attacking_right);

    let home = Field::project(FieldSituation {
        down: 1,
        distance: 10,
        yard_line: 25,
        possession: Side::Home,
        red_zone: false,
    });
    assert_eq!(home.scrimmage_x, geometry::FOOTBALL_FIELD_YARD0_X + 75);
    assert_eq!(home.first_down_x, geometry::FOOTBALL_FIELD_YARD0_X + 65);
    assert!(!home.attacking_right);
}

#[test]
fn goal_to_go_clamps_the_first_down_line_to_the_goal_line() {
    let goal_to_go = Field::project(FieldSituation {
        down: 1,
        distance: 10,
        yard_line: 95,
        possession: Side::Away,
        red_zone: true,
    });
    assert_eq!(
        goal_to_go.first_down_x,
        (geometry::FOOTBALL_FIELD_YARD0_X + 100).min(geometry::FOOTBALL_FIELD_LOS_MAX_X),
        "the line stops at the goal line, never past it"
    );

    let home_goal_to_go = Field::project(FieldSituation {
        down: 1,
        distance: 10,
        yard_line: 95,
        possession: Side::Home,
        red_zone: true,
    });
    assert_eq!(
        home_goal_to_go.first_down_x,
        geometry::FOOTBALL_FIELD_YARD0_X
    );
}

#[test]
fn every_projected_line_leans_toward_the_vanishing_point() {
    for yard_line in 0..=100u8 {
        for possession in [Side::Away, Side::Home] {
            let field = Field::project(FieldSituation {
                down: 1,
                distance: 10,
                yard_line,
                possession,
                red_zone: false,
            });
            for (bottom, top) in [
                (field.scrimmage_x, field.scrimmage_top_x),
                (field.first_down_x, field.first_down_top_x),
            ] {
                let vanishing = geometry::FOOTBALL_VP_X;
                assert!(
                    (bottom.min(vanishing)..=bottom.max(vanishing)).contains(&top),
                    "yard {yard_line} leaned from {bottom} to {top}"
                );
                assert!(bottom <= geometry::FOOTBALL_FIELD_LOS_MAX_X);
            }
        }
    }
}

#[test]
fn the_football_screen_tints_its_endzones_and_bars() {
    let frame = football().draw();
    let table = geometry::FOOTBALL_LIVE;
    // Three bars per team, emptying left to right: two held for the away side.
    assert_eq!(
        frame.pixel(table.timeout_away_x, table.timeout_y),
        pack(AWAY_TEAM)
    );
    assert_eq!(
        frame.pixel(table.timeout_away_x + 14, table.timeout_y),
        DIM_GRAY,
        "the spent timeout"
    );
    assert_eq!(
        frame.pixel(table.timeout_home_x + 14, table.timeout_y),
        pack(HOME_TEAM),
        "all three held"
    );

    // The endzone blocks sit outside the 100-yard span, at the strip's ends.
    let field = scoreboard_render::generated::layout::football_field::POSITION;
    assert!(
        frame.count(
            field.x..field.x + 11,
            field.y..field.y + field.height,
            pack(AWAY_TEAM)
        ) > 0,
        "the away endzone takes its team color"
    );
}

#[test]
fn undeclared_timeouts_draw_no_bars_at_all() {
    let mut harness = football();
    harness.snapshot.football_live.away_timeouts = None;
    let frame = harness.draw();
    let table = geometry::FOOTBALL_LIVE;
    assert_eq!(
        frame.lit_in(
            table.timeout_away_x..table.timeout_away_x + 20,
            table.timeout_y..table.timeout_y + 1
        ),
        0,
        "no bars beats three fake ones"
    );
}

#[test]
fn the_red_zone_warns_on_the_situation_and_its_arrow() {
    let mut harness = football();
    let table = geometry::FOOTBALL_LIVE;
    let warning = pack(harness.snapshot.ui_colors.clock_warning);
    let band = |frame: &Frame, color| {
        frame.count(
            table.situation.x - 6..table.situation.x + table.situation.width + 6,
            table.situation.y..table.situation.y + table.situation.height,
            color,
        )
    };

    assert_eq!(band(&harness.draw(), warning), 0, "outside the red zone");

    if let Some(situation) = harness.snapshot.football_live.situation.as_mut() {
        situation.red_zone = true;
    }
    harness.snapshot.commit_seq += 1;
    assert!(band(&harness.draw(), warning) > 0);
}

#[test]
fn the_play_flash_replaces_the_field_strip() {
    let mut harness = football();
    let field = scoreboard_render::generated::layout::football_field::POSITION;
    // The part of the field that lies left of the shared flash strip: only the
    // field itself can light it, so it says whether the field was drawn at all.
    let strip = |frame: &Frame| {
        frame.lit_in(
            field.x..geometry::PLAY_TEXT.x,
            field.y..field.y + field.height,
        )
    };
    assert!(strip(&harness.draw()) > 0);

    harness.snapshot.play.text = text("MAHOMES PASS COMPLETE TO KELCE FOR 12 YARDS");
    harness.snapshot.play.updated_ms = 10_000;
    harness.snapshot.commit_seq += 1;
    let flashing = harness.draw();
    assert_eq!(
        strip(&flashing),
        0,
        "the field is not drawn under the flash"
    );
}

// -- Dispatch ----------------------------------------------------------------

#[test]
fn the_menu_preempts_every_mode() {
    let mut harness = mlb();
    harness.snapshot.menu.active = true;
    let with_menu = harness.draw();

    let mut plain = Harness::new(Mode::Idle);
    plain.snapshot.menu = harness.snapshot.menu.clone();
    plain.snapshot.ui_colors = harness.snapshot.ui_colors;
    assert_eq!(
        with_menu.0,
        plain.draw().0,
        "the mode underneath makes no difference at all"
    );
}

#[test]
fn every_mode_has_a_renderer() {
    // A mode that fell through to a default would draw the wrong screen
    // silently; the dispatch is exhaustive, and this is the check that each arm
    // actually draws something distinguishable.
    for mode in [
        Mode::Startup,
        Mode::Idle,
        Mode::NoGames,
        Mode::Error,
        Mode::Updating,
        Mode::MlbLive,
        Mode::Pregame,
        Mode::Final,
        Mode::SoccerLive,
        Mode::SoccerFinal,
        Mode::NbaLive,
        Mode::FootballLive,
    ] {
        let mut harness = Harness::new(mode);
        harness.snapshot.error.lines.push(text("detail")).ok();
        harness.snapshot.updating.phase = text("Downloading");
        harness.snapshot.startup.operation = text("WIFI");
        harness.snapshot.linescore_final.final_text = text("FINAL");
        harness.snapshot.soccer_final.ft_text = text("FULL TIME");
        harness.snapshot.nba_live.clock_text = text("4:37");
        harness.snapshot.football_live.clock_text = text("10:42");
        harness.snapshot.pregame.time_text = text("7:05 PM");
        let frame = harness.draw();
        assert!(
            frame.lit_in(0..WIDTH as i32, 0..HEIGHT as i32) > 0,
            "{mode:?} drew an empty frame"
        );
    }
    // Setup is left out above: it needs a QR, and tests/screens.rs covers it.
}

// -- The skip memo -----------------------------------------------------------

#[test]
fn a_static_screen_redraws_only_when_something_changed() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.mode = Mode::Idle;
    snapshot.commit_seq = 7;
    let mut memo = SkipMemo::new();

    assert!(memo.should_render(&snapshot, WallMs(0)), "the first frame");
    assert!(
        !memo.should_render(&snapshot, WallMs(50)),
        "nothing changed"
    );
    snapshot.commit_seq += 1;
    assert!(memo.should_render(&snapshot, WallMs(100)), "a new commit");
    assert!(!memo.should_render(&snapshot, WallMs(150)));
}

#[test]
fn an_animated_screen_never_skips() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.mode = Mode::MlbLive;
    let mut memo = SkipMemo::new();
    for tick in 0..10u64 {
        assert!(memo.should_render(&snapshot, WallMs(tick * 50)));
    }
}

#[test]
fn a_toast_keeps_a_static_screen_alive_through_its_fade() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.mode = Mode::NoGames;
    snapshot.toast = ToastView {
        text: Text::new(),
        kind: ToastKind::Lock,
        updated_ms: 1_000,
        sticky: false,
        pulse_ms: 0,
    };
    let mut memo = SkipMemo::new();

    // Up: every frame redraws, because the overlay animates.
    for at in [1_000, 1_500, 2_400] {
        assert!(memo.should_render(&snapshot, WallMs(at)), "at {at}");
    }
    // Expired, but the dim is still easing back out.
    for at in [2_500, 2_550, 2_600] {
        assert!(memo.should_render(&snapshot, WallMs(at)), "fading at {at}");
    }
    // The frame after the tail ends still draws — it is the one that removes
    // the last of the dim.
    assert!(memo.should_render(&snapshot, WallMs(2_700)));
    assert!(!memo.should_render(&snapshot, WallMs(2_750)), "clean again");
}

#[test]
fn the_menu_keeps_redrawing_whatever_is_underneath() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.mode = Mode::Idle;
    snapshot.menu.active = true;
    let mut memo = SkipMemo::new();
    for tick in 0..5u64 {
        assert!(
            memo.should_render(&snapshot, WallMs(tick * 50)),
            "the marquee animates regardless of the mode"
        );
    }
}

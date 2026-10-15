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
use scoreboard_render::geometry::{self, FinalVariant, RenderSettings};
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
    for pixel in slot.as_chunks_mut::<2>().0 {
        *pixel = CREST.to_le_bytes();
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
    let (away, home) = (table.logo_away, table.logo_home);
    assert_eq!(frame.pixel(away.x, away.y), CREST, "the away crest");
    assert_eq!(frame.pixel(home.x, home.y), CREST, "the home crest");
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
fn the_play_flash_outranks_sport_content() {
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
        label.y..geometry::MLB_LIVE.strip.y,
    )
}

#[test]
fn the_play_window_is_sized_to_the_strip_the_flash_scrolls_in() {
    // 112 px of text: it overflows the column screens' 76 px strip but fits the
    // corner screens' full-width one, so only the first scrolls at all.
    let play = "SINGLE TO LEFT";
    let speed = RenderSettings::new().scroll_px_per_second as u64;
    for (mode, window) in [
        (Mode::MlbLive, 1_000 + 36 * 1_000 / speed + 1_000),
        (Mode::NbaLive, 1_000 + 36 * 1_000 / speed + 1_000),
        (Mode::FootballLive, 2_000),
        // Hockey's default is the rink, whose strip is the full width.
        (Mode::HockeyLive, 2_000),
    ] {
        let mut harness = Harness::new(mode);
        harness.snapshot.play.text = text(play);
        harness.snapshot.play.updated_ms = 10_000;
        harness.prepared.sync(&harness.snapshot, &harness.settings);
        assert_eq!(harness.prepared.play_window_ms(), window, "{mode:?}");
    }

    // Soccer's strip is the variant's, and a variant switch re-sizes the window
    // once the prepared view is invalidated, as core 1 does on a settings update.
    let mut harness = Harness::new(Mode::SoccerLive);
    harness.snapshot.play.text = text(play);
    harness.snapshot.play.updated_ms = 10_000;
    harness.prepared.sync(&harness.snapshot, &harness.settings);
    assert_eq!(
        harness.prepared.play_window_ms(),
        1_000 + 36 * 1_000 / speed + 1_000
    );
    harness.settings.apply_variant("soccer_live", "C");
    harness.prepared.invalidate();
    harness.prepared.sync(&harness.snapshot, &harness.settings);
    assert_eq!(harness.prepared.play_window_ms(), 2_000);

    // Hockey's period ledger keeps a column, so its flash scrolls.
    let mut harness = Harness::new(Mode::HockeyLive);
    harness.snapshot.play.text = text(play);
    harness.snapshot.play.updated_ms = 10_000;
    harness.settings.apply_variant("hockey_live", "A");
    harness.prepared.sync(&harness.snapshot, &harness.settings);
    assert_eq!(
        harness.prepared.play_window_ms(),
        1_000 + 36 * 1_000 / speed + 1_000
    );
}

// -- Pregame -----------------------------------------------------------------

fn pregame() -> Harness {
    let mut harness = Harness::new(Mode::Pregame);
    let view = &mut harness.snapshot.pregame;
    view.sport = Sport::Mlb;
    view.away.record = Some(Record {
        wins: 51,
        losses: 44,
        ot_losses: None,
    });
    view.home.record = Some(Record {
        wins: 60,
        losses: 35,
        ot_losses: None,
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

// -- The edge ring -----------------------------------------------------------

/// Lit pixels on the panel's unreliable outer ring — row 0, row 63, column 0
/// and column 127 — which nothing may draw on (BACKLOG 56).
fn edge_ring(frame: &Frame) -> usize {
    let (width, height) = (WIDTH as i32, HEIGHT as i32);
    frame.lit_in(0..width, 0..1)
        + frame.lit_in(0..width, height - 1..height)
        + frame.lit_in(0..1, 1..height - 1)
        + frame.lit_in(width - 1..width, 1..height - 1)
}

/// Frames across a scroll: the opening dwell, mid-scroll, and the far end.
const SCROLL_SAMPLES: [u64; 5] = [0, 2_000, 4_000, 7_000, 12_000];

#[test]
fn every_final_variant_keeps_a_basketball_total_whole_and_off_the_edge_ring() {
    // Three-digit totals are every NBA final. Variant C carried them in a
    // 20 px slot that ran the third digit off the panel; A and B inked
    // column 127. The crest pool's crests are solid, so a crest on the ring
    // shows up here too.
    for variant in [FinalVariant::A, FinalVariant::B, FinalVariant::C] {
        let mut harness = linescore_final(Sport::Nba);
        harness.settings.nba_final = variant;
        let view = &mut harness.snapshot.linescore_final;
        view.away_score = 130;
        view.home_score = 128;
        view.header_row = text(" 1  2  3  4  5  6  7 ");
        view.away_row = text("28 31 25 24 10 12  0 ");
        view.home_row = text("30 27 22 29 10 10  0 ");
        let table = variant.table();

        for at in SCROLL_SAMPLES {
            harness.view = FrameElapsed(at);
            let frame = harness.draw();
            assert_eq!(edge_ring(&frame), 0, "{variant:?} at {at} ms");
            // A dark column between the rule and the widest total.
            let gutter = table.divider_x + 1;
            for slot in [table.total_away, table.total_home] {
                assert_eq!(
                    frame.lit_in(gutter..gutter + 1, slot.y..slot.y + slot.height),
                    0,
                    "{variant:?}: a three-digit total touches the rule"
                );
            }
        }
    }
}

#[test]
fn a_four_quarter_line_score_holds_still_in_every_variant() {
    // The rows end in a trailing space (the model's three-char columns). B's
    // 54 px window once scrolled a 60 px row 6 px for nothing, sliding that
    // blank into view and cutting the first quarter's tens digit off.
    for variant in [FinalVariant::A, FinalVariant::B, FinalVariant::C] {
        let mut harness = linescore_final(Sport::Nba);
        harness.settings.nba_final = variant;
        let view = &mut harness.snapshot.linescore_final;
        view.away_score = 118;
        view.home_score = 100;
        view.header_row = text(" 1  2  3  4 ");
        view.away_row = text("36 32 25 25 ");
        view.home_row = text("30 35 25 10 ");
        let slot = variant.table().linescore_away;

        let rest = harness.draw();
        // The first column's tens digit is inked at the window's left edge.
        assert!(
            rest.lit_in(slot.x..slot.x + 4, slot.y..slot.y + slot.height) > 0,
            "{variant:?}: the first quarter's tens digit is missing"
        );
        for at in SCROLL_SAMPLES {
            harness.view = FrameElapsed(at);
            assert!(harness.draw().0 == rest.0, "{variant:?} moved at {at} ms");
        }
    }
}

#[test]
fn a_row_too_long_even_without_its_trailing_space_still_scrolls() {
    // Nine innings never fit, so the fit rule must not freeze them.
    let mut harness = linescore_final(Sport::Mlb);
    harness.settings.mlb_final = FinalVariant::B;
    let view = &mut harness.snapshot.linescore_final;
    view.header_row = text(" 1  2  3  4  5  6  7  8  9 ");
    view.away_row = text(" 0  1  0  2  0  0  1  0  0 ");
    view.home_row = text(" 3  0  0  0  4  0  0  0  X ");
    let rest = harness.draw();
    harness.view = FrameElapsed(7_000);
    assert!(harness.draw().0 != rest.0, "a nine-inning row stopped scrolling");
}

#[test]
fn full_time_scorer_lists_scroll_without_touching_the_edge_ring() {
    let mut harness = Harness::new(Mode::SoccerFinal);
    let view = &mut harness.snapshot.soccer_final;
    view.away_score = 3;
    view.home_score = 1;
    view.ft_text = text("AET");
    view.scorers_away = text("J. Quintero 12', J. Campaz 58', L. Diaz 120'+1'");
    view.scorers_home = text("G. Xhaka 33'");
    for at in SCROLL_SAMPLES {
        harness.view = FrameElapsed(at);
        assert_eq!(edge_ring(&harness.draw()), 0, "at {at} ms");
    }
}

#[test]
fn pregame_crests_and_text_stay_off_the_edge_ring() {
    // Dividers off: the pregame's rules come from `column_dividers`, which it
    // shares with the live screens. This pins the pregame's own table.
    let mut harness = pregame();
    harness.settings.show_dividers = false;
    let view = &mut harness.snapshot.pregame;
    view.info_primary = text("Oriole Park at Camden Yards");
    view.info_secondary = text("58F MOSTLY CLOUDY");
    view.away.line = text("#25 MIDDLE TENNESSEE");
    view.date_text = text("WED SEP 30");
    for at in SCROLL_SAMPLES {
        harness.view = FrameElapsed(at);
        assert_eq!(edge_ring(&harness.draw()), 0, "at {at} ms");
    }
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
    let flash = geometry::FOOTBALL_LIVE.strip;
    // The field's rows below the flash strip: no glyph reaches them, so only
    // the field itself can light them, and they say whether it drew at all.
    let below = |frame: &Frame| {
        frame.lit_in(
            field.x..field.x + field.width,
            flash.y + flash.height..field.y + field.height,
        )
    };
    assert!(below(&harness.draw()) > 0);

    harness.snapshot.play.text = text("MAHOMES PASS COMPLETE TO KELCE FOR 12 YARDS");
    harness.snapshot.play.updated_ms = 10_000;
    harness.snapshot.commit_seq += 1;
    let flashing = harness.draw();
    assert_eq!(
        below(&flashing),
        0,
        "the field is not drawn under the flash"
    );
}

/// The corner screens keep nothing beside the flash, so it starts at the first
/// legal column rather than at the column screens' data margin.
#[test]
fn the_corner_screens_flash_across_the_full_width() {
    let column = geometry::COLUMN_STRIP;
    let mut football = football();
    let mut soccer = soccer_live();
    soccer.settings.apply_variant("soccer_live", "C");
    for (name, harness, strip) in [
        ("football", &mut football, geometry::FOOTBALL_LIVE.strip),
        ("soccer C", &mut soccer, geometry::SOCCER_LIVE_C.strip),
    ] {
        assert_eq!(strip.x, 1, "{name}");
        assert_eq!(strip.x + strip.width, geometry::WIDTH - 1, "{name}");
        harness.snapshot.play.text = text("MAHOMES PASS COMPLETE TO KELCE FOR 12 YARDS");
        harness.snapshot.play.updated_ms = 10_000;
        harness.snapshot.commit_seq += 1;
        let frame = harness.draw();
        assert!(
            frame.lit_in(1..column.x, strip.y..strip.y + strip.height) > 0,
            "{name}: the flash should start left of the column screens' margin"
        );
    }
}

// -- Hockey ------------------------------------------------------------------
//
// Hand-built wire games committed through the real model, so these pin the
// path the backend's hockey payloads will take, from the wire's types to the
// panel. Real-fixture goldens follow with the backend lane.

use scoreboard_model::snapshot::GoalMark;
use scoreboard_model::{GameDetail, LeagueId, LocalClock, Logos as CommitLogos, Store};
use scoreboard_render::game::hockey::{mark_at, mix};
use scoreboard_render::generated::layout;
use scoreboard_render::geometry::{HOCKEY_FINAL, HOCKEY_LIVE_A, HOCKEY_LIVE_B, HockeyLiveVariant};
use scoreboard_render::{Slice, WHITE, rgb565};
use scoreboard_wire::hockey as wire;
use scoreboard_wire::{LivePhase, TeamColors, TeamState};

/// When the last commit lands, and the harness's default wall time: the lamp
/// and the view's rails all start at zero.
const COMMIT_MS: u64 = 10_000;

const LAMP_RED: u16 = rgb565(255, 24, 24);
const MISS_RED: u16 = rgb565(255, 10, 10);
const ICE: u16 = rgb565(14, 22, 34);
const BOARDS: u16 = rgb565(150, 160, 170);
const CREASE: u16 = rgb565(70, 140, 255);
const OT_LOSSES: u16 = rgb565(60, 60, 60);

fn nhl_team(abbreviation: &str, score: u16, primary: u32) -> TeamState<'_> {
    TeamState {
        abbreviation,
        score,
        colors: TeamColors {
            primary,
            alternate: 0,
        },
    }
}

/// TB (red, away) at NYR (blue, home): second period, five on five.
fn nhl_live<'a>(goals: &'a [wire::Goal]) -> wire::Live<'a> {
    wire::Live {
        game_id: "401891781",
        period: 2,
        phase: LivePhase::InProgress,
        clock: "16:41",
        playoffs: false,
        away: nhl_team("TB", 1, 0xFF_0000),
        home: nhl_team("NYR", 1, 0x00_00FF),
        shots: Some(wire::Shots { away: 14, home: 8 }),
        strength: Some(wire::Strength {
            kind: wire::StrengthKind::Even,
            away_skaters: 5,
            home_skaters: 5,
        }),
        goals: wire::Goals::new(goals),
        last_scorer: "E. Tolvanen",
        shootout: None,
        last_play: None,
    }
}

const AWAY_GOAL: wire::Goal = wire::Goal {
    side: Side::Away,
    tag: wire::GoalTag::Goal,
    period: 1,
    elapsed: 441,
    coordinate: Some(wire::Coordinate { x: 70, y: -4 }),
};

const HOME_GOAL: wire::Goal = wire::Goal {
    side: Side::Home,
    tag: wire::GoalTag::PowerPlay,
    period: 2,
    elapsed: 100,
    coordinate: Some(wire::Coordinate { x: -75, y: 7 }),
};

fn strength(kind: wire::StrengthKind, away: u8, home: u8) -> Option<wire::Strength> {
    Some(wire::Strength {
        kind,
        away_skaters: away,
        home_skaters: home,
    })
}

/// Commit `games` five seconds apart, the last at [`COMMIT_MS`], and draw
/// whatever the store ends up holding.
fn hockey(variant: HockeyLiveVariant, games: &[wire::Live<'_>]) -> Harness {
    let mut store = Store::new();
    let first = COMMIT_MS - (games.len() as u64 - 1) * 5_000;
    for (index, game) in games.iter().enumerate() {
        commit(
            &mut store,
            GameDetail::Hockey(wire::Game::Live(*game)),
            first + index as u64 * 5_000,
        );
    }
    from_store(Mode::HockeyLive, &store, variant)
}

fn commit(store: &mut Store, detail: GameDetail<'_>, now_ms: u64) {
    let league = LeagueId::from_slug(Sport::Hockey, "nhl");
    let logos = CommitLogos {
        away: Some(LogoRef(0)),
        home: Some(LogoRef(0)),
    };
    store.commit_detail(&league, &detail, logos, now_ms, LocalClock::default());
}

fn from_store(mode: Mode, store: &Store, variant: HockeyLiveVariant) -> Harness {
    let mut harness = Harness::new(mode);
    let colors = harness.snapshot.ui_colors;
    harness.snapshot = store.snapshot().clone();
    harness.snapshot.ui_colors = colors;
    assert_eq!(harness.snapshot.mode, mode);
    harness.settings.hockey_live = variant;
    harness
}

fn sides(harness: &Harness) -> (u16, u16) {
    let view = &harness.snapshot.hockey_live;
    (pack(view.away_color), pack(view.home_color))
}

fn lit_in_slot(frame: &Frame, slot: Slice) -> usize {
    frame.lit_in(slot.x..slot.x + slot.width, slot.y..slot.y + slot.height)
}

fn count_in_slot(frame: &Frame, slot: Slice, color: u16) -> usize {
    frame.count(
        slot.x..slot.x + slot.width,
        slot.y..slot.y + slot.height,
        color,
    )
}

/// A packed color scaled per 565 channel — the renderer's glow arithmetic,
/// restated so the test does not borrow what it checks.
fn scaled(color: u16, num: u32, den: u32) -> u16 {
    let red = ((color >> 11) & 0x1F) as u32 * num / den;
    let green = ((color >> 5) & 0x3F) as u32 * num / den;
    let blue = (color & 0x1F) as u32 * num / den;
    ((red as u16) << 11) | ((green as u16) << 5) | blue as u16
}

const VARIANTS: [HockeyLiveVariant; 2] = [HockeyLiveVariant::A, HockeyLiveVariant::B];

#[test]
fn both_hockey_layouts_draw_their_crests_scores_clock_and_shots() {
    let goals = [AWAY_GOAL, HOME_GOAL];
    for variant in VARIANTS {
        let mut harness = hockey(variant, &[nhl_live(&goals)]);
        let frame = harness.draw();
        let normal = pack(harness.snapshot.ui_colors.clock_normal);
        let (logos, scores, clock, shots) = match variant {
            HockeyLiveVariant::A => {
                let table = HOCKEY_LIVE_A;
                assert_eq!(frame.pixel(table.divider_x, 30), DIM_GRAY, "A's rule");
                (
                    [table.logo_away, table.logo_home],
                    [table.score_away, table.score_home],
                    table.clock,
                    table.shots,
                )
            }
            HockeyLiveVariant::B => {
                let table = HOCKEY_LIVE_B;
                (
                    [table.logo_away, table.logo_home],
                    [table.score_away, table.score_home],
                    table.clock,
                    table.shots,
                )
            }
        };
        for slot in logos {
            assert_eq!(frame.pixel(slot.x, slot.y), CREST, "{variant:?}");
        }
        for slot in scores {
            assert!(count_in_slot(&frame, slot, WHITE) > 0, "{variant:?}");
        }
        assert!(count_in_slot(&frame, clock, normal) > 0, "{variant:?}");
        assert!(
            count_in_slot(&frame, shots, DIM_GRAY) > 0,
            "{variant:?} SHOTS"
        );
        assert!(
            count_in_slot(&frame, shots, WHITE) > 0,
            "{variant:?} counts"
        );
    }
}

/// D9: a mark is 2 × 2, each pixel the team color at 141/256 over what is
/// lit, so goals from one spot climb toward the team color instead of fusing.
#[test]
fn goal_marks_composite_over_the_rink_in_play_order() {
    let bare = hockey(HockeyLiveVariant::B, &[nhl_live(&[])]).draw();
    let mut one = hockey(HockeyLiveVariant::B, &[nhl_live(&[AWAY_GOAL])]);
    let (away, _) = sides(&one);
    let one = one.draw();
    let two = hockey(HockeyLiveVariant::B, &[nhl_live(&[AWAY_GOAL, AWAY_GOAL])]).draw();

    let (x, y) = mark_at(GoalMark {
        side: Side::Away,
        x: 70,
        y: -4,
    });
    // The away side attacks the right-hand net.
    assert!(x > layout::rink::POSITION.x + layout::rink::POSITION.width / 2);
    for (px, py) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
        assert_eq!(one.pixel(px, py), mix(bare.pixel(px, py), away, 141));
        assert_eq!(two.pixel(px, py), mix(one.pixel(px, py), away, 141));
        assert_ne!(two.pixel(px, py), away, "translucent, never solid");
    }
    // Nothing else on the rink moved.
    let mark = |px: i32, py: i32| (x..x + 2).contains(&px) && (y..y + 2).contains(&py);
    for py in 47..62 {
        for px in 1..127 {
            if !mark(px, py) {
                assert_eq!(one.pixel(px, py), bare.pixel(px, py), "({px}, {py})");
            }
        }
    }
}

#[test]
fn the_lamp_pulses_the_scorer_for_twenty_seconds_on_the_wall_rail() {
    let before = wire::Live {
        home: nhl_team("NYR", 0, 0x00_00FF),
        ..nhl_live(&[AWAY_GOAL])
    };
    let goals = [AWAY_GOAL, HOME_GOAL];
    let after = nhl_live(&goals);
    for variant in VARIANTS {
        let mut harness = hockey(variant, &[before, after]);
        let lamp = &harness.snapshot.hockey_live;
        assert_eq!(
            (lamp.lamp_side, lamp.lamp_ms),
            (Some(Side::Home), COMMIT_MS)
        );
        let (away_slot, home_slot) = match variant {
            HockeyLiveVariant::A => (HOCKEY_LIVE_A.score_away, HOCKEY_LIVE_A.score_home),
            HockeyLiveVariant::B => (HOCKEY_LIVE_B.score_away, HOCKEY_LIVE_B.score_home),
        };

        // Half a pulse in, the scorer's number is all lamp red.
        harness.now = WallMs(COMMIT_MS + 500);
        let frame = harness.draw();
        assert!(
            count_in_slot(&frame, home_slot, LAMP_RED) > 0,
            "{variant:?}"
        );
        assert_eq!(count_in_slot(&frame, away_slot, LAMP_RED), 0, "{variant:?}");

        // The window is a duration: twenty seconds of wall time end it,
        // whatever the frame rail did.
        harness.now = WallMs(COMMIT_MS + 20_000);
        let frame = harness.draw();
        assert_eq!(count_in_slot(&frame, home_slot, LAMP_RED), 0, "{variant:?}");
        assert!(count_in_slot(&frame, home_slot, WHITE) > 0, "{variant:?}");
    }
}

#[test]
fn the_newest_mark_blinks_white_while_the_lamp_burns() {
    let before = wire::Live {
        home: nhl_team("NYR", 0, 0x00_00FF),
        ..nhl_live(&[AWAY_GOAL])
    };
    let goals = [AWAY_GOAL, HOME_GOAL];
    let mut harness = hockey(HockeyLiveVariant::B, &[before, nhl_live(&goals)]);
    let (x, y) = mark_at(GoalMark {
        side: Side::Home,
        x: -75,
        y: 7,
    });
    // The home side attacks the left-hand net.
    assert!(x < layout::rink::POSITION.x + layout::rink::POSITION.width / 2);
    for (at, white) in [
        (0, true),
        (399, true),
        (400, false),
        (800, true),
        (20_000, false),
    ] {
        harness.now = WallMs(COMMIT_MS + at);
        let frame = harness.draw();
        assert_eq!(frame.pixel(x, y) == WHITE, white, "at {at} ms");
        assert_eq!(frame.pixel(x + 1, y + 1) == WHITE, white, "at {at} ms");
    }
}

#[test]
fn a_power_play_glows_the_attacking_zone_and_points_at_the_advantage() {
    let pp = wire::Live {
        strength: strength(wire::StrengthKind::PowerPlay(Side::Away), 5, 4),
        ..nhl_live(&[])
    };
    let mut harness = hockey(HockeyLiveVariant::B, &[pp]);
    let (away, home) = sides(&harness);
    let frame = harness.draw();
    let even = hockey(HockeyLiveVariant::B, &[nhl_live(&[])]).draw();

    // The away side attacks right: its zone, from inside the far blue line to
    // short of the goal line, glows a quarter of its color at the pulse's
    // start — the view's anchor.
    assert_eq!(even.pixel(100, 50), ICE);
    assert_eq!(frame.pixel(100, 50), scaled(away, 64, 256));
    // The creases the glow covered are drawn back over it; the home end, the
    // neutral zone and the other side's zone do not glow.
    assert_eq!(frame.pixel(116, 54), CREASE);
    assert_eq!(frame.pixel(30, 50), ICE);
    assert_eq!(frame.pixel(63, 50), even.pixel(63, 50));

    // The banner, with an arrow on its left — toward the away crest.
    let slot = HOCKEY_LIVE_B.situation;
    assert!(count_in_slot(&frame, slot, away) > 0);
    let title =
        scoreboard_render::font::measure("POWER PLAY", &scoreboard_render::generated::SPLEEN_5X8);
    let text_x = slot.x + (slot.width - title) / 2;
    assert!(frame.count(text_x - 6..text_x - 3, slot.y..slot.y + 6, away) > 0);

    // Five dots for the away side, four and an empty slot for the home side.
    let unfilled = layout::dot::PALETTE[2];
    let row = HOCKEY_LIVE_B.skaters_y;
    assert_eq!(frame.count(1..26, row..row + 4, unfilled), 0);
    assert!(frame.count(103..127, row..row + 4, unfilled) > 0);
    assert!(frame.count(103..127, row..row + 4, home) > 0);

    // A: the banner in the strip, the count under it, arrows up to the away
    // crest on both sides of it.
    let mut harness = hockey(HockeyLiveVariant::A, &[pp]);
    let frame = harness.draw();
    let table = HOCKEY_LIVE_A;
    assert!(count_in_slot(&frame, table.strip_top, away) > 0);
    assert!(count_in_slot(&frame, table.strip_bottom, away) > 0);
    for x in table.banner_arrows_x {
        // inning_top: the apex is the top row's middle pixel.
        assert_eq!(frame.pixel(x + 2, table.banner_arrows_y), away);
        assert_eq!(frame.pixel(x, table.banner_arrows_y), BLACK);
    }
}

#[test]
fn an_empty_net_counts_the_extra_attacker_in_the_accent_color() {
    let en = wire::Live {
        period: 3,
        strength: strength(wire::StrengthKind::EmptyNet(Side::Home), 5, 6),
        ..nhl_live(&[])
    };
    let mut harness = hockey(HockeyLiveVariant::B, &[en]);
    let (_, home) = sides(&harness);
    let accent = pack(harness.snapshot.ui_colors.accent);
    let frame = harness.draw();
    let row = HOCKEY_LIVE_B.skaters_y;
    // The sixth dot sits outermost on the home row, at the panel's last
    // legal column.
    assert!(frame.count(123..127, row..row + 4, accent) > 0);
    assert_eq!(frame.count(1..30, row..row + 4, accent), 0);
    // "EMPTY NET", pointing right — at the home crest.
    let slot = HOCKEY_LIVE_B.situation;
    let title =
        scoreboard_render::font::measure("EMPTY NET", &scoreboard_render::generated::SPLEEN_5X8);
    let text_end = slot.x + (slot.width - title) / 2 + title;
    assert!(frame.count(text_end + 3..text_end + 6, slot.y..slot.y + 6, home) > 0);
    // The home side's goalie is out, so the zone it attacks — the left — glows.
    assert_ne!(frame.pixel(30, 50), ICE);
    assert_eq!(frame.pixel(100, 50), ICE);

    let frame = hockey(HockeyLiveVariant::A, &[en]).draw();
    let table = HOCKEY_LIVE_A;
    assert!(count_in_slot(&frame, table.strip_top, home) > 0);
    assert!(
        count_in_slot(&frame, table.strip_bottom, home) > 0,
        "6 ON 5"
    );
    for x in table.banner_arrows_x {
        // inning_bottom: the apex is the bottom row's middle pixel.
        assert_eq!(frame.pixel(x + 2, table.banner_arrows_y + 2), home);
    }
}

#[test]
fn four_on_four_is_announced_without_a_side() {
    let matched = wire::Live {
        strength: strength(wire::StrengthKind::Matched, 4, 4),
        ..nhl_live(&[AWAY_GOAL])
    };
    let mut harness = hockey(HockeyLiveVariant::B, &[matched]);
    let (away, home) = sides(&harness);
    let frame = harness.draw();
    let slot = HOCKEY_LIVE_B.situation;
    assert!(count_in_slot(&frame, slot, WHITE) > 0, "4 ON 4");
    assert_eq!(
        count_in_slot(&frame, slot, away),
        0,
        "no goal line under it"
    );
    assert_eq!(frame.pixel(100, 50), ICE, "and no zone glows");
    assert_eq!(frame.pixel(30, 50), ICE);

    let frame = hockey(HockeyLiveVariant::A, &[matched]).draw();
    let table = HOCKEY_LIVE_A;
    assert!(
        count_in_slot(&frame, table.strip_top, DIM_GRAY) > 0,
        "EVEN STRENGTH"
    );
    assert!(
        count_in_slot(&frame, table.strip_bottom, WHITE) > 0,
        "4 ON 4"
    );
    assert_eq!(count_in_slot(&frame, table.strip_top, home), 0);
}

#[test]
fn three_on_three_overtime_is_play_as_usual_with_the_clock_running_out() {
    let goals = [AWAY_GOAL, HOME_GOAL];
    let overtime = wire::Live {
        period: 4,
        clock: "0:42",
        strength: strength(wire::StrengthKind::Matched, 3, 3),
        ..nhl_live(&goals)
    };
    for variant in VARIANTS {
        let mut harness = hockey(variant, &[overtime]);
        let (_, home) = sides(&harness);
        let warning = pack(harness.snapshot.ui_colors.clock_warning);
        let frame = harness.draw();
        let (clock, line) = match variant {
            HockeyLiveVariant::A => (HOCKEY_LIVE_A.clock, HOCKEY_LIVE_A.strip_top),
            HockeyLiveVariant::B => (HOCKEY_LIVE_B.clock, HOCKEY_LIVE_B.situation),
        };
        assert!(count_in_slot(&frame, clock, warning) > 0, "{variant:?}");
        // No banner: the newest goal, in its scorer's color.
        assert!(count_in_slot(&frame, line, home) > 0, "{variant:?}");
    }
    // Three filled dots and two empty slots a side.
    let frame = hockey(HockeyLiveVariant::B, &[overtime]).draw();
    let unfilled = layout::dot::PALETTE[2];
    let row = HOCKEY_LIVE_B.skaters_y;
    assert!(frame.count(1..26, row..row + 4, unfilled) > 0);
    assert!(frame.count(103..127, row..row + 4, unfilled) > 0);
}

#[test]
fn the_intermission_reads_int_in_the_accent_color() {
    let intermission = wire::Live {
        phase: LivePhase::EndOfPeriod,
        clock: "0:00",
        ..nhl_live(&[AWAY_GOAL])
    };
    for variant in VARIANTS {
        let mut harness = hockey(variant, &[intermission]);
        assert_eq!(harness.snapshot.hockey_live.clock_text.as_str(), "INT");
        let accent = pack(harness.snapshot.ui_colors.accent);
        let frame = harness.draw();
        let (clock, period) = match variant {
            HockeyLiveVariant::A => (HOCKEY_LIVE_A.clock, HOCKEY_LIVE_A.period),
            HockeyLiveVariant::B => (HOCKEY_LIVE_B.clock, HOCKEY_LIVE_B.period),
        };
        assert!(count_in_slot(&frame, clock, accent) > 0, "{variant:?}");
        assert!(
            count_in_slot(&frame, period, WHITE) > 0,
            "the chip keeps 2ND"
        );
    }
}

/// DAL at FLA, tied 3-3: FLA scored its third attempt and DAL shoots next.
fn shootout(next_shooter: Option<Side>) -> wire::Live<'static> {
    wire::Live {
        period: 5,
        clock: "0:00",
        away: nhl_team("DAL", 3, 0xFF_0000),
        home: nhl_team("FLA", 3, 0x00_00FF),
        strength: None,
        shootout: Some(wire::Shootout {
            away: wire::ShootoutAttempts {
                taken: 2,
                scored: 0,
            },
            home: wire::ShootoutAttempts {
                taken: 3,
                scored: 0b100,
            },
            next_shooter,
        }),
        ..nhl_live(&[])
    }
}

#[test]
fn the_shootout_takes_over_the_rink_layout() {
    let mut harness = hockey(HockeyLiveVariant::B, &[shootout(Some(Side::Away))]);
    let (_, home) = sides(&harness);
    let colors = harness.snapshot.ui_colors;
    let frame = harness.draw();
    let table = HOCKEY_LIVE_B;

    // The tally takes the clock's slot; the clock string never shows.
    assert!(count_in_slot(&frame, table.clock, pack(colors.accent)) > 0);
    assert_eq!(
        count_in_slot(&frame, table.clock, pack(colors.clock_normal)),
        0
    );
    assert!(
        count_in_slot(&frame, table.situation, DIM_GRAY) > 0,
        "SHOOTOUT"
    );

    // Home: miss, miss, goal. Away: miss, miss, then DAL's shooter up.
    let (x, y) = (table.shootout_home.x, table.shootout_home.y);
    assert_eq!(frame.pixel(x, y), MISS_RED);
    assert_eq!(frame.pixel(x + 3, y + 3), MISS_RED);
    assert_eq!(frame.pixel(x + 20 + 3, y + 3), home);
    let (x, y) = (table.shootout_away.x + 20, table.shootout_away.y);
    assert_eq!(frame.pixel(x, y), WHITE, "the ring");
    assert_eq!(frame.pixel(x + 1, y + 1), BLACK);
    assert_eq!(frame.pixel(x + 3, y + 3), WHITE, "the pip");

    // The rink shrinks to nine rows under the attempts, rounded corners and
    // all, and nothing draws where the full one would start.
    let top = table.shootout_rink_y;
    assert_eq!(frame.pixel(60, top), BOARDS);
    assert_eq!(frame.pixel(60, top + 8), BOARDS);
    assert_eq!(frame.pixel(2, top), BLACK, "a rounded corner");
    assert_eq!(frame.pixel(2, top + 4), BOARDS);
    assert_eq!(frame.pixel(60, top + 9), BLACK);
    assert_eq!(frame.lit_in(26..101, 48..top), 0);
}

#[test]
fn the_shootout_takes_over_the_ledger_layout() {
    let mut harness = hockey(HockeyLiveVariant::A, &[shootout(Some(Side::Away))]);
    let (_, home) = sides(&harness);
    let accent = pack(harness.snapshot.ui_colors.accent);
    let normal = pack(harness.snapshot.ui_colors.clock_normal);
    let frame = harness.draw();
    let table = HOCKEY_LIVE_A;
    assert!(count_in_slot(&frame, table.shootout_label, accent) > 0);
    assert!(count_in_slot(&frame, table.shootout_tally, WHITE) > 0);
    let (x, y) = (table.shootout_home.x + 20, table.shootout_home.y);
    assert_eq!(frame.pixel(x + 3, y + 3), home);
    assert_eq!(frame.pixel(table.divider_x, 30), DIM_GRAY);
    assert_eq!(
        frame.pixel(100, table.separator_y),
        BLACK,
        "no rule across the rows"
    );
    assert_eq!(
        count_in_slot(&frame, table.clock, normal),
        0,
        "no clock string"
    );
}

#[test]
fn a_finished_shootout_marks_no_next_shooter() {
    for variant in VARIANTS {
        let frame = hockey(variant, &[shootout(None)]).draw();
        let rows = match variant {
            HockeyLiveVariant::A => [HOCKEY_LIVE_A.shootout_away, HOCKEY_LIVE_A.shootout_home],
            HockeyLiveVariant::B => [HOCKEY_LIVE_B.shootout_away, HOCKEY_LIVE_B.shootout_home],
        };
        for slot in rows {
            assert_eq!(count_in_slot(&frame, slot, WHITE), 0, "{variant:?}");
            assert!(lit_in_slot(&frame, slot) > 0, "{variant:?}");
        }
    }
}

/// Ruling 3: the backend degrades to the scoreboard when the summary fetch
/// fails, and every screen still reads as whole.
#[test]
fn a_missing_summary_draws_no_banner_marks_or_scorer() {
    let bare = wire::Live {
        away: nhl_team("TB", 2, 0xFF_0000),
        shots: None,
        strength: None,
        last_scorer: "",
        ..nhl_live(&[])
    };
    let frame = hockey(HockeyLiveVariant::B, &[bare]).draw();
    let table = HOCKEY_LIVE_B;
    assert_eq!(lit_in_slot(&frame, table.situation), 0);
    assert_eq!(
        frame.lit_in(26..102, table.shots.y..table.shots_bar_y + 2),
        0,
        "no 0-0"
    );
    assert_eq!(
        frame.lit_in(1..127, table.skaters_y..table.skaters_y + 4),
        0,
        "no dots"
    );
    // The rink is the bare art.
    let plain = hockey(HockeyLiveVariant::B, &[nhl_live(&[])]).draw();
    for py in 47..62 {
        for px in 1..127 {
            assert_eq!(frame.pixel(px, py), plain.pixel(px, py), "({px}, {py})");
        }
    }

    // A scored game with its goals missing leaves the ledger's strip dark
    // rather than claim there are none.
    let frame = hockey(HockeyLiveVariant::A, &[bare]).draw();
    let table = HOCKEY_LIVE_A;
    assert_eq!(frame.lit_in(51..127, 38..63), 0);
    assert_eq!(lit_in_slot(&frame, table.shots), 0);
    // A scoreless one says so.
    let scoreless = wire::Live {
        away: nhl_team("TB", 0, 0xFF_0000),
        home: nhl_team("NYR", 0, 0x00_00FF),
        ..bare
    };
    let frame = hockey(HockeyLiveVariant::A, &[scoreless]).draw();
    assert!(
        count_in_slot(&frame, table.strip_empty, DIM_GRAY) > 0,
        "NO GOALS"
    );
}

#[test]
fn the_hockey_flash_takes_each_layouts_own_strip() {
    let flashing = wire::Live {
        last_play: Some(scoreboard_wire::LastPlay {
            id: "p9",
            text: "GOAL! Mika Zibanejad (2) Wrist Shot",
        }),
        ..nhl_live(&[AWAY_GOAL])
    };
    // B gives up the whole band under the shots, rink and all.
    let mut harness = hockey(HockeyLiveVariant::B, &[flashing]);
    let frame = harness.draw();
    let strip = HOCKEY_LIVE_B.strip;
    assert!(frame.lit_in(1..geometry::COLUMN_STRIP.x, strip.y..strip.y + strip.height) > 0);
    assert_eq!(
        frame.count(1..127, 47..63, BOARDS),
        0,
        "the rink is skipped"
    );

    // A keeps its identity column and gives up the data column's strip.
    let mut harness = hockey(HockeyLiveVariant::A, &[flashing]);
    let (away, _) = sides(&harness);
    let frame = harness.draw();
    let strip = geometry::COLUMN_STRIP;
    assert!(
        frame.lit_in(
            strip.x..strip.x + strip.width,
            strip.y..strip.y + strip.height
        ) > 0
    );
    assert_eq!(count_in_slot(&frame, HOCKEY_LIVE_A.strip_top, away), 0);
}

fn nhl_final(stars: bool, series: bool, shots: bool) -> Harness {
    let team = |abbreviation, primary, line| scoreboard_wire::FinalTeam {
        abbreviation,
        score: 0,
        colors: TeamColors {
            primary,
            alternate: 0,
        },
        line_score: line,
    };
    let game = wire::Final {
        game_id: "401874173",
        periods_played: 5,
        playoffs: true,
        away: scoreboard_wire::FinalTeam {
            score: 4,
            ..team("CAR", 0xFF_0000, &[0, 0, 4, 0, 0])
        },
        home: scoreboard_wire::FinalTeam {
            score: 5,
            ..team("VGK", 0x00_00FF, &[0, 4, 0, 0, 1])
        },
        shots: shots.then_some(wire::Shots { away: 33, home: 35 }),
        stars: stars.then_some([
            wire::Star {
                name: "S. Theodore",
                side: Some(Side::Home),
            },
            wire::Star {
                name: "S. Aho",
                side: Some(Side::Away),
            },
            wire::Star {
                name: "B. McNabb",
                side: None,
            },
        ]),
        series: series.then_some("VGK leads series 2-1"),
    };
    let mut store = Store::new();
    commit(
        &mut store,
        GameDetail::Hockey(wire::Game::Final(game)),
        COMMIT_MS,
    );
    from_store(Mode::HockeyFinal, &store, HockeyLiveVariant::B)
}

#[test]
fn the_stars_final_colors_the_winner_and_names_the_stars() {
    let mut harness = nhl_final(true, true, true);
    let shared = &harness.snapshot.linescore_final;
    let (away, home) = (pack(shared.away_color), pack(shared.home_color));
    let accent = pack(harness.snapshot.ui_colors.accent);
    let frame = harness.draw();
    let table = HOCKEY_FINAL;

    assert!(
        count_in_slot(&frame, table.score_home, home) > 0,
        "the winner"
    );
    assert!(
        count_in_slot(&frame, table.score_away, DIM_GRAY) > 0,
        "the loser"
    );
    assert!(
        count_in_slot(&frame, table.final_label, accent) > 0,
        "F/2OT"
    );

    // One gold star for the first, three for the third — the star's top pixel
    // is its middle column.
    let gold = layout::star::PALETTE[1];
    for (rank, row) in table.star_rows.iter().enumerate() {
        for count in 0..3 {
            let x = table.star_x + count * table.star_pitch + 2;
            let lit = frame.pixel(x, row.y + 1) == gold;
            assert_eq!(lit, count <= rank as i32, "rank {rank}, star {count}");
        }
    }
    // Names in their own team's color, winner or not; an unknown side white.
    assert!(count_in_slot(&frame, table.star_rows[0], home) > 0);
    assert!(count_in_slot(&frame, table.star_rows[1], away) > 0);
    assert!(count_in_slot(&frame, table.star_rows[2], WHITE) > 0);

    assert_eq!(frame.pixel(100, table.rule_y), DIM_GRAY);
    assert!(count_in_slot(&frame, table.series, WHITE) > 0);
    assert!(count_in_slot(&frame, table.shots, DIM_GRAY) > 0);
}

/// The stars arrive minutes after the horn. Until then their band carries the
/// line score, so the screen reads as a finished final, not a broken one.
#[test]
fn a_final_without_its_stars_shows_the_line_score_in_their_place() {
    let mut harness = nhl_final(false, true, true);
    let home = pack(harness.snapshot.linescore_final.home_color);
    let frame = harness.draw();
    let table = HOCKEY_FINAL;
    let gold = layout::star::PALETTE[1];
    assert_eq!(frame.count(49..127, 10..43, gold), 0);
    let [header, away_row, home_row] = table.linescore_rows;
    assert!(count_in_slot(&frame, header, DIM_GRAY) > 0, " 1  2  3 OT");
    assert!(
        count_in_slot(&frame, away_row, DIM_GRAY) > 0,
        "the loser's row"
    );
    assert!(
        count_in_slot(&frame, home_row, home) > 0,
        "the winner's row"
    );
}

#[test]
fn a_lone_series_or_shots_line_takes_the_middle_of_its_band() {
    let table = HOCKEY_FINAL;
    let below_rule = table.rule_y + 1..63;
    for (series, shots, color) in [(false, true, DIM_GRAY), (true, false, WHITE)] {
        let frame = nhl_final(true, series, shots).draw();
        let single = table.single_line;
        assert!(count_in_slot(&frame, single, color) > 0);
        assert_eq!(
            frame.lit_in(49..127, below_rule.start..single.y),
            0,
            "series {series}, shots {shots}"
        );
    }
    let frame = nhl_final(true, false, false).draw();
    assert_eq!(frame.lit_in(49..127, below_rule), 0);
}

fn nhl_pregame(records: bool, goalies: bool) -> Harness {
    let team = |abbreviation, primary, record: wire::Record, goalie| wire::PregameTeam {
        abbreviation,
        colors: TeamColors {
            primary,
            alternate: 0,
        },
        record: records.then_some(record),
        goalie: goalies.then_some(goalie),
    };
    let game = wire::Pregame {
        game_id: "401874176",
        start_time: 1_781_481_600,
        venue: "T-Mobile Arena",
        info_secondary: "Stanley Cup Final - Game 6 - CAR leads series 3-2",
        playoffs: true,
        away: team(
            "CAR",
            0xFF_0000,
            wire::Record {
                wins: 53,
                losses: 22,
                ot_losses: 7,
            },
            "B. Bussi",
        ),
        home: team(
            "VGK",
            0x00_00FF,
            wire::Record {
                wins: 39,
                losses: 26,
                ot_losses: 17,
            },
            "C. Hart",
        ),
    };
    let mut store = Store::new();
    commit(
        &mut store,
        GameDetail::Hockey(wire::Game::Pregame(game)),
        COMMIT_MS,
    );
    from_store(Mode::Pregame, &store, HockeyLiveVariant::B)
}

#[test]
fn a_hockey_pregame_stacks_three_record_rows_down_the_crest() {
    let mut harness = nhl_pregame(true, true);
    let pregame = &harness.snapshot.pregame;
    let (away, home) = (pack(pregame.away.color), pack(pregame.home.color));
    let frame = harness.draw();
    let table = geometry::PREGAME;
    for (rows, crest) in [
        (table.hockey_record_away, table.logo_away),
        (table.hockey_record_home, table.logo_home),
    ] {
        assert_eq!(rows[0].y, crest.y, "the stack starts at the crest");
        assert_eq!(
            rows[2].y + rows[2].height,
            crest.y + crest.height,
            "and ends with it"
        );
        assert!(count_in_slot(&frame, rows[0], WHITE) > 0, "wins");
        assert!(count_in_slot(&frame, rows[1], DIM_GRAY) > 0, "losses");
        assert!(
            count_in_slot(&frame, rows[2], OT_LOSSES) > 0,
            "overtime losses"
        );
    }
    // The goalies are the per-team lines.
    assert!(count_in_slot(&frame, table.team_line_away, away) > 0);
    assert!(count_in_slot(&frame, table.team_line_home, home) > 0);
}

#[test]
fn a_hockey_pregame_without_records_or_goalies_leaves_them_blank() {
    let frame = nhl_pregame(false, false).draw();
    let table = geometry::PREGAME;
    assert_eq!(frame.lit_in(25..45, 1..63), 0, "no stack");
    assert_eq!(lit_in_slot(&frame, table.team_line_away), 0);
    assert_eq!(lit_in_slot(&frame, table.team_line_home), 0);
}

#[test]
fn only_hockey_grows_the_third_record_row() {
    let mut harness = pregame();
    harness.snapshot.pregame.away.record = Some(Record {
        wins: 51,
        losses: 44,
        ot_losses: Some(7),
    });
    let frame = harness.draw();
    assert_eq!(frame.count(25..45, 1..63, OT_LOSSES), 0);
}

#[test]
fn every_hockey_screen_keeps_off_the_edge_ring() {
    let goals = [AWAY_GOAL, HOME_GOAL];
    let states = [
        nhl_live(&goals),
        wire::Live {
            strength: strength(wire::StrengthKind::EmptyNet(Side::Home), 5, 6),
            ..nhl_live(&goals)
        },
        wire::Live {
            strength: strength(wire::StrengthKind::EmptyNet(Side::Away), 6, 5),
            last_scorer: "A. Very-Long-Hyphenated-Name",
            ..nhl_live(&goals)
        },
        shootout(Some(Side::Away)),
        wire::Live {
            last_play: Some(scoreboard_wire::LastPlay {
                id: "p9",
                text: "GOAL! Mika Zibanejad (2) Wrist Shot, assists: Artemi Panarin (4)",
            }),
            ..nhl_live(&goals)
        },
    ];
    let mut harnesses: Vec<(String, Harness)> = Vec::new();
    for variant in VARIANTS {
        for (index, state) in states.iter().enumerate() {
            harnesses.push((format!("{variant:?} {index}"), hockey(variant, &[*state])));
        }
    }
    for (stars, series) in [(true, true), (false, true), (true, false)] {
        harnesses.push((
            format!("final {stars} {series}"),
            nhl_final(stars, series, true),
        ));
    }
    harnesses.push(("pregame".into(), nhl_pregame(true, true)));
    for (name, harness) in &mut harnesses {
        for at in SCROLL_SAMPLES {
            harness.view = FrameElapsed(at);
            harness.play = FrameElapsed(at);
            assert_eq!(edge_ring(&harness.draw()), 0, "{name} at {at} ms");
        }
    }
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
        Mode::HockeyLive,
        Mode::HockeyFinal,
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

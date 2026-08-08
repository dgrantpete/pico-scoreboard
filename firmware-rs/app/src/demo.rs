//! The stand-in for core 0, and the only reason it exists is deliverable 4.
//!
//! Phase 3's real core-0 story — Wi-Fi, the poller, the HTTP server, storage,
//! inputs — arrives in later tasks. Until then something has to push state
//! through the [`SnapshotChannel`](scoreboard_model::SnapshotChannel) so the
//! render loop has something to render and the frame-time probe has something
//! to measure, and that something should be the *hard* frames, not a smiley
//! face.
//!
//! So this cycles six scenarios chosen to bracket the render budget, each held
//! long enough for the probe to gather a hundred frames:
//!
//! 1. **Startup** — a static screen recommitted six times. Redraw-on-commit,
//!    skip in between.
//! 2. **Idle** — a static screen with no commits at all: the skip path, and the
//!    floor under every measurement.
//! 3. **MLB live + play flash at the 255-byte cap** — the wire format's largest
//!    legal play line, scrolling through a 76 px window, over a screen that
//!    also draws two crests, count dots, base markers and an inning. This is
//!    the case MicroPython could not draw glyph-by-glyph.
//! 4. **Final line score, three overflowing rows** — the other one: three rows
//!    scrolling in lockstep measured ~41 ms of a 50 ms frame in MicroPython and
//!    is why `state.py` pre-rendered text into 1-bit strips at all.
//! 5. **The same screen under a sticky spinner toast** — adds a full-frame dim
//!    pass over all 8,192 pixels plus an animated 12-dot sprite, and forces a
//!    redraw every frame.
//! 6. **The league menu** — preempts the mode dispatch, marquees every frame.
//!
//! Nothing here is a fixture of record: the parity harness
//! (`crates/scoreboard-render/tests/parity_frames.rs`) owns correctness against
//! the real corpus. These are shaped *like* corpus data so the numbers mean
//! something, and they go away with this module.

use embassy_time::{Duration, Instant, Timer};
use scoreboard_model::snapshot::{Bases, InningHalf, LogoRef, MenuRow};
use scoreboard_model::{
    Millis, Mode, Publisher, Rgb888, ScoreboardSnapshot, Sport, Text, ToastKind,
};

use crate::display_core1::BRIGHTNESS;
use crate::probe::{self, Scenario};

/// How long each scenario holds. 100 frames at 20 FPS — enough for a mean that
/// is not one unlucky frame, short enough that a full cycle is half a minute.
const SCENARIO: Duration = Duration::from_secs(5);

/// A play-by-play line, repeated until it hits the wire's 255-byte ceiling.
const PLAY_PHRASE: &str = "MARQUEZ SINGLED TO LEFT, GONZALEZ SCORED, RODRIGUEZ TO THIRD. ";

fn now_ms() -> Millis {
    Instant::now().as_millis()
}

fn text<const N: usize>(value: &str) -> Text<N> {
    let mut out = Text::new();
    let _ = out.push_str(value);
    out
}

/// Core 0's publisher side: builds the snapshots, announces the scenario.
#[embassy_executor::task]
pub async fn feed(mut publisher: Publisher<'static>) -> ! {
    let mut snapshot = ScoreboardSnapshot::new();
    dress(&mut snapshot);
    let mut commits: u32 = 0;

    loop {
        startup(&mut snapshot, &mut publisher, &mut commits).await;
        idle(&mut snapshot, &mut publisher, &mut commits).await;
        mlb_play_flash(&mut snapshot, &mut publisher, &mut commits).await;
        final_linescore(&mut snapshot, &mut publisher, &mut commits).await;
        toast_overlay(&mut snapshot, &mut publisher, &mut commits).await;
        menu(&mut snapshot, &mut publisher, &mut commits).await;
    }
}

/// Core 0's other high-rate output: the auto-brightness atomic.
///
/// A real light sensor moves this at a human pace; the sweep here is only fast
/// enough that the probe catches several applies per scenario, which is what
/// makes "brightness costs nothing per frame" a measurement rather than an
/// assumption.
#[embassy_executor::task]
pub async fn brightness() -> ! {
    const FLOOR: u8 = 40;
    let mut level: u8 = FLOOR;
    let mut rising = true;
    loop {
        Timer::after_millis(200).await;
        level = match (rising, level) {
            (true, l) if l >= u8::MAX - 15 => {
                rising = false;
                u8::MAX
            }
            (true, l) => l + 15,
            (false, l) if l <= FLOOR + 15 => {
                rising = true;
                FLOOR
            }
            (false, l) => l - 15,
        };
        BRIGHTNESS.store(level, core::sync::atomic::Ordering::Relaxed);
    }
}

/// The fields every screen reads that do not change between scenarios.
fn dress(snapshot: &mut ScoreboardSnapshot) {
    snapshot.ui_colors.primary = Rgb888::new(255, 255, 255);
    snapshot.ui_colors.secondary = Rgb888::new(120, 120, 120);
    snapshot.ui_colors.accent = Rgb888::new(0, 220, 90);
    snapshot.ui_colors.clock_normal = Rgb888::new(210, 210, 210);
    snapshot.ui_colors.clock_warning = Rgb888::new(255, 170, 0);
    snapshot.game_id = text("401581000");
    snapshot.away_abbr = text("NYY");
    snapshot.home_abbr = text("BOS");
    snapshot.away_logo = Some(LogoRef(0));
    snapshot.home_logo = Some(LogoRef(1));
}

/// Publish, bumping the commit sequence — the thing the render loop's skip and
/// the prepared view both key on.
fn commit(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    // The config→snapshot seam. UI colours are model state rather than a
    // core-1 setting, so `PUT /api/config` cannot apply them itself — whoever
    // owns the snapshot has to, before its next commit. Today that is this
    // stand-in; task #11's poller inherits exactly this call, which is why it
    // is here rather than folded into `dress`.
    if let Some(colors) = crate::settings::take_ui_colors() {
        snapshot.ui_colors = colors;
    }
    *commits += 1;
    snapshot.commit_seq = *commits;
    publisher.publish(snapshot);
}

/// Restamp the animation epoch: the displayed view's identity changed, so every
/// continuous animation restarts from zero.
fn restart_animations(snapshot: &mut ScoreboardSnapshot) {
    snapshot.animation_start_ms = now_ms();
    snapshot.toast.updated_ms = 0;
    snapshot.toast.sticky = false;
    snapshot.play.updated_ms = 0;
    snapshot.play.text.clear();
    snapshot.play.id.clear();
    snapshot.menu.active = false;
}

async fn startup(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    probe::enter(Scenario::Startup);
    restart_animations(snapshot);
    snapshot.mode = Mode::Startup;
    snapshot.startup.total_steps = 6;
    snapshot.startup.attempts_total = 3;

    const STEPS: [(&str, &str); 6] = [
        ("HARDWARE", "PANEL + INPUTS"),
        ("STORAGE", "READING CONFIG"),
        ("WIFI", "HOME-NETWORK-5G"),
        ("TIME", "SYNCING CLOCK"),
        ("BACKEND", "FETCHING SLATE"),
        ("READY", ""),
    ];
    for (index, (operation, detail)) in STEPS.iter().enumerate() {
        snapshot.startup.step = index as u8 + 1;
        snapshot.startup.attempt = (index as u8 % 3) + 1;
        snapshot.startup.operation = text(operation);
        snapshot.startup.detail = text(detail);
        commit(snapshot, publisher, commits);
        Timer::after(SCENARIO / STEPS.len() as u32).await;
    }
}

async fn idle(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    probe::enter(Scenario::Idle);
    restart_animations(snapshot);
    snapshot.mode = Mode::Idle;
    // One commit and then silence, which is the point: everything the probe
    // reports for this scenario is the skip path.
    commit(snapshot, publisher, commits);
    Timer::after(SCENARIO).await;
}

async fn mlb_play_flash(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    probe::enter(Scenario::MlbPlayFlash);
    restart_animations(snapshot);
    snapshot.mode = Mode::MlbLive;

    let live = &mut snapshot.mlb_live;
    live.half = InningHalf::Bottom;
    live.inning_text = text("7th");
    live.away_score = 4;
    live.home_score = 7;
    live.balls = 3;
    live.strikes = 2;
    live.outs = 2;
    live.bases = Bases {
        first: true,
        second: false,
        third: true,
    };
    live.pitch_color = Some(Rgb888::new(200, 40, 40));
    live.bat_color = Some(Rgb888::new(40, 90, 220));
    live.pitcher = text("G. MARQUEZ");
    live.batter = text("R. DEVERS");
    live.has_at_bat = true;

    // Exactly the wire's ceiling: fill with whole phrases, then top up a byte
    // at a time so the line is 255 bytes and not 254.
    snapshot.play.id = text("play-255");
    snapshot.play.text.clear();
    while snapshot.play.text.push_str(PLAY_PHRASE).is_ok() {}
    while snapshot.play.text.push('.').is_ok() {}
    // Stamped once. The flash's window is derived from the line's width, and at
    // 2,040 px through a 76 px slot that is a minute and a half — far longer
    // than this scenario, so the scroll runs the whole way without restarting.
    snapshot.play.updated_ms = now_ms();

    commit(snapshot, publisher, commits);
    Timer::after(SCENARIO / 2).await;

    // A second commit mid-scenario, without touching `play.updated_ms`: the
    // scroll carries on where it was, and the probe gets a rebuild measurement
    // for the case that has to measure 255 glyphs to size the flash window.
    snapshot.mlb_live.strikes = 1;
    commit(snapshot, publisher, commits);
    Timer::after(SCENARIO / 2).await;
}

/// Twelve innings of line score in each row, which is wider than the 75 px
/// window they show through, so all three scroll.
fn dress_linescore(snapshot: &mut ScoreboardSnapshot) {
    let view = &mut snapshot.linescore_final;
    view.sport = Sport::Mlb;
    view.away_score = 9;
    view.home_score = 11;
    view.final_text = text("F/12");
    view.header_row = text("  1  2  3  4  5  6  7  8  9 10 11 12");
    view.away_row = text("  0  2  0  1  0  3  0  0  1  0  2  0");
    view.home_row = text("  1  0  4  0  0  0  2  1  0  1  0  2");
    view.home_won = true;
    view.away_color = Rgb888::new(200, 40, 40);
    view.home_color = Rgb888::new(40, 90, 220);
}

async fn final_linescore(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    probe::enter(Scenario::FinalLinescore);
    restart_animations(snapshot);
    snapshot.mode = Mode::Final;
    dress_linescore(snapshot);
    commit(snapshot, publisher, commits);
    Timer::after(SCENARIO).await;
}

async fn toast_overlay(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    probe::enter(Scenario::ToastOverlay);
    // Deliberately *not* restarting animations: the line score keeps scrolling
    // from where the previous scenario left it, so the toast's cost lands on
    // top of a screen already doing its most expensive thing.
    snapshot.mode = Mode::Final;
    dress_linescore(snapshot);
    snapshot.toast.kind = ToastKind::Spinner;
    snapshot.toast.sticky = true;
    snapshot.toast.updated_ms = now_ms();
    snapshot.toast.text.clear();
    commit(snapshot, publisher, commits);
    Timer::after(SCENARIO).await;
}

async fn menu(
    snapshot: &mut ScoreboardSnapshot,
    publisher: &mut Publisher<'static>,
    commits: &mut u32,
) {
    probe::enter(Scenario::Menu);
    restart_animations(snapshot);

    const LEAGUES: [&str; 5] = [
        "MAJOR LEAGUE BASEBALL",
        "NATIONAL BASKETBALL ASSOCIATION",
        "NATIONAL FOOTBALL LEAGUE",
        "COLLEGE FOOTBALL",
        "PREMIER LEAGUE",
    ];
    snapshot.menu.active = true;
    snapshot.menu.rows.clear();
    for (index, label) in LEAGUES.iter().enumerate() {
        let row = MenuRow {
            label: text(label),
            checked: index % 2 == 0,
            source: index as u8,
        };
        let _ = snapshot.menu.rows.push(row);
    }
    snapshot.menu.thumb_y = 8;
    snapshot.menu.thumb_h = 20;

    // Walk the cursor down the list. Each move restamps the marquee, which is
    // the only thing that makes a long label scroll.
    for highlight in 0..5i8 {
        snapshot.menu.highlight = highlight;
        snapshot.menu.updated_ms = now_ms();
        commit(snapshot, publisher, commits);
        Timer::after(SCENARIO / 5).await;
    }
    // The menu closes when the next scenario calls `restart_animations`, which
    // is also what publishes the close — closing it here would only mutate a
    // snapshot nobody reads again.
}

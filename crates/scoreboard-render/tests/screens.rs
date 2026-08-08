//! Screen renderers, driven through the real `hub75` display API and asserted
//! on the frame its simulator sink receives — the same path the driver takes on
//! the device, minus the panel.

use hub75::display::{FrameBytes, Hub75Display};
use hub75::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};
use hub75::sim::SimulatorSink;
use scoreboard_model::snapshot::{MenuRow, MenuView, ToastView};
use scoreboard_model::{Millis, Rgb888, ScoreboardSnapshot, SetupReason, Text, ToastKind};
use scoreboard_render::blit::Canvas;
use scoreboard_render::prepared::PreparedView;
use scoreboard_render::time::WallMs;
use scoreboard_render::{BLACK, menu, pack, qr, screens, toast};

const PRIMARY: Rgb888 = Rgb888::new(255, 0, 0);
const ACCENT: Rgb888 = Rgb888::new(0, 255, 0);
const SECONDARY: Rgb888 = Rgb888::new(0, 0, 255);
const WARNING: Rgb888 = Rgb888::new(255, 255, 0);

/// A frame as the driver would receive it.
struct Frame(Vec<u8>);

impl Frame {
    fn pixel(&self, x: i32, y: i32) -> u16 {
        let index = (y as usize * WIDTH + x as usize) * 2;
        u16::from_le_bytes([self.0[index], self.0[index + 1]])
    }

    fn lit(&self, x: i32, y: i32) -> bool {
        self.pixel(x, y) != BLACK
    }

    fn lit_in(&self, x: core::ops::Range<i32>, y: core::ops::Range<i32>) -> usize {
        y.flat_map(|row| x.clone().map(move |col| (col, row)))
            .filter(|(col, row)| self.lit(*col, *row))
            .count()
    }
}

/// Draw with the crate, push through `Hub75Display::show`, and read back what
/// the sink got.
fn render(draw: impl FnOnce(&mut Canvas<'_>)) -> Frame {
    let mut buffer: Box<FrameBytes> = Box::new([0; RGB565_FRAME_BYTES]);
    let mut display = Hub75Display::new(&mut buffer, SimulatorSink::new());
    {
        let mut canvas = Canvas::new(display.buffer_mut(), WIDTH as i32, HEIGHT as i32);
        draw(&mut canvas);
    }
    display.show();
    let sink = display.sink_mut();
    assert_eq!(sink.frames_shown(), 1);
    Frame(sink.front().to_vec())
}

fn snapshot() -> ScoreboardSnapshot {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.ui_colors.primary = PRIMARY;
    snapshot.ui_colors.accent = ACCENT;
    snapshot.ui_colors.secondary = SECONDARY;
    snapshot.ui_colors.clock_warning = WARNING;
    snapshot
}

fn text<const N: usize>(value: &str) -> Text<N> {
    let mut out = Text::new();
    out.push_str(value).expect("test text fits");
    out
}

// -- Static screens ----------------------------------------------------------

#[test]
fn idle_draws_its_two_lines_and_nothing_on_the_edges() {
    let snapshot = snapshot();
    let frame = render(|canvas| screens::idle(canvas, &snapshot));

    assert!(frame.lit_in(0..WIDTH as i32, 16..32) > 0, "title band");
    assert!(frame.lit_in(0..WIDTH as i32, 40..48) > 0, "subtitle band");
    assert_eq!(frame.lit_in(0..WIDTH as i32, 0..16), 0, "above the title");
    assert_eq!(
        frame.lit_in(0..WIDTH as i32, 48..64),
        0,
        "below the subtitle"
    );
}

#[test]
fn the_startup_bar_tracks_the_step() {
    // 80 px bar centered at x = 24, one pixel of border: the fill runs from
    // x = 25 for (80 - 2) * percent / 100 pixels.
    let mut snapshot = snapshot();
    snapshot.startup.step = 1;
    snapshot.startup.total_steps = 5;
    let frame = render(|canvas| screens::startup(canvas, &snapshot));

    let fill = pack(ACCENT);
    assert_eq!(frame.pixel(25, 27), fill, "the first fill column");
    assert_eq!(
        frame.pixel(25 + 14, 27),
        fill,
        "the last of 15 fill columns"
    );
    assert_eq!(frame.pixel(25 + 15, 27), BLACK, "one past the fill");
    assert_eq!(frame.pixel(24, 24), pack(SECONDARY), "the border");
}

#[test]
fn the_startup_attempt_dots_appear_only_while_retrying() {
    let mut snapshot = snapshot();
    snapshot.startup.attempts_total = 0;
    let without = render(|canvas| screens::startup(canvas, &snapshot));
    snapshot.startup.attempts_total = 3;
    snapshot.startup.attempt = 2;
    let with = render(|canvas| screens::startup(canvas, &snapshot));

    assert_eq!(without.lit_in(0..WIDTH as i32, 34..38), 0);
    assert!(with.lit_in(0..WIDTH as i32, 34..38) > 0);
}

#[test]
fn an_error_without_a_title_still_says_error() {
    let mut snapshot = snapshot();
    snapshot
        .error
        .lines
        .push(text("backend not responding"))
        .ok();
    let implicit = render(|canvas| screens::error(canvas, &snapshot));

    snapshot.error.title = text("ERROR");
    let explicit = render(|canvas| screens::error(canvas, &snapshot));
    assert_eq!(implicit.0, explicit.0);
}

#[test]
fn error_lines_stack_down_the_screen() {
    let mut snapshot = snapshot();
    for line in ["one", "two", "three", "four"] {
        snapshot.error.lines.push(text(line)).ok();
    }
    let frame = render(|canvas| screens::error(canvas, &snapshot));
    for y in [24, 34, 44, 54] {
        assert!(frame.lit_in(0..WIDTH as i32, y..y + 8) > 0, "line at y={y}");
    }
}

// -- Setup + QR --------------------------------------------------------------

fn setup_snapshot() -> ScoreboardSnapshot {
    let mut snapshot = snapshot();
    snapshot.commit_seq = 1;
    snapshot.setup.reason = SetupReason::NoConfig;
    snapshot.setup.ap_ssid = text("pico-scoreboard");
    snapshot.setup.title = text("SETUP");
    snapshot.setup.line_18 = text("Scan QR or join");
    snapshot.setup.line_28 = text("\"pico-scoreboard\" WiFi");
    snapshot.setup.line_44 = text("Then go to");
    snapshot.setup.line_54 = text("192.168.4.1");
    snapshot
}

#[test]
fn the_setup_screen_blits_its_qr_in_the_top_right() {
    let snapshot = setup_snapshot();
    let mut prepared = PreparedView::new();
    assert!(prepared.sync(&snapshot));
    let size = prepared.qr().size();
    assert!(size > 0, "the QR should have been built");

    let frame = render(|canvas| screens::setup(canvas, &snapshot, &prepared, WallMs(0)));
    let qr_x = WIDTH as i32 - size - 2;
    // The quiet zone is white, and the top-left module of the symbol is dark.
    assert_eq!(frame.pixel(qr_x, 2), 0xFFFF, "quiet zone");
    assert_eq!(
        frame.pixel(qr_x + qr::QUIET_ZONE, 2 + qr::QUIET_ZONE),
        BLACK,
        "the finder pattern's corner"
    );
}

#[test]
fn setup_lines_that_meet_the_qr_stop_short_of_it() {
    // `Regions.update_for_qr`'s rule: a line whose vertical range intersects the
    // QR is narrowed to end four pixels before its left edge; a line entirely
    // below it keeps the full width.
    let mut snapshot = setup_snapshot();
    snapshot.setup.line_18 = text("XXXXXXXXXXXXXXXXXXXXXXXXX");
    snapshot.setup.line_54 = text("XXXXXXXXXXXXXXXXXXXXXXXXX");
    let mut prepared = PreparedView::new();
    prepared.sync(&snapshot);
    let size = prepared.qr().size();
    let qr_x = WIDTH as i32 - size - 2;

    let frame = render(|canvas| screens::setup(canvas, &snapshot, &prepared, WallMs(0)));
    assert_eq!(
        frame.lit_in(qr_x - 4..qr_x, 18..26),
        0,
        "the gap between the narrowed line and the QR"
    );
    assert!(
        frame.lit_in(qr_x - 4..qr_x, 54..62) > 0,
        "a line below the QR keeps the full width"
    );
}

#[test]
fn a_failed_join_colors_the_title_as_a_warning() {
    let mut snapshot = setup_snapshot();
    let mut prepared = PreparedView::new();
    prepared.sync(&snapshot);
    let normal = render(|canvas| screens::setup(canvas, &snapshot, &prepared, WallMs(0)));

    snapshot.setup.reason = SetupReason::BadAuth;
    let failed = render(|canvas| screens::setup(canvas, &snapshot, &prepared, WallMs(0)));

    let title_pixel = (0..WIDTH as i32)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .find(|(x, y)| normal.lit(*x, *y))
        .expect("the title drew something");
    assert_eq!(normal.pixel(title_pixel.0, title_pixel.1), pack(ACCENT));
    assert_eq!(failed.pixel(title_pixel.0, title_pixel.1), pack(WARNING));
}

// -- Prepared view -----------------------------------------------------------

#[test]
fn the_prepared_view_rebuilds_only_on_a_new_commit() {
    let mut snapshot = setup_snapshot();
    let mut prepared = PreparedView::new();
    assert!(prepared.sync(&snapshot), "the first sync always builds");
    assert!(!prepared.sync(&snapshot), "the same commit is a no-op");

    snapshot.commit_seq += 1;
    assert!(prepared.sync(&snapshot));
    assert_eq!(prepared.commit_seq(), Some(snapshot.commit_seq));
}

#[test]
fn the_qr_follows_the_ssid_and_survives_an_unrelated_commit() {
    let mut snapshot = setup_snapshot();
    let mut prepared = PreparedView::new();
    prepared.sync(&snapshot);
    let first: Vec<u8> = prepared.qr().source().data.to_vec();

    // A commit that does not change the SSID leaves the bitmap untouched.
    snapshot.commit_seq += 1;
    prepared.sync(&snapshot);
    assert_eq!(prepared.qr().source().data, first.as_slice());

    // A different network gets a different code.
    snapshot.commit_seq += 1;
    snapshot.setup.ap_ssid = text("pico-scoreboard-2");
    prepared.sync(&snapshot);
    assert_ne!(prepared.qr().source().data, first.as_slice());
}

// -- Toasts ------------------------------------------------------------------

fn toast_view(kind: ToastKind, body: &str, at: Millis) -> ToastView {
    ToastView {
        text: text(body),
        kind,
        updated_ms: at,
        sticky: false,
        pulse_ms: 0,
    }
}

#[test]
fn a_text_toast_owns_the_bottom_strip() {
    let mut snapshot = snapshot();
    snapshot.toast = toast_view(ToastKind::Text, "LOCKED", 1_000);
    let now = WallMs(1_100);
    assert!(toast::is_active(&snapshot.toast, now));

    let frame = render(|canvas| {
        screens::no_games(canvas, &snapshot, now);
    });
    assert!(frame.lit_in(51..127, 43..59) > 0, "the flash strip");
}

#[test]
fn a_toast_expires_on_the_wall_rail() {
    let toast = toast_view(ToastKind::Text, "LOCKED", 1_000);
    assert!(toast::is_active(&toast, WallMs(1_000)));
    assert!(toast::is_active(&toast, WallMs(2_499)));
    assert!(!toast::is_active(&toast, WallMs(2_500)));

    let mut sticky = toast.clone();
    sticky.sticky = true;
    assert!(
        toast::is_active(&sticky, WallMs(2_500)),
        "a sticky toast outlives the display window"
    );
    assert!(
        !toast::is_active(&sticky, WallMs(21_001)),
        "but not the belt against a stranded one"
    );
}

#[test]
fn an_empty_text_toast_is_not_a_toast() {
    let toast = toast_view(ToastKind::Text, "", 1_000);
    assert!(!toast::is_active(&toast, WallMs(1_100)));

    // An icon toast has no text to be empty.
    let icon = toast_view(ToastKind::Lock, "", 1_000);
    assert!(toast::is_active(&icon, WallMs(1_100)));
}

#[test]
fn an_icon_toast_dims_the_frame_and_draws_its_icon() {
    let mut snapshot = snapshot();
    let plain = render(|canvas| screens::idle(canvas, &snapshot));

    snapshot.toast = toast_view(ToastKind::Lock, "", 1_000);
    // 200 ms in: past the four fade-in steps, so the frame is at the held level.
    let now = WallMs(1_200);
    let dimmed = render(|canvas| {
        screens::idle(canvas, &snapshot);
        toast::overlay(canvas, &snapshot, now);
    });

    let title = (0..WIDTH as i32)
        .flat_map(|x| (0..32).map(move |y| (x, y)))
        .find(|(x, y)| plain.lit(*x, *y))
        .expect("the idle title drew something");
    assert_eq!(
        dimmed.pixel(title.0, title.1),
        plain.pixel(title.0, title.1) >> 1 & 0x7BEF,
        "the held ladder level is half brightness"
    );
    assert!(
        dimmed.lit_in(50..78, 19..45) > 0,
        "the padlock draws over the middle of the panel"
    );
}

#[test]
fn the_overlay_fades_out_after_the_toast_expires() {
    let toast = toast_view(ToastKind::Lock, "", 1_000);
    assert!(!toast::overlay_fading(&toast, WallMs(2_499)), "still up");
    assert!(
        toast::overlay_fading(&toast, WallMs(2_500)),
        "first tail step"
    );
    assert!(
        toast::overlay_fading(&toast, WallMs(2_649)),
        "last tail step"
    );
    assert!(!toast::overlay_fading(&toast, WallMs(2_650)), "clean again");

    // A text toast has no dim to fade.
    let mut text_toast = toast.clone();
    text_toast.kind = ToastKind::Text;
    assert!(!toast::overlay_fading(&text_toast, WallMs(2_500)));
}

/// Where the fully-bright head dot sits, as an average of its pixels.
fn spinner_head(frame: &Frame) -> (i32, i32) {
    let pixels: Vec<(i32, i32)> = (SPINNER_BOX.1..SPINNER_BOX.3)
        .flat_map(|y| (SPINNER_BOX.0..SPINNER_BOX.2).map(move |x| (x, y)))
        .filter(|(x, y)| frame.pixel(*x, *y) == 0xFFFF)
        .collect();
    assert!(!pixels.is_empty(), "no dot at full brightness");
    let count = pixels.len() as i32;
    (
        pixels.iter().map(|(x, _)| x).sum::<i32>() / count,
        pixels.iter().map(|(_, y)| y).sum::<i32>() / count,
    )
}

/// The spinner sprite's footprint, centered on the panel.
const SPINNER_BOX: (i32, i32, i32, i32) = (52, 20, 77, 45);

#[test]
fn the_spinner_head_walks_the_ring_in_angular_order() {
    // This is the test for the palette-index inversion. `gen_toast_icons.py`
    // bakes dot k's color so its value is k + 1, but `compile_layout.py` assigns
    // palette indices in row-major first-seen order — which is not angular
    // order. Get the inversion wrong and the comet still draws twelve dots, it
    // just lights them in a scrambled sequence. So: half a revolution apart, the
    // head must be diametrically opposite.
    let mut snapshot = snapshot();
    snapshot.toast = toast_view(ToastKind::Spinner, "", 1_000);

    let start = render(|canvas| toast::overlay(canvas, &snapshot, WallMs(1_000)));
    let half = render(|canvas| toast::overlay(canvas, &snapshot, WallMs(1_500)));

    let (ax, ay) = spinner_head(&start);
    let (bx, by) = spinner_head(&half);
    let center = (
        (SPINNER_BOX.0 + SPINNER_BOX.2) / 2,
        (SPINNER_BOX.1 + SPINNER_BOX.3) / 2,
    );
    assert!(
        (ax + bx - 2 * center.0).abs() <= 1 && (ay + by - 2 * center.1).abs() <= 1,
        "head at ({ax}, {ay}) and ({bx}, {by}) are not opposite about {center:?}"
    );
}

#[test]
fn the_spinner_leaves_a_gap_behind_its_tail() {
    // The trail covers 10 of 12 dots; the other two get the key as their color,
    // so the blit skips them and whatever is underneath shows through.
    let mut snapshot = snapshot();
    snapshot.toast = toast_view(ToastKind::Spinner, "", 1_000);
    let frame = render(|canvas| {
        canvas.fill(0xFFFF);
        toast::overlay(canvas, &snapshot, WallMs(1_000));
    });

    // The dimmed background is what an undrawn dot leaves behind. Every dot the
    // comet does light is brighter than it.
    let background = frame.pixel(0, 0);
    let dots: Vec<u16> = (SPINNER_BOX.1..SPINNER_BOX.3)
        .flat_map(|y| (SPINNER_BOX.0..SPINNER_BOX.2).map(move |x| (x, y)))
        .map(|(x, y)| frame.pixel(x, y))
        .filter(|color| *color != background)
        .collect();
    assert!(!dots.is_empty(), "the comet drew nothing");
    assert!(dots.contains(&0xFFFF), "the head is at full brightness");
    assert!(
        dots.iter().any(|color| *color != 0xFFFF),
        "the tail fades behind the head"
    );
}

// -- Menu --------------------------------------------------------------------

fn menu_snapshot(highlight: i8) -> ScoreboardSnapshot {
    let mut snapshot = snapshot();
    let mut view = MenuView::new();
    view.active = true;
    view.updated_ms = 0;
    for (index, label) in ["MLB", "NBA", "PREMIER LEAGUE"].iter().enumerate() {
        let mut row = MenuRow::new();
        row.label = text(label);
        row.checked = index != 1;
        row.source = index as u8;
        view.rows.push(row).ok();
    }
    view.highlight = highlight;
    snapshot.menu = view;
    snapshot
}

#[test]
fn the_menu_highlight_never_inverts_a_checkbox() {
    // The rule the geometry exists to enforce: the bar starts after the
    // checkbox, so a checked row reads the same highlighted or not.
    let highlighted = render(|canvas| menu::render(canvas, &menu_snapshot(0), WallMs(0)));
    let plain = render(|canvas| menu::render(canvas, &menu_snapshot(1), WallMs(0)));

    let primary = pack(PRIMARY);
    for y in 2..9 {
        assert_eq!(
            highlighted.pixel(2, y),
            plain.pixel(2, y),
            "checkbox at y={y}"
        );
    }
    assert_eq!(highlighted.pixel(2, 2), primary, "checkbox border");
    assert_eq!(highlighted.pixel(11, 5), primary, "highlight bar");
    assert_eq!(plain.pixel(11, 5), BLACK, "no bar on an unhighlighted row");
}

#[test]
fn a_checked_row_fills_its_box_and_an_unchecked_one_does_not() {
    let frame = render(|canvas| menu::render(canvas, &menu_snapshot(-1), WallMs(0)));
    let primary = pack(PRIMARY);
    // Row 0 is checked, row 1 is not.
    assert_eq!(frame.pixel(4, 4), primary, "row 0's fill");
    assert_eq!(frame.pixel(4, 14), BLACK, "row 1 is unchecked");
}

#[test]
fn the_done_footer_inverts_when_it_is_the_cursor() {
    let selected = render(|canvas| menu::render(canvas, &menu_snapshot(-1), WallMs(0)));
    let unselected = render(|canvas| menu::render(canvas, &menu_snapshot(0), WallMs(0)));
    assert_eq!(
        selected.pixel(1, 56),
        pack(PRIMARY),
        "the footer band fills"
    );
    assert_eq!(unselected.pixel(1, 56), BLACK);
    // Row 63 stays dark either way: the panel's edge pixels are unreliable.
    assert_eq!(selected.lit_in(0..WIDTH as i32, 63..64), 0);
}

#[test]
fn a_short_list_draws_no_scrollbar() {
    let frame = render(|canvas| menu::render(canvas, &menu_snapshot(0), WallMs(0)));
    assert_eq!(
        frame.lit_in(125..127, 1..51),
        0,
        "three items fit, so there is no track"
    );

    let mut snapshot = menu_snapshot(0);
    snapshot.menu.thumb_y = 1;
    snapshot.menu.thumb_h = 12;
    let scrolled = render(|canvas| menu::render(canvas, &snapshot, WallMs(0)));
    assert!(scrolled.lit_in(125..127, 1..51) > 0, "the track and thumb");
    assert_eq!(scrolled.pixel(125, 1), pack(PRIMARY), "the thumb");
    assert_eq!(
        scrolled.pixel(125, 40),
        scoreboard_render::DIM_GRAY,
        "track"
    );
}

//! Pixel parity against the shipping MicroPython firmware.
//!
//! Phase 2's acceptance test. Every committed wire fixture is pushed through
//! *both* stacks and the two RGB565 frames are compared byte for byte:
//!
//! ```text
//! backend/testdata/wire/**.bin
//!   ├─ scoreboard-wire → scoreboard-model Store → scoreboard-render → hub75 sim
//!   └─ scoreboard/{mlb,nba,football,soccer}.py → state.py → display.render_frame
//! ```
//!
//! The MicroPython side is not re-run here — `tests/gen_parity.py` runs it under
//! `tools/preview`'s shims and commits the frames it produced. That script's
//! docstring is the normative description of what is pinned; this file consumes
//! the manifest it emits, so neither side can drift without the other noticing.
//!
//! # Reading a failure
//!
//! A mismatch writes `expected | actual | diff` panels to
//! `target/parity-diffs/<case>__t<ms>.png` and reports the differing-pixel
//! count and their bounding box. The MicroPython output is the baseline: work
//! out which side is right against `display.py` before touching the renderer.
//!
//! One diff class is known and accepted rather than chased — the team-color
//! brightening split on [`Rgb888::brightened`]. [`classify`] recognises it by
//! shape alone, never by fixture name, so a genuine bug cannot hide inside it.
//! It does not occur anywhere in the current corpus; `firmware-rs/PARITY.md`
//! has the verdict table and why.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use hub75::display::{FrameBytes, Hub75Display};
use hub75::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};
use hub75::sim::SimulatorSink;
use scoreboard_model::snapshot::LogoRef;
use scoreboard_model::{
    GameFeed, LeagueId, LocalClock, Logos as CommitLogos, MenuRowInput, Millis, Mode, Rgb888,
    SetupReason, Sport, Store, ToastKind, UiColors, WireFeed,
};
use scoreboard_render::blit::Canvas;
use scoreboard_render::frame;
use scoreboard_render::game::{LOGO_BYTES, LogoSlot, Logos, Scene};
use scoreboard_render::geometry::RenderSettings;
use scoreboard_render::prepared::PreparedView;
use scoreboard_render::time::{FrameElapsed, WallMs};

mod png;

const PIXELS: usize = WIDTH * HEIGHT;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    crate_dir().join("..").join("..")
}

// -- The manifest -------------------------------------------------------------

/// One fixture, as `gen_parity.py` recorded it.
struct GameCase {
    name: String,
    sport: Sport,
    slug: String,
    fixture: String,
    away_slot: u8,
    home_slot: u8,
}

/// Everything both stacks were pinned to. Read rather than restated: the
/// palette, the layout variants and the scroll speed are the firmware's own
/// defaults, and a change to any of them moves both sides together.
struct Manifest {
    commit_ms: Millis,
    now_epoch_s: u32,
    utc_offset_s: i32,
    time_points: Vec<Millis>,
    colors: UiColors,
    settings: RenderSettings,
    logo_slots: usize,
    games: Vec<GameCase>,
    /// Hand-published screens: the name [`publish_screen`] dispatches on, and
    /// the fixture published underneath it (`None` for a bare screen).
    screens: Vec<(String, Option<String>)>,
}

fn sport_from(name: &str) -> Sport {
    match name {
        "mlb" => Sport::Mlb,
        "nba" => Sport::Nba,
        "football" => Sport::Football,
        "soccer" => Sport::Soccer,
        other => panic!("manifest names an unknown sport: {other}"),
    }
}

fn parse_manifest(text: &str) -> Manifest {
    let mut commit_ms = None;
    let mut now_epoch_s = None;
    let mut utc_offset_s = None;
    let mut time_points = Vec::new();
    let mut colors = UiColors::new();
    let mut settings = RenderSettings::new();
    let mut logo_slots = 0usize;
    let mut games = Vec::new();
    let mut screens = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut field = line.split_whitespace();
        let key = field.next().expect("non-empty line has a key");
        let mut next = || {
            field
                .next()
                .unwrap_or_else(|| panic!("short record: {line}"))
        };
        match key {
            "version" => assert_eq!(next(), "1", "unsupported manifest version"),
            "commit_ms" => commit_ms = Some(next().parse().expect("commit_ms")),
            "now_epoch_s" => now_epoch_s = Some(next().parse().expect("now_epoch_s")),
            "utc_offset_s" => utc_offset_s = Some(next().parse().expect("utc_offset_s")),
            "time_points" => {
                time_points = field.map(|t| t.parse().expect("time point")).collect();
            }
            "color" => {
                let which = next().to_string();
                let red = next().parse().expect("red");
                let green = next().parse().expect("green");
                let blue = next().parse().expect("blue");
                let value = Rgb888::new(red, green, blue);
                match which.as_str() {
                    "primary" => colors.primary = value,
                    "secondary" => colors.secondary = value,
                    "accent" => colors.accent = value,
                    "clock_normal" => colors.clock_normal = value,
                    "clock_warning" => colors.clock_warning = value,
                    other => panic!("manifest names an unknown color: {other}"),
                }
            }
            "variant" => {
                let (key, letter) = (next().to_string(), next().to_string());
                assert!(
                    settings.apply_variant(&key, &letter),
                    "the Rust settings do not know the variant {key}={letter} \
                     that screen_geometry selects"
                );
            }
            "show_dividers" => settings.show_dividers = next() == "1",
            "scroll_px_per_sec" => {
                let requested: i32 = next().parse().expect("scroll speed");
                assert_eq!(
                    settings.set_scroll_speed(requested),
                    requested,
                    "the Rust scroll ladder rejected the speed screen_geometry runs at"
                );
            }
            "logo_slots" => logo_slots = next().parse().expect("logo_slots"),
            "game" => games.push(GameCase {
                name: next().to_string(),
                sport: sport_from(next()),
                slug: next().to_string(),
                fixture: next().to_string(),
                away_slot: next().parse().expect("away slot"),
                home_slot: next().parse().expect("home slot"),
            }),
            "screen" => {
                let name = next().to_string();
                let base = next();
                screens.push((name, (base != "-").then(|| base.to_string())));
            }
            other => panic!("unknown manifest record: {other}"),
        }
    }

    assert!(!games.is_empty(), "the manifest lists no fixtures");
    assert!(!time_points.is_empty(), "the manifest lists no time points");
    Manifest {
        commit_ms: commit_ms.expect("manifest carries commit_ms"),
        now_epoch_s: now_epoch_s.expect("manifest carries now_epoch_s"),
        utc_offset_s: utc_offset_s.expect("manifest carries utc_offset_s"),
        time_points,
        colors,
        settings,
        logo_slots,
        games,
        screens,
    }
}

// -- Rendering ----------------------------------------------------------------

/// The crest pool, split back out of the flat blob the generator dumped.
fn load_logo_pool(path: &Path, expected: usize) -> Vec<LogoSlot> {
    let blob = std::fs::read(path).expect("the crest pool is committed beside the manifest");
    assert_eq!(
        blob.len(),
        expected * LOGO_BYTES,
        "crest pool is {} bytes, expected {expected} slots of {LOGO_BYTES}",
        blob.len()
    );
    blob.chunks_exact(LOGO_BYTES)
        .map(|chunk| {
            let mut slot: LogoSlot = [0; LOGO_BYTES];
            slot.copy_from_slice(chunk);
            slot
        })
        .collect()
}

/// Read one fixture's payload off disk.
fn payload_of(case: &GameCase) -> Vec<u8> {
    std::fs::read(repo_root().join(&case.fixture))
        .unwrap_or_else(|error| panic!("{}: {error}", case.fixture))
}

/// Publish one fixture into `store`, through the same two calls the poller
/// makes: decode, then commit.
fn commit_case(store: &mut Store, manifest: &Manifest, case: &GameCase) {
    let payload = payload_of(case);
    let league = LeagueId::from_slug(case.sport, &case.slug);
    let detail = WireFeed
        .detail(case.sport, &payload)
        .unwrap_or_else(|error| panic!("{}: wire decode failed: {error:?}", case.name));

    store.commit_detail(
        &league,
        &detail,
        CommitLogos {
            away: Some(LogoRef(case.away_slot)),
            home: Some(LogoRef(case.home_slot)),
        },
        manifest.commit_ms,
        LocalClock {
            now_epoch_s: manifest.now_epoch_s,
            utc_offset_s: Some(manifest.utc_offset_s),
        },
    );
}

/// The league menu's published window, mirroring `gen_parity.py`'s constants —
/// including the over-long highlighted label that forces the marquee to move.
const MENU_LABELS: [&str; 5] = ["MLB", "NBA", "NFL", "ENG.CHAMPIONSHIP", "LIGA MX"];
const MENU_CHECKED: [bool; 5] = [true, true, false, true, false];
const MENU_HIGHLIGHT: i8 = 3;
const MENU_THUMB_Y: i8 = 1;
const MENU_THUMB_H: u8 = 25;

/// Publish the screen `name` stands for, with the arguments `gen_parity.py`'s
/// `static_screens` used.
///
/// The two tables are the parity subject for screens no wire payload reaches,
/// so their literals must agree — which is why the manifest names every case
/// and this function panics on one it does not know, rather than skipping it.
fn publish_screen(store: &mut Store, name: &str, now_ms: Millis) {
    match name {
        "idle" => store.set_mode(Mode::Idle),
        "no_games" => store.set_mode(Mode::NoGames),
        "startup" => store.set_startup_step(2, 5, "Connecting WiFi", "HOME-NET-5G", 2, 4),
        "error" => store.set_error("NO WIFI", &["Check credentials", "in the web UI"]),
        "updating_progress" => store.set_updating_progress(42, "1.4.2"),
        "updating_countdown" => store.set_updating_countdown(3),
        "setup_no_config" => store.set_setup_mode(SetupReason::NoConfig, "", "", ""),
        "setup_bad_auth" => store.set_setup_mode(SetupReason::BadAuth, "", "", "HOME-NET-5G"),
        // The overlays draw over whatever the manifest names as their base,
        // which the caller has already committed.
        "toast_text" => store.set_toast("ROTATION LOCKED", ToastKind::Text, false, now_ms),
        "toast_lock" => store.set_toast("", ToastKind::Lock, true, now_ms),
        "toast_spinner" => store.set_toast("", ToastKind::Spinner, true, now_ms),
        "menu" => {
            store.set_mode(Mode::Idle);
            let rows: Vec<MenuRowInput<'_>> = MENU_LABELS
                .iter()
                .zip(MENU_CHECKED)
                .enumerate()
                .map(|(index, (label, checked))| MenuRowInput {
                    label,
                    checked,
                    source: index as u8,
                })
                .collect();
            store.set_menu(&rows, MENU_HIGHLIGHT, MENU_THUMB_Y, MENU_THUMB_H, now_ms);
        }
        other => panic!(
            "the manifest names the static screen {other:?}, which this test \
             does not know how to publish — add it beside gen_parity.py's entry"
        ),
    }
}

/// One frame, `offset` ms after the commit.
///
/// Both frame rails carry `offset` and the wall clock carries
/// `commit_ms + offset` — the values `FrameRail::advance_and_latch` produces
/// under ideal pacing after a commit that changed the view identity, and
/// exactly what the generator fed `render_frame`.
fn render_frame(
    manifest: &Manifest,
    store: &Store,
    prepared: &PreparedView,
    pool: &[LogoSlot],
    offset: Millis,
) -> Vec<u8> {
    let scene = Scene {
        snapshot: store.snapshot(),
        prepared,
        settings: &manifest.settings,
        logos: Logos::new(pool),
        now: WallMs(manifest.commit_ms + offset),
        view: FrameElapsed(offset),
        play: FrameElapsed(offset),
    };
    let mut buffer: Box<FrameBytes> = Box::new([0; RGB565_FRAME_BYTES]);
    let mut display = Hub75Display::new(&mut buffer, SimulatorSink::new());
    {
        let mut canvas = Canvas::new(display.buffer_mut(), WIDTH as i32, HEIGHT as i32);
        frame::render(&mut canvas, &scene);
    }
    display.show();
    display.sink_mut().front().to_vec()
}

// -- Comparison ---------------------------------------------------------------

fn pixel(frame: &[u8], index: usize) -> u16 {
    u16::from_le_bytes([frame[index * 2], frame[index * 2 + 1]])
}

/// What the two frames disagree about.
struct Diff {
    /// Indices of every differing pixel.
    indices: Vec<usize>,
    /// `(x0, y0, x1, y1)`, inclusive.
    bounds: (usize, usize, usize, usize),
    /// Each distinct `(micropython, rust)` pair and how often it occurs.
    pairs: BTreeMap<(u16, u16), usize>,
}

fn compare(expected: &[u8], actual: &[u8]) -> Option<Diff> {
    let mut indices = Vec::new();
    let mut pairs: BTreeMap<(u16, u16), usize> = BTreeMap::new();
    for index in 0..PIXELS {
        let (want, got) = (pixel(expected, index), pixel(actual, index));
        if want != got {
            indices.push(index);
            *pairs.entry((want, got)).or_default() += 1;
        }
    }
    if indices.is_empty() {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (WIDTH, HEIGHT, 0, 0);
    for &index in &indices {
        let (x, y) = (index % WIDTH, index / WIDTH);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Some(Diff {
        indices,
        bounds: (x0, y0, x1, y1),
        pairs,
    })
}

/// The channel codes an RGB565 word carries: 5 bits red, 6 green, 5 blue.
fn channels(value: u16) -> (i32, i32, i32) {
    (
        ((value >> 11) & 0x1F) as i32,
        ((value >> 5) & 0x3F) as i32,
        (value & 0x1F) as i32,
    )
}

/// How a case's diff is accounted for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Match,
    /// Every differing pixel is one channel code apart. That is the signature
    /// of the team-color brightening split documented on
    /// [`Rgb888::brightened`]: `state.py` computed the scale in floating point
    /// and truncated `127.999…`, the port computes `channel * 128 / max` in
    /// integers, and the two land one unit apart exactly on the channels where
    /// that product is integral. The port is the correct arm — the float form
    /// undershoots the `_TEAM_COLOR_MIN_CHANNEL` floor it exists to enforce.
    Brightening,
    Fail,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Match => "MATCH",
            Verdict::Brightening => "ACCEPTED-DIFF (brightening)",
            Verdict::Fail => "FAIL",
        }
    }
}

/// Decide what a diff is, from its pixels alone.
///
/// The brightening class is recognised structurally — every differing pixel is
/// within one channel code on all three channels — so no fixture is exempted
/// by name and a genuine bug cannot ride along inside an accepted one.
fn classify(diff: &Diff) -> Verdict {
    let one_unit = diff.pairs.keys().all(|&(want, got)| {
        let (wr, wg, wb) = channels(want);
        let (gr, gg, gb) = channels(got);
        (wr - gr).abs() <= 1 && (wg - gg).abs() <= 1 && (wb - gb).abs() <= 1
    });
    if one_unit {
        Verdict::Brightening
    } else {
        Verdict::Fail
    }
}

// -- Diff artifacts -----------------------------------------------------------

const DIFF_SCALE: usize = 4;
const GUTTER: usize = 4;

/// `expected | actual | diff` panels, upscaled, into one PNG.
///
/// The diff panel paints matching pixels dark and differing ones red, so where
/// the disagreement is reads at a glance before any pixel values are inspected.
fn write_panels(path: &Path, expected: &[u8], actual: &[u8], diff: &Diff) {
    let panel = WIDTH * DIFF_SCALE;
    let width = panel * 3 + GUTTER * 2;
    let height = HEIGHT * DIFF_SCALE;
    let mut rgb = vec![0u8; width * height * 3];

    let mut differing = vec![false; PIXELS];
    for &index in &diff.indices {
        differing[index] = true;
    }

    for y in 0..height {
        let source_y = y / DIFF_SCALE;
        for x in 0..width {
            let (panel_index, panel_x) = if x < panel {
                (0, x)
            } else if x < panel + GUTTER {
                (usize::MAX, 0)
            } else if x < panel * 2 + GUTTER {
                (1, x - panel - GUTTER)
            } else if x < panel * 2 + GUTTER * 2 {
                (usize::MAX, 0)
            } else {
                (2, x - panel * 2 - GUTTER * 2)
            };
            let out = (y * width + x) * 3;
            if panel_index == usize::MAX {
                rgb[out..out + 3].copy_from_slice(&[0x40, 0x40, 0x40]);
                continue;
            }
            let index = source_y * WIDTH + panel_x / DIFF_SCALE;
            let color = match panel_index {
                0 => expand(pixel(expected, index)),
                1 => expand(pixel(actual, index)),
                _ if differing[index] => [0xFF, 0x30, 0x30],
                _ => [0x10, 0x10, 0x10],
            };
            rgb[out..out + 3].copy_from_slice(&color);
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    png::write_rgb(path, width, height, &rgb).expect("diff PNG written");
}

/// RGB565 to RGB888 by bit replication — the expansion the panel's own drive
/// performs, and injective, so nothing about the frame is lost in the artifact.
fn expand(value: u16) -> [u8; 3] {
    let (red, green, blue) = channels(value);
    [
        ((red << 3) | (red >> 2)) as u8,
        ((green << 2) | (green >> 4)) as u8,
        ((blue << 3) | (blue >> 2)) as u8,
    ]
}

// -- The tests ----------------------------------------------------------------

/// The brightening class does not occur anywhere in the committed corpus (see
/// PARITY.md), so without this its recogniser would be code nobody ever runs —
/// and a broken guard that never fires reads exactly like a passing one.
#[test]
fn the_accepted_diff_class_is_recognised_by_shape_and_nothing_else_is() {
    let mut expected = vec![0u8; RGB565_FRAME_BYTES];
    let mut actual = expected.clone();

    // The real thing: `Rgb888::brightened`'s float and integer forms on a
    // primary whose brightest channel is 98 — one of only four maxima where
    // they disagree at all. Float truncates 127.999… to 127, the port keeps
    // 128, and RGB565 carries the gap as one code on each channel.
    let float_form = scoreboard_render::rgb565(127, 63, 0);
    let integer_form = scoreboard_render::rgb565(128, 64, 0);
    for index in [0, 1, 200, 4000] {
        expected[index * 2..index * 2 + 2].copy_from_slice(&float_form.to_le_bytes());
        actual[index * 2..index * 2 + 2].copy_from_slice(&integer_form.to_le_bytes());
    }
    let diff = compare(&expected, &actual).expect("the frames differ");
    assert_eq!(diff.indices.len(), 4);
    assert!(matches!(classify(&diff), Verdict::Brightening));

    // One pixel two codes apart is enough to disqualify the whole frame: the
    // class is "every difference is one unit", not "most of them are".
    let index = 4000;
    actual[index * 2..index * 2 + 2]
        .copy_from_slice(&scoreboard_render::rgb565(255, 64, 0).to_le_bytes());
    let diff = compare(&expected, &actual).expect("the frames differ");
    assert!(matches!(classify(&diff), Verdict::Fail));

    // And a frame that agrees everywhere has no diff to classify.
    assert!(compare(&expected, &expected).is_none());
}

// -- The parity test ----------------------------------------------------------

#[test]
fn every_wire_fixture_renders_the_pixels_the_micropython_firmware_renders() {
    let parity = crate_dir().join("tests").join("parity");
    let manifest_path = parity.join("manifest.txt");
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}\nRegenerate with `py crates/scoreboard-render/tests/gen_parity.py`",
            manifest_path.display()
        )
    });
    let manifest = parse_manifest(&text);
    let pool = load_logo_pool(&parity.join("logos.rgb565"), manifest.logo_slots);
    let diff_dir = repo_root().join("target").join("parity-diffs");

    let mut report = String::new();
    let mut failures = Vec::new();
    let mut counts = [0usize; 3];

    // Every case as `(name, published state)`: the wire fixtures first, then
    // the hand-published screens, so both walk the same comparison.
    let mut published: Vec<(&str, Store)> = Vec::new();
    for case in &manifest.games {
        let mut store = Store::new();
        store.set_ui_colors(manifest.colors);
        commit_case(&mut store, &manifest, case);
        published.push((&case.name, store));
    }
    for (name, base) in &manifest.screens {
        let mut store = Store::new();
        store.set_ui_colors(manifest.colors);
        if let Some(base) = base {
            let case = manifest
                .games
                .iter()
                .find(|case| &case.name == base)
                .unwrap_or_else(|| panic!("{name}: base fixture {base:?} is not in the corpus"));
            commit_case(&mut store, &manifest, case);
        }
        publish_screen(&mut store, name, manifest.commit_ms);
        published.push((name, store));
    }

    // How many cases actually look different at different time points. Printed
    // rather than asserted case by case, because which screens animate is a
    // property of the fixtures; a corpus where *nothing* moved would mean the
    // time pinning had collapsed, and every scroll, cycle and pulse in the port
    // was going unchecked while the run still read as green.
    let mut animated = 0usize;

    for (name, store) in &published {
        let mut prepared = PreparedView::new();
        prepared.sync(store.snapshot(), &manifest.settings);
        let mut rendered_first: Option<Vec<u8>> = None;
        let mut varies = false;

        for &offset in &manifest.time_points {
            let label = format!("{name}__t{offset}");
            let golden = parity.join("frames").join(format!("{label}.bin"));
            let expected = std::fs::read(&golden).unwrap_or_else(|error| {
                panic!(
                    "{}: {error}\nRegenerate with \
                     `py crates/scoreboard-render/tests/gen_parity.py`",
                    golden.display()
                )
            });
            assert_eq!(
                expected.len(),
                RGB565_FRAME_BYTES,
                "{label}: golden is not a frame"
            );

            let actual = render_frame(&manifest, store, &prepared, &pool, offset);
            match &rendered_first {
                None => rendered_first = Some(actual.clone()),
                Some(first) => varies |= *first != actual,
            }
            let Some(diff) = compare(&expected, &actual) else {
                counts[0] += 1;
                let _ = writeln!(report, "  {:<52} {}", label, Verdict::Match.label());
                continue;
            };

            let verdict = classify(&diff);
            let (x0, y0, x1, y1) = diff.bounds;
            let artifact = diff_dir.join(format!("{label}.png"));
            write_panels(&artifact, &expected, &actual, &diff);
            let _ = writeln!(
                report,
                "  {:<52} {:<28} {} px, box ({x0},{y0})-({x1},{y1}), {} value pair(s)",
                label,
                verdict.label(),
                diff.indices.len(),
                diff.pairs.len()
            );
            match verdict {
                Verdict::Match => unreachable!("compare() returned a diff"),
                Verdict::Brightening => counts[1] += 1,
                Verdict::Fail => {
                    counts[2] += 1;
                    let pairs = diff
                        .pairs
                        .iter()
                        .take(8)
                        .map(|(&(want, got), n)| format!("{want:#06x}->{got:#06x} x{n}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    failures.push(format!(
                        "{label}: {} px differ, box ({x0},{y0})-({x1},{y1})\n    \
                         micropython->rust: {pairs}\n    artifact: {}",
                        diff.indices.len(),
                        artifact.display()
                    ));
                }
            }
        }
        animated += usize::from(varies);
    }

    println!(
        "\nparity over {} wire fixtures + {} static screens x {} time points\n{report}\n\
         {} MATCH, {} ACCEPTED-DIFF, {} FAIL \
         ({animated} of {} cases move between time points)\n",
        manifest.games.len(),
        manifest.screens.len(),
        manifest.time_points.len(),
        counts[0],
        counts[1],
        counts[2],
        published.len(),
    );
    assert!(
        animated > 0,
        "no case looked different at any two time points — the time pinning has \
         collapsed and every scroll, cycle and pulse is going unchecked"
    );
    assert!(
        failures.is_empty(),
        "\n{} frame(s) differ beyond the accepted classes:\n\n{}\n\n\
         The MicroPython frame is the baseline. Diagnose which side is right \
         against display.py before changing the renderer.\n",
        failures.len(),
        failures.join("\n\n")
    );
}

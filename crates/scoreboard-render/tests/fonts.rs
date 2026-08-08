//! Glyph-table parity against the Python generator, and the text placement
//! arithmetic on top of it.

mod goldens;

use goldens::{FONTS, FontGolden, fnv1a64};
use scoreboard_render::blit::Canvas;
use scoreboard_render::font::{self, Align, FontFace, Scroll, Style};
use scoreboard_render::generated::{SPLEEN_5X8, UNSCII_8, UNSCII_16};
use scoreboard_render::time::Motion;

fn faces() -> [(&'static FontFace, &'static FontGolden); 3] {
    let by_name = |name: &str| FONTS.iter().find(|font| font.name == name).unwrap();
    [
        (&UNSCII_8, by_name("UNSCII_8")),
        (&UNSCII_16, by_name("UNSCII_16")),
        (&SPLEEN_5X8, by_name("SPLEEN_5X8")),
    ]
}

#[test]
fn every_glyph_matches_the_python_generator() {
    // The digest covers all 224 table entries resolved exactly as a consumer
    // resolves them, so shared records, Latin-1 stand-ins and absent-glyph
    // fallbacks are all inside it — not just the spot checks below.
    for (face, golden) in faces() {
        assert_eq!(face.height(), golden.height, "{} height", golden.name);
        let mut digest = Vec::new();
        for codepoint in 32u32..=255 {
            let glyph = face.glyph(char::from_u32(codepoint).unwrap());
            digest.extend_from_slice(&(glyph.width as u16).to_le_bytes());
            digest.extend_from_slice(glyph.bits);
        }
        assert_eq!(
            fnv1a64(&digest),
            golden.glyphs_fnv,
            "{} glyph table digest",
            golden.name
        );
    }
}

#[test]
fn spot_glyphs_carry_the_generators_bits() {
    for (face, golden) in faces() {
        for spot in golden.glyphs {
            let glyph = face.glyph(char::from_u32(spot.codepoint).unwrap());
            assert_eq!(
                glyph.width, spot.width,
                "{} U+{:04X} width",
                golden.name, spot.codepoint
            );
            assert_eq!(
                glyph.bits, spot.bits,
                "{} U+{:04X} bits",
                golden.name, spot.codepoint
            );
        }
    }
}

#[test]
fn out_of_repertoire_codepoints_fall_back_to_the_default_glyph() {
    // Wire strings are folded into ASCII + Latin-1 at ingest, but an SSID is not
    // our text and can be anything.
    let fallback = UNSCII_8.glyph('?');
    for c in ['☃', '中', '\u{1F600}', '\u{0}'] {
        let glyph = UNSCII_8.glyph(c);
        assert_eq!(glyph.bits, fallback.bits, "{c:?} should render as '?'");
    }
}

#[test]
fn spleen_stands_latin1_in_while_unscii_draws_it() {
    // The compile-time remap: spleen ships blank bitmaps for most of Latin-1, so
    // its 'é' is 'e'''s record. unscii draws a real one.
    assert_eq!(SPLEEN_5X8.glyph('é').bits, SPLEEN_5X8.glyph('e').bits);
    assert_eq!(SPLEEN_5X8.glyph('ñ').bits, SPLEEN_5X8.glyph('n').bits);
    assert_ne!(UNSCII_8.glyph('é').bits, UNSCII_8.glyph('e').bits);
}

#[test]
fn measured_widths_match_the_geometry_tables_assumptions() {
    // Every one of these numbers is load-bearing in a geometry comment.
    assert_eq!(font::measure("PREMIER LEAGUE", &UNSCII_8), 112);
    assert_eq!(font::measure("WED JUL 16", &UNSCII_16), 80);
    assert_eq!(font::measure("FINAL", &UNSCII_8), 40);
    assert_eq!(font::measure("scheduled", &SPLEEN_5X8), 45);
    assert_eq!(font::measure("", &UNSCII_8), 0);
}

#[test]
fn the_fonts_are_fixed_width() {
    for (face, expected) in [(&UNSCII_8, 8), (&UNSCII_16, 8), (&SPLEEN_5X8, 5)] {
        for c in ['0', '9', 'W', 'i', '.', ' '] {
            assert_eq!(face.glyph(c).width, expected, "{c:?}");
        }
    }
}

#[test]
fn integers_render_as_their_digits() {
    let mut by_integer = vec![0u8; 64 * 16 * 2];
    let mut canvas = Canvas::new(&mut by_integer, 64, 16);
    let end = font::integer(
        &mut canvas,
        1234,
        0,
        0,
        64,
        Align::Right,
        Style::new(&UNSCII_16, 0xFFFF),
    );
    assert_eq!(end, 64, "right-aligned text ends at the box's right edge");

    let mut by_text = vec![0u8; 64 * 16 * 2];
    let mut canvas = Canvas::new(&mut by_text, 64, 16);
    font::aligned_text(
        &mut canvas,
        "1234",
        0,
        0,
        64,
        Align::Right,
        Style::new(&UNSCII_16, 0xFFFF),
    );
    assert_eq!(by_integer, by_text);
}

#[test]
fn zero_renders_as_one_digit() {
    let mut digits = vec![0u8; 32 * 16 * 2];
    let mut canvas = Canvas::new(&mut digits, 32, 16);
    let end = font::integer(
        &mut canvas,
        0,
        0,
        0,
        32,
        Align::Left,
        Style::new(&UNSCII_16, 0xFFFF),
    );
    assert_eq!(end, 8);
}

#[test]
fn alignment_floors_when_the_text_overflows_its_box() {
    // MicroPython's `//` floors; truncation would put an overflowing centered
    // line one pixel right of where the panel drew it. Three unscii_16 digits
    // (24 px) in a 22 px slot is the real case — soccer's score windows.
    let mut pixels = vec![0u8; 22 * 16 * 2];
    let mut canvas = Canvas::new(&mut pixels, 22, 16);
    let end = font::integer(
        &mut canvas,
        100,
        0,
        0,
        22,
        Align::Center,
        Style::new(&UNSCII_16, 0xFFFF),
    );
    assert_eq!(end, -1 + 24, "starts at floor((22 - 24) / 2) = -1");
}

#[test]
fn the_scroll_cycle_pauses_scrolls_and_pauses() {
    // A 100 px line in a 76 px window at 20 px/s: 24 px to travel, 1200 ms of
    // scrolling between two 1000 ms dwells.
    let scroll = Scroll {
        pause_ms: 1000,
        pixels_per_second: 20,
    };
    let at = |ms| font::scroll_offset(100, 76, Motion(ms), scroll);
    assert_eq!(at(0), 0);
    assert_eq!(at(999), 0);
    assert_eq!(at(1000), 0);
    assert_eq!(at(1500), 10);
    assert_eq!(at(2199), 23);
    assert_eq!(at(2200), 24, "held at the end for the closing dwell");
    assert_eq!(at(3199), 24);
    assert_eq!(at(3200), 0, "the cycle wraps");
}

#[test]
fn text_that_fits_never_scrolls() {
    let scroll = Scroll {
        pause_ms: 1000,
        pixels_per_second: 20,
    };
    for ms in [0, 5_000, 1_000_000] {
        assert_eq!(font::scroll_offset(40, 76, Motion(ms), scroll), 0);
    }
}

#[test]
fn every_legal_speed_advances_by_a_whole_number_of_pixels_per_frame() {
    // The divisibility constraint, exercised rather than asserted: walk one
    // scroll cycle a frame at a time and check the step never changes size. A
    // speed like 30 px/s alternates 1 px and 2 px here, which is the stutter the
    // legal set exists to prevent.
    const TRAVEL: i32 = 100;
    const PAUSE_MS: u64 = 1000;
    for speed in scoreboard_render::geometry::SCROLL_SPEEDS {
        let scroll = Scroll {
            pause_ms: PAUSE_MS,
            pixels_per_second: speed,
        };
        let first = PAUSE_MS / 50;
        let last = first + (TRAVEL as u64 * 1000 / speed as u64) / 50;
        let mut steps = Vec::new();
        let mut previous = 0;
        for frame in first..=last {
            let offset = font::scroll_offset(TRAVEL + 100, 100, Motion(frame * 50), scroll);
            if offset != previous {
                steps.push(offset - previous);
                previous = offset;
            }
        }
        assert_eq!(previous, TRAVEL, "{speed} px/s did not finish its travel");
        assert!(
            steps.windows(2).all(|pair| pair[0] == pair[1]),
            "{speed} px/s produced uneven steps: {steps:?}"
        );
    }
}

#[test]
fn a_scrolled_line_stays_inside_its_region() {
    let mut pixels = vec![0u8; 32 * 8 * 2];
    let mut canvas = Canvas::new(&mut pixels, 32, 8);
    let mut region = canvas.region(8, 0, 16, 8);
    font::draw(
        &mut region,
        "AAAAAAAAAAAAAAAAAAAA",
        Align::Left,
        Motion(2_000),
        Style::new(&UNSCII_8, 0xFFFF),
        Scroll::DEFAULT,
    );
    for y in 0..8 {
        for x in (0..8).chain(24..32) {
            assert_eq!(canvas.pixel_at(x, y), Some(0), "spilled at ({x}, {y})");
        }
    }
}

#[test]
fn a_transparent_draw_leaves_the_background_alone() {
    let mut pixels = vec![0u8; 16 * 8 * 2];
    let mut canvas = Canvas::new(&mut pixels, 16, 8);
    canvas.fill(0x1234);
    let mut region = canvas.region(0, 0, 16, 8);
    font::draw_unscrolled(&mut region, "I", Align::Left, Style::new(&UNSCII_8, 0xFFFF));
    assert!(
        (0..16).any(|x| (0..8).any(|y| canvas.pixel_at(x, y) == Some(0xFFFF))),
        "nothing was drawn"
    );
    assert!(
        (0..16).any(|x| (0..8).any(|y| canvas.pixel_at(x, y) == Some(0x1234))),
        "the background was painted over"
    );
}

#[test]
fn an_opaque_draw_paints_its_background() {
    let mut pixels = vec![0u8; 16 * 8 * 2];
    let mut canvas = Canvas::new(&mut pixels, 16, 8);
    canvas.fill(0x1234);
    let mut region = canvas.region(0, 0, 16, 8);
    font::draw(
        &mut region,
        "I",
        Align::Left,
        Motion(0),
        Style::new(&UNSCII_8, 0xFFFF).on(0),
        Scroll::DEFAULT,
    );
    assert!(
        (0..16).all(|x| (0..8).all(|y| canvas.pixel_at(x, y) != Some(0x1234))),
        "the region should have been cleared first"
    );
}

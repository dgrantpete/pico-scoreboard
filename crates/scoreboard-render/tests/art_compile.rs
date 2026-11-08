//! The hand-drawn art's compile step (BACKLOG 116): `art/*.png` becomes the
//! committed `src/generated/art.rs`, and this test is the compiler.
//!
//! Four sprites are drawn by hand rather than in code — the MLB diamond and
//! base marker, the football and the football field — and their PNGs are the
//! source of truth: open one in any editor, change it, save it as 8-bit RGB
//! or RGBA with alpha 0 or 255. The generated module is committed so that
//! building the firmware needs nothing but cargo, and so that this compiler
//! needs nothing but a test: no build script, no Python, no Aseprite.
//!
//! ```text
//! SCOREBOARD_BLESS_ART=1 cargo test -p scoreboard-render --test art_compile
//! ```
//!
//! rewrites the module from the PNGs; without the variable the test fails
//! whenever the committed module is not exactly what the PNGs compile to — the
//! golden frames' bless pattern, applied to a source file. The comparison
//! ignores CRLF, because `core.autocrlf` checks the module out with CRLF on
//! Windows while the compiler writes LF.
//!
//! # The packing
//!
//! Every sprite is 4 bpp, [`PixelFormat::Gs4Hmsb`] — leftmost pixel in a
//! byte's high nibble, rows byte-whole — through a palette whose index 0 is
//! `None`, see-through, and whose other entries are the PNG's colours,
//! exactly, in row-major first-seen order. One format for all four: each has
//! at most 15 colours, and a format chooser would save five bytes on the base
//! marker. The blitter decodes it, and [`every_generated_sprite_draws_its_png`]
//! proves the round trip pixel for pixel.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use scoreboard_model::Rgb888;
use scoreboard_render::blit::{BYTES_PER_PIXEL, Canvas, PixelFormat, Sprite};
use scoreboard_render::generated::art;

/// The PNGs under `art/`, by file stem — each one module in the output.
const SOURCES: [&str; 4] = ["base_marker", "field", "football_ball", "football_field"];

/// Set to rewrite the generated module instead of comparing against it.
const BLESS_VAR: &str = "SCOREBOARD_BLESS_ART";

/// The most colours a sprite can have: a 4-bit index, less the see-through 0.
const MAX_COLORS: usize = 15;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn generated_path() -> PathBuf {
    crate_dir().join("src").join("generated").join("art.rs")
}

/// One decoded PNG: `None` where it is transparent, else its exact colour.
struct Image {
    width: usize,
    height: usize,
    pixels: Vec<Option<Rgb888>>,
}

fn decode(path: &Path) -> Image {
    let file = File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder
        .read_info()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut buffer = vec![0; reader.output_buffer_size().expect("image fits in memory")];
    let frame = reader
        .next_frame(&mut buffer)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(
        frame.bit_depth,
        png::BitDepth::Eight,
        "{}: save it at 8 bits per channel",
        path.display()
    );
    let channels = match frame.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        other => panic!("{}: {other:?} is not RGB or RGBA", path.display()),
    };
    let pixels = buffer[..frame.buffer_size()]
        .chunks_exact(channels)
        .map(|pixel| {
            let alpha = if channels == 4 { pixel[3] } else { 255 };
            assert!(
                alpha == 0 || alpha == 255,
                "{}: alpha {alpha} — a pixel is lit or it is not; use 0 or 255",
                path.display()
            );
            (alpha == 255).then(|| Rgb888::new(pixel[0], pixel[1], pixel[2]))
        })
        .collect();
    Image {
        width: frame.width as usize,
        height: frame.height as usize,
        pixels,
    }
}

fn source(name: &str) -> Image {
    decode(&crate_dir().join("art").join(format!("{name}.png")))
}

/// The palette — `None`, then the colours in row-major first-seen order — and
/// the 4 bpp indices, high nibble first.
fn compile(name: &str, image: &Image) -> (Vec<Option<Rgb888>>, Vec<u8>) {
    let mut palette: Vec<Option<Rgb888>> = vec![None];
    let row_bytes = PixelFormat::Gs4Hmsb.row_bytes(image.width);
    let mut data = vec![0u8; row_bytes * image.height];
    for y in 0..image.height {
        for x in 0..image.width {
            let pixel = image.pixels[y * image.width + x];
            let index = match palette.iter().position(|&entry| entry == pixel) {
                Some(index) => index,
                None => {
                    palette.push(pixel);
                    palette.len() - 1
                }
            };
            let byte = &mut data[y * row_bytes + x / 2];
            *byte |= (index as u8) << if x % 2 == 0 { 4 } else { 0 };
        }
    }
    assert!(
        palette.len() <= MAX_COLORS + 1,
        "{name}.png: {} colours; a 4-bit sprite holds {MAX_COLORS}",
        palette.len() - 1
    );
    (palette, data)
}

const HEADER: &str = "\
// GENERATED by crates/scoreboard-render/tests/art_compile.rs from
// crates/scoreboard-render/art/*.png — do not edit.
//
// After changing a PNG, regenerate this file and commit the two together:
//
//     SCOREBOARD_BLESS_ART=1 cargo test -p scoreboard-render --test art_compile
//
//! The hand-drawn sprites, compiled from their PNGs (BACKLOG 116): the MLB
//! diamond and base marker, the football and the football field. Everything
//! else on the panel is drawn in code.
//!
//! Every sprite is 4 bpp ([`PixelFormat::Gs4Hmsb`]) through a palette whose
//! index 0 is `None`, see-through, and whose other entries are the PNG's
//! colours, exactly, in row-major first-seen order. Where a sprite lands is
//! not here: a PNG carries no position, so the code that draws it owns that
//! number.
//!
//! Palettes are `&'static` and immutable. A renderer that tints one — the
//! base markers, the endzones — copies it into a local array first.

use crate::Rgb888;
use crate::blit::{PixelFormat, Sprite};
";

fn module(name: &str, image: &Image) -> String {
    let (palette, data) = compile(name, image);
    let mut out = format!(
        "/// `art/{name}.png` — {} × {}, {} colours.\n\
         pub mod {name} {{\n\
         \x20   use super::*;\n\
         \n\
         \x20   pub const SPRITE: Sprite = Sprite {{\n\
         \x20       data: &DATA,\n\
         \x20       width: {},\n\
         \x20       height: {},\n\
         \x20       format: PixelFormat::Gs4Hmsb,\n\
         \x20       palette: Some(&PALETTE),\n\
         \x20   }};\n\
         \n\
         \x20   #[rustfmt::skip]\n\
         \x20   static DATA: [u8; {}] = [\n",
        image.width,
        image.height,
        palette.len() - 1,
        image.width,
        image.height,
        data.len(),
    );
    for chunk in data.chunks(16) {
        let bytes: Vec<String> = chunk.iter().map(|byte| format!("0x{byte:02x},")).collect();
        out += &format!("        {}\n", bytes.join(" "));
    }
    out += &format!(
        "    ];\n\
         \n\
         \x20   /// Index 0 is see-through; the rest are the PNG's colours.\n\
         \x20   #[rustfmt::skip]\n\
         \x20   pub static PALETTE: [Option<Rgb888>; {}] = [\n",
        palette.len()
    );
    let entries: Vec<String> = palette
        .iter()
        .map(|entry| match entry {
            None => "None,".to_string(),
            Some(c) => format!(
                "Some(Rgb888::new({}, {}, {})),",
                c.red(),
                c.green(),
                c.blue()
            ),
        })
        .collect();
    for line in entries.chunks(3) {
        out += &format!("        {}\n", line.join(" "));
    }
    out += "    ];\n}\n";
    out
}

/// The whole generated module, from the PNGs as they are now.
fn generate() -> String {
    let modules: Vec<String> = SOURCES
        .iter()
        .map(|name| module(name, &source(name)))
        .collect();
    format!("{HEADER}\n{}", modules.join("\n"))
}

#[test]
fn the_generated_module_is_what_the_pngs_compile_to() {
    let path = generated_path();
    let fresh = generate();
    if std::env::var_os(BLESS_VAR).is_some() {
        std::fs::write(&path, &fresh).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("\r\n", "\n");
    if committed == fresh {
        return;
    }
    let (line, ours, theirs) = fresh
        .lines()
        .zip(committed.lines())
        .enumerate()
        .find(|(_, (ours, theirs))| ours != theirs)
        .map(|(index, (ours, theirs))| (index + 1, ours, theirs))
        .unwrap_or((
            fresh.lines().count().min(committed.lines().count()) + 1,
            "",
            "",
        ));
    panic!(
        "{} is stale: the PNGs under art/ compile to something else.\n\
         first difference, line {line}:\n  committed: {theirs}\n  compiled:  {ours}\n\
         If the PNG change is intended, regenerate and commit both: {BLESS_VAR}=1 \
         cargo test -p scoreboard-render --test art_compile",
        path.display()
    );
}

/// Nothing under `art/` is left out of the compile, and nothing listed is
/// missing.
#[test]
fn the_art_directory_holds_exactly_the_listed_sources() {
    let mut found: Vec<String> = std::fs::read_dir(crate_dir().join("art"))
        .expect("art/ exists")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    let listed: Vec<String> = SOURCES.iter().map(|name| format!("{name}.png")).collect();
    assert_eq!(found, listed);
}

/// The compiled sprites, by source name: what the round trip draws.
const COMPILED: [(&str, Sprite); 4] = [
    ("base_marker", art::base_marker::SPRITE),
    ("field", art::field::SPRITE),
    ("football_ball", art::football_ball::SPRITE),
    ("football_field", art::football_field::SPRITE),
];

/// A colour no PNG here uses, under every round-trip draw.
const SENTINEL: Rgb888 = Rgb888::new(1, 2, 3);

/// Each compiled sprite, blitted through the crate's own decoder onto a
/// sentinel-filled canvas, is its PNG bit for bit, transparent pixels left as
/// they were — the packing and the blitter agree.
#[test]
fn every_generated_sprite_draws_its_png() {
    assert_eq!(COMPILED.map(|(name, _)| name), SOURCES);
    for (name, sprite) in COMPILED {
        let image = source(name);
        assert_eq!(
            (sprite.width, sprite.height),
            (image.width as i32, image.height as i32),
            "{name}: size"
        );
        let sentinel = [SENTINEL.red(), SENTINEL.green(), SENTINEL.blue()];
        let mut pixels: Vec<u8> = sentinel.repeat(image.width * image.height);
        let mut canvas = Canvas::new(&mut pixels, sprite.width, sprite.height);
        canvas.blit(&sprite.source(), 0, 0);
        for y in 0..image.height {
            for x in 0..image.width {
                let want = image.pixels[y * image.width + x];
                assert_ne!(
                    want,
                    Some(SENTINEL),
                    "{name}: the sentinel is an art colour"
                );
                assert_eq!(
                    canvas.pixel_at(x as i32, y as i32),
                    Some(want.unwrap_or(SENTINEL)),
                    "{name} at ({x}, {y})"
                );
            }
        }
        assert_eq!(pixels.len(), image.width * image.height * BYTES_PER_PIXEL);
    }
}

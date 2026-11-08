//! The art oracle: 8-bit RGBA PNGs, and the pixel-for-pixel comparison every
//! art test makes against them (BACKLOG 116).
//!
//! `tests/art/` holds the Aseprite exports the procedural replacements are
//! pinned to (see its README); `art/` holds the hand-drawn sources. Both read
//! the same way: a pixel is transparent when its alpha is 0 — the layout
//! compiler's rule (`flatten_rgba`), and every file here has only 0 or 255 —
//! and otherwise carries its exact 8-bit colour.
//!
//! The comparison has two halves. [`assert_quantized`] is today's: the canvas
//! is RGB565, so a drawn pixel must equal the oracle's colour through
//! [`rgb565`], and a transparent one must still hold [`SENTINEL`]. And
//! [`assert_drawn`] adds the 8-bit half for code that draws from named
//! constants: the constants must be exactly the oracle's colours, no more and
//! no fewer, with distinct 565 packings — so the 565 value a pixel holds names
//! the one constant that drew it, and the draw is exact at eight bits too.

#![allow(dead_code)]

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use scoreboard_model::Rgb888;
use scoreboard_render::blit::Canvas;
use scoreboard_render::rgb565;

/// A frame-buffer value no art here quantizes to, written under every draw so
/// that a pixel the draw must leave alone is seen to survive it.
pub const SENTINEL: u16 = rgb565(8, 4, 8);

/// One decoded image: `None` for a transparent pixel, else its exact colour.
pub struct Art {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pixels: Vec<Option<[u8; 3]>>,
}

impl Art {
    /// An Aseprite export or toast PNG from `tests/art/`.
    pub fn oracle(name: &str) -> Art {
        Art::load(&crate_path("tests/art").join(format!("{name}.png")))
    }

    /// A hand-drawn source from `art/`.
    pub fn source(name: &str) -> Art {
        Art::load(&crate_path("art").join(format!("{name}.png")))
    }

    pub fn load(path: &Path) -> Art {
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
            "{}: not 8 bits per channel",
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
                    "{}: partial alpha {alpha} has no meaning on the panel",
                    path.display()
                );
                (alpha == 255).then(|| [pixel[0], pixel[1], pixel[2]])
            })
            .collect();
        Art {
            name: path.file_stem().unwrap().to_string_lossy().into_owned(),
            width: frame.width as i32,
            height: frame.height as i32,
            pixels,
        }
    }

    /// The colour at `(x, y)`, or `None` where the art is transparent.
    pub fn at(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        assert!(x >= 0 && y >= 0 && x < self.width && y < self.height);
        self.pixels[(y * self.width + x) as usize]
    }

    /// The distinct opaque colours, in row-major first-seen order — the order
    /// `compile_layout.py` numbered its palettes in.
    pub fn colors(&self) -> Vec<[u8; 3]> {
        let mut seen = Vec::new();
        for color in self.pixels.iter().flatten() {
            if !seen.contains(color) {
                seen.push(*color);
            }
        }
        seen
    }
}

fn crate_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A `width` × `height` RGB565 frame filled with [`SENTINEL`].
pub fn buffer(width: i32, height: i32) -> Vec<u8> {
    SENTINEL.to_le_bytes().repeat((width * height) as usize)
}

/// The 8-bit triple of a model colour.
pub fn channels(color: Rgb888) -> [u8; 3] {
    [color.red(), color.green(), color.blue()]
}

/// The window of `canvas` whose top-left is `(x0, y0)` is `art`, pixel for
/// pixel: every opaque pixel is its colour through [`rgb565`], every
/// transparent one still [`SENTINEL`]. Lists every mismatch, not the first.
pub fn assert_quantized(canvas: &Canvas<'_>, x0: i32, y0: i32, art: &Art, what: &str) {
    for color in art.colors() {
        assert_ne!(
            rgb565(color[0], color[1], color[2]),
            SENTINEL,
            "{what}: the sentinel collides with an art colour"
        );
    }
    let mut wrong = Vec::new();
    for y in 0..art.height {
        for x in 0..art.width {
            let want = match art.at(x, y) {
                Some([r, g, b]) => rgb565(r, g, b),
                None => SENTINEL,
            };
            let got = canvas.pixel_at(x0 + x, y0 + y);
            if got != Some(want) {
                let got = got.map_or("nothing (outside the canvas)".into(), |v| {
                    format!("{v:#06x}")
                });
                wrong.push(format!(
                    "  ({x}, {y}): want {want:#06x} ({:?}), got {got}",
                    art.at(x, y)
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{what}: {} of {} pixels differ from {}.png\n{}",
        wrong.len(),
        art.width * art.height,
        art.name,
        wrong[..wrong.len().min(24)].join("\n")
    );
}

/// [`assert_quantized`], plus the 8-bit half: `palette` — the colour
/// constants the draw was given — is exactly the art's set of colours, and no
/// two of them pack to one 565 value. Then the 565 value at each pixel can only
/// have come from the constant that equals the oracle's 8-bit colour there.
pub fn assert_drawn(
    canvas: &Canvas<'_>,
    x0: i32,
    y0: i32,
    art: &Art,
    palette: &[Rgb888],
    what: &str,
) {
    let packed: Vec<u16> = palette
        .iter()
        .map(|&color| {
            let [r, g, b] = channels(color);
            rgb565(r, g, b)
        })
        .collect();
    for (i, a) in packed.iter().enumerate() {
        assert!(
            !packed[i + 1..].contains(a),
            "{what}: two palette colours share the 565 value {a:#06x}"
        );
    }
    let mut constants: Vec<[u8; 3]> = palette.iter().map(|&c| channels(c)).collect();
    let mut colors = art.colors();
    constants.sort_unstable();
    colors.sort_unstable();
    assert_eq!(
        constants, colors,
        "{what}: the colour constants are not exactly {}.png's 8-bit colours",
        art.name
    );
    assert_quantized(canvas, x0, y0, art, what);
}

//! The art oracle: 8-bit RGBA PNGs, and the pixel-for-pixel comparison every
//! art test makes against them (BACKLOG 116).
//!
//! `tests/art/` holds the Aseprite exports the procedural replacements are
//! pinned to (see its README); `art/` holds the hand-drawn sources. Both read
//! the same way: a pixel is transparent when its alpha is 0 — the layout
//! compiler's rule, and every file here has only 0 or 255 — and otherwise
//! carries its exact 8-bit colour.
//!
//! The canvas is eight bits a channel (BACKLOG 115), so the comparison is
//! exact: [`assert_art`] wants every opaque pixel to be the art's colour and
//! every transparent one to still hold [`SENTINEL`]. [`assert_drawn`] adds,
//! for code that draws from named constants, that the constants are exactly
//! the art's colours — no more and no fewer.

#![allow(dead_code)]

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use scoreboard_model::Rgb888;
use scoreboard_render::blit::{BYTES_PER_PIXEL, Canvas};

/// A colour no art here uses, written under every draw so that a pixel the
/// draw must leave alone is seen to survive it.
pub const SENTINEL: Rgb888 = Rgb888::new(1, 2, 3);

/// One decoded image: `None` for a transparent pixel, else its exact colour.
pub struct Art {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pixels: Vec<Option<Rgb888>>,
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
                (alpha == 255).then(|| Rgb888::new(pixel[0], pixel[1], pixel[2]))
            })
            .collect();
        Art {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            width: frame.width as i32,
            height: frame.height as i32,
            pixels,
        }
    }

    /// The colour at `(x, y)`, or `None` where the art is transparent.
    pub fn at(&self, x: i32, y: i32) -> Option<Rgb888> {
        assert!(x >= 0 && y >= 0 && x < self.width && y < self.height);
        self.pixels[(y * self.width + x) as usize]
    }

    /// The distinct opaque colours, in row-major first-seen order — the order
    /// `compile_layout.py` numbered its palettes in.
    pub fn colors(&self) -> Vec<Rgb888> {
        let mut seen = Vec::new();
        for color in self.pixels.iter().flatten() {
            if !seen.contains(color) {
                seen.push(*color);
            }
        }
        seen
    }

    /// This art with every pixel of the given colours made transparent — what
    /// a draw that skips those colours should leave.
    pub fn without(&self, colors: &[Rgb888]) -> Art {
        for color in colors {
            assert!(
                self.colors().contains(color),
                "{}: no {color:?} to drop",
                self.name
            );
        }
        Art {
            name: format!("{} without {colors:?}", self.name),
            pixels: self
                .pixels
                .iter()
                .map(|pixel| pixel.filter(|color| !colors.contains(color)))
                .collect(),
            ..*self
        }
    }

    /// The `width` × `height` window of this art whose top-left is `(x, y)`.
    pub fn crop(&self, x: i32, y: i32, width: i32, height: i32) -> Art {
        assert!(x >= 0 && y >= 0 && x + width <= self.width && y + height <= self.height);
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for row in y..y + height {
            for col in x..x + width {
                pixels.push(self.at(col, row));
            }
        }
        Art {
            name: format!("{} at ({x}, {y}) {width}x{height}", self.name),
            width,
            height,
            pixels,
        }
    }
}

fn crate_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// A `width` × `height` frame filled with [`SENTINEL`].
pub fn buffer(width: i32, height: i32) -> Vec<u8> {
    let pixel: [u8; BYTES_PER_PIXEL] = [SENTINEL.red(), SENTINEL.green(), SENTINEL.blue()];
    pixel.repeat((width * height) as usize)
}

/// The window of `canvas` whose top-left is `(x0, y0)` is `art`, pixel for
/// pixel and bit for bit: every opaque pixel its colour, every transparent
/// one still [`SENTINEL`]. Lists every mismatch, not the first.
pub fn assert_art(canvas: &Canvas<'_>, x0: i32, y0: i32, art: &Art, what: &str) {
    assert!(
        !art.colors().contains(&SENTINEL),
        "{what}: the sentinel collides with an art colour"
    );
    let mut wrong = Vec::new();
    for y in 0..art.height {
        for x in 0..art.width {
            let want = art.at(x, y).unwrap_or(SENTINEL);
            let got = canvas.pixel_at(x0 + x, y0 + y);
            if got != Some(want) {
                wrong.push(format!(
                    "  ({x}, {y}): want {want:?} ({:?}), got {got:?}",
                    art.at(x, y)
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{what}: {} of {} pixels differ from {}\n{}",
        wrong.len(),
        art.width * art.height,
        art.name,
        wrong[..wrong.len().min(24)].join("\n")
    );
}

/// [`assert_art`], plus: `palette` — the colour constants the draw was given
/// — is exactly the art's set of colours.
pub fn assert_drawn(
    canvas: &Canvas<'_>,
    x0: i32,
    y0: i32,
    art: &Art,
    palette: &[Rgb888],
    what: &str,
) {
    let mut constants: Vec<u32> = palette.iter().map(|color| color.0).collect();
    let mut colors: Vec<u32> = art.colors().iter().map(|color| color.0).collect();
    constants.sort_unstable();
    colors.sort_unstable();
    assert_eq!(
        constants, colors,
        "{what}: the colour constants are not exactly the colours of {}",
        art.name
    );
    assert_art(canvas, x0, y0, art, what);
}

/// Every pixel of `canvas` outside the `width` × `height` window at
/// `(x0, y0)` still holds [`SENTINEL`]: the draw stayed inside its rectangle.
pub fn assert_untouched_outside(
    canvas: &Canvas<'_>,
    (x0, y0, width, height): (i32, i32, i32, i32),
    what: &str,
) {
    for y in 0..canvas.height() {
        for x in 0..canvas.width() {
            let inside = (x0..x0 + width).contains(&x) && (y0..y0 + height).contains(&y);
            if !inside {
                assert_eq!(
                    canvas.pixel_at(x, y),
                    Some(SENTINEL),
                    "{what}: drew outside its rectangle at ({x}, {y})"
                );
            }
        }
    }
}

/// Two frames of one size are the same, pixel for pixel: the procedural draw
/// and the compiled sprite blitted through the palette a renderer tints.
/// Lists every pixel that differs.
pub fn assert_same_frame(drawn: &Canvas<'_>, blitted: &Canvas<'_>, what: &str) {
    assert_eq!(
        (drawn.width(), drawn.height()),
        (blitted.width(), blitted.height())
    );
    let mut wrong = Vec::new();
    for y in 0..drawn.height() {
        for x in 0..drawn.width() {
            let (a, b) = (drawn.pixel_at(x, y), blitted.pixel_at(x, y));
            if a != b {
                wrong.push(format!("  ({x}, {y}): drawn {a:?}, blitted {b:?}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{what}: {} pixels differ from the tinted sprite\n{}",
        wrong.len(),
        wrong[..wrong.len().min(24)].join("\n")
    );
}

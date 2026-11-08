//! The pixel path: an RGB888 draw surface, clipping sub-views, and blits from
//! the five packed source formats the generated art and the crests use.
//!
//! This is the port of MicroPython's `framebuf` module as the firmware
//! actually used it — `display.py`'s `Region`, the `blit`/`fill_rect`/`line`
//! calls under it, and the packing conventions
//! `tools/compile_layout.py` emits. The normative reference for those
//! conventions is the layout compiler's `pack_*` functions; the pure-Python
//! oracle `tools/preview/shims/framebuf_shim.py` is pinned against them by its
//! own tests, and this module's tests are a port of that pinning.
//!
//! Three details are load-bearing and easy to get wrong:
//!
//! 1. **The bit orders disagree between formats.** `MONO_HLSB` puts the
//!    leftmost pixel in the *most* significant bit; `GS2_HMSB` puts it in the
//!    *low* two bits; `GS4_HMSB` puts it in the *high* nibble. That is
//!    MicroPython's own inconsistency, and the compiled sprites are packed to
//!    match it.
//! 2. **Transparency is a palette entry, not a colour.** A see-through pixel
//!    is one whose palette entry is `None` — index 0 of every compiled sprite
//!    with transparency, the background of a text draw that has none, any
//!    entry a renderer blanks to reveal what is under a sprite. MicroPython
//!    keyed on the mapped colour magenta instead, which made every colour that
//!    packed to the key undrawable (BACKLOG 112, gone with it).
//! 3. **Rows are byte-whole.** A source's stride rounds up to a whole number
//!    of bytes per row, so an 11-pixel-wide `MONO_HLSB` glyph occupies 2 bytes
//!    per row and row 1 never reads row 0's tail.

use crate::Rgb888;

/// Packed source formats, named for their `framebuf` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 1 bpp, **MSB = leftmost pixel**. Fonts, QR, 1-bit sprites.
    MonoHlsb,
    /// 2 bpp, **leftmost pixel in the LOW two bits** (`shift = (x % 4) * 2`).
    Gs2Hmsb,
    /// 4 bpp, **leftmost pixel in the HIGH nibble**.
    Gs4Hmsb,
    /// 1 byte per pixel, palette index.
    Gs8,
    /// 2 bytes per pixel, little-endian. No palette: the pixels are colours,
    /// widened to eight bits a channel by [`expand_rgb565`] as they land. The
    /// crests' format.
    Rgb565,
}

impl PixelFormat {
    const fn pixels_per_byte(self) -> usize {
        match self {
            PixelFormat::MonoHlsb => 8,
            PixelFormat::Gs2Hmsb => 4,
            PixelFormat::Gs4Hmsb => 2,
            PixelFormat::Gs8 | PixelFormat::Rgb565 => 1,
        }
    }

    /// Bytes one source row occupies at `stride` pixels.
    ///
    /// The rounding *is* MicroPython's stride rounding: it rounds the stride up
    /// to a whole number of bytes, which is why generated sprites whose row
    /// byte counts use `ceil(width / pixels_per_byte)` read back correctly at
    /// the default `stride == width`.
    pub const fn row_bytes(self, stride: usize) -> usize {
        match self {
            PixelFormat::Rgb565 => stride * 2,
            other => stride.div_ceil(other.pixels_per_byte()),
        }
    }

    /// Read the raw value at `(x, y)` — a palette index, or an RGB565 colour
    /// for [`PixelFormat::Rgb565`].
    fn read(self, data: &[u8], row_bytes: usize, x: usize, y: usize) -> u16 {
        let row = y * row_bytes;
        match self {
            PixelFormat::Rgb565 => u16::from_le_bytes([data[row + x * 2], data[row + x * 2 + 1]]),
            PixelFormat::Gs8 => data[row + x] as u16,
            PixelFormat::Gs4Hmsb => {
                let byte = data[row + (x >> 1)];
                if x & 1 == 1 {
                    (byte & 0x0F) as u16
                } else {
                    (byte >> 4) as u16
                }
            }
            PixelFormat::Gs2Hmsb => {
                let byte = data[row + (x >> 2)];
                ((byte >> ((x & 0x03) * 2)) & 0x03) as u16
            }
            PixelFormat::MonoHlsb => {
                let byte = data[row + (x >> 3)];
                ((byte >> (7 - (x & 0x07))) & 1) as u16
            }
        }
    }
}

/// One blit source: packed pixels plus how to read and map them.
///
/// `stride` is the source's row pitch in pixels, which is the width for a
/// sprite and the pool capacity for a text strip — a strip can present any
/// prefix width over the same buffer.
#[derive(Debug, Clone, Copy)]
pub struct Source<'data> {
    pub data: &'data [u8],
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: PixelFormat,
    /// Entries indexed by the raw source value; a `None` entry is not drawn.
    /// `None` for the whole palette means the source carries its own colours
    /// ([`PixelFormat::Rgb565`]), all of them drawn.
    pub palette: Option<&'data [Option<Rgb888>]>,
}

impl<'data> Source<'data> {
    /// A source whose stride equals its width — every sprite and glyph.
    pub const fn new(
        data: &'data [u8],
        width: i32,
        height: i32,
        format: PixelFormat,
        palette: Option<&'data [Option<Rgb888>]>,
    ) -> Self {
        Source {
            data,
            width,
            height,
            stride: width,
            format,
            palette,
        }
    }

    /// Swap in a different palette — the Rust replacement for MicroPython's
    /// tint-the-shared-palette-and-restore-in-a-`finally` idiom. The
    /// replacement is a caller-owned array, so nothing outlives the draw.
    pub const fn with_palette(mut self, palette: &'data [Option<Rgb888>]) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Narrow to the leftmost `width` pixels, keeping the row pitch — how a
    /// pre-rendered strip presents the part of the pool that holds text.
    pub const fn with_width(mut self, width: i32) -> Self {
        self.width = width;
        self
    }

    /// The colour a raw source value draws, or `None` where it is
    /// see-through.
    fn map(&self, raw: u16) -> Option<Rgb888> {
        match self.palette {
            Some(palette) => palette[raw as usize],
            None => Some(expand_rgb565(raw)),
        }
    }
}

/// A sprite compiled from source art: pixels and palette, ready to blit.
///
/// The generated modules in [`crate::generated`] are `const` values of this
/// type. `palette` stays `&'static` and immutable; a renderer that needs to
/// tint copies it into a local array and calls [`Sprite::tinted`].
#[derive(Debug, Clone, Copy)]
pub struct Sprite {
    pub data: &'static [u8],
    pub width: i32,
    pub height: i32,
    pub format: PixelFormat,
    pub palette: Option<&'static [Option<Rgb888>]>,
}

impl Sprite {
    pub const fn source(&self) -> Source<'static> {
        Source::new(
            self.data,
            self.width,
            self.height,
            self.format,
            self.palette,
        )
    }

    /// This sprite drawn through a caller-owned palette.
    pub const fn tinted<'a>(&self, palette: &'a [Option<Rgb888>]) -> Source<'a> {
        Source {
            data: self.data,
            width: self.width,
            height: self.height,
            stride: self.width,
            format: self.format,
            palette: Some(palette),
        }
    }
}

/// A named rectangle from the source art — an Aseprite slice, or an
/// `__absolute` layer's authoritative position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Bytes one pixel occupies on a [`Canvas`]: red, green, blue, in that order —
/// the layout `hub75`'s `load_rgb888` packs, so a finished frame goes to the
/// driver as it lies.
pub const BYTES_PER_PIXEL: usize = 3;

/// An RGB565 crest or clip pixel (a little-endian `framebuf.RGB565` value)
/// widened to eight bits a channel by bit replication: each field's top bits
/// repeat into its low ones, so 0 stays 0 and full scale lands on 255.
///
/// This is exactly the expansion the panel driver applied when the drawing
/// surface was RGB565, which is why a crest lights the same LEDs it did then,
/// and why truncating the result back to RGB565 returns the original value.
pub const fn expand_rgb565(raw: u16) -> Rgb888 {
    let red = (raw >> 11) as u8;
    let green = ((raw >> 5) & 0x3F) as u8;
    let blue = (raw & 0x1F) as u8;
    Rgb888::new(
        (red << 3) | (red >> 2),
        (green << 2) | (green >> 4),
        (blue << 3) | (blue >> 2),
    )
}

/// An RGB888 drawing surface, or a clipping sub-view of one.
///
/// The port of `display.py`'s `Region`: a view carries its parent's row pitch,
/// so every write clips to the view's own bounds with no masking by the
/// caller. Views nest, and the borrow checker keeps a parent unusable while
/// one of its views is alive — the compile-time form of "Regions are the draw
/// target, and only one thing draws at a time".
pub struct Canvas<'pixels> {
    /// [`BYTES_PER_PIXEL`] bytes a pixel, starting at this view's top-left
    /// pixel.
    pixels: &'pixels mut [u8],
    width: i32,
    height: i32,
    /// Row pitch in pixels — the *parent's* width, for a sub-view.
    stride: i32,
}

impl<'pixels> Canvas<'pixels> {
    /// A canvas over a whole frame buffer.
    pub fn new(pixels: &'pixels mut [u8], width: i32, height: i32) -> Self {
        assert!(width >= 0 && height >= 0, "negative canvas size");
        assert!(
            pixels.len() >= (width * height) as usize * BYTES_PER_PIXEL,
            "canvas buffer too small"
        );
        Canvas {
            pixels,
            width,
            height,
            stride: width,
        }
    }

    pub const fn width(&self) -> i32 {
        self.width
    }

    pub const fn height(&self) -> i32 {
        self.height
    }

    /// A clipping sub-view. The rectangle must lie inside this canvas: the
    /// geometry tables are compile-time constants, so a rectangle that does not
    /// fit is a table bug worth failing the first host test that draws it,
    /// never something to silently clamp.
    pub fn region(&mut self, x: i32, y: i32, width: i32, height: i32) -> Canvas<'_> {
        assert!(
            x >= 0 && y >= 0 && width >= 0 && height >= 0,
            "negative region"
        );
        assert!(
            x + width <= self.width && y + height <= self.height,
            "region escapes its parent"
        );
        let offset = (y * self.stride + x) as usize * BYTES_PER_PIXEL;
        Canvas {
            pixels: &mut self.pixels[offset..],
            width,
            height,
            stride: self.stride,
        }
    }

    /// A sub-view over a [`Slice`] from the source art.
    pub fn slice(&mut self, rect: Slice) -> Canvas<'_> {
        self.region(rect.x, rect.y, rect.width, rect.height)
    }

    fn index(&self, x: i32, y: i32) -> usize {
        (y * self.stride + x) as usize * BYTES_PER_PIXEL
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.width && y < self.height
    }

    /// Set one pixel; out-of-bounds coordinates are dropped.
    pub fn pixel(&mut self, x: i32, y: i32, color: Rgb888) {
        if self.contains(x, y) {
            let index = self.index(x, y);
            self.pixels[index..index + BYTES_PER_PIXEL].copy_from_slice(&bytes(color));
        }
    }

    /// Read one pixel; `None` out of bounds.
    pub fn pixel_at(&self, x: i32, y: i32) -> Option<Rgb888> {
        self.contains(x, y).then(|| {
            let index = self.index(x, y);
            let pixel = &self.pixels[index..index + BYTES_PER_PIXEL];
            Rgb888::new(pixel[0], pixel[1], pixel[2])
        })
    }

    pub fn fill(&mut self, color: Rgb888) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// Fill a rectangle, clipped to the canvas.
    ///
    /// A gray — black above all, which clears every frame — is one byte value
    /// repeated, so it fills as a byte run the compiler turns into a `memset`
    /// of whole words; any other colour is written a pixel at a time.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: Rgb888) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + width).min(self.width);
        let y1 = (y + height).min(self.height);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let pixel = bytes(color);
        let gray = pixel[0] == pixel[1] && pixel[1] == pixel[2];
        for row in y0..y1 {
            let (start, end) = (self.index(x0, row), self.index(x1, row));
            let span = &mut self.pixels[start..end];
            if gray {
                span.fill(pixel[0]);
            } else {
                for out in span.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
                    *out = pixel;
                }
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, width: i32, color: Rgb888) {
        self.fill_rect(x, y, width, 1, color);
    }

    pub fn vline(&mut self, x: i32, y: i32, height: i32, color: Rgb888) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// A one-pixel outline.
    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: Rgb888) {
        if width <= 0 || height <= 0 {
            return;
        }
        self.hline(x, y, width, color);
        self.hline(x, y + height - 1, width, color);
        self.vline(x, y, height, color);
        self.vline(x + width - 1, y, height, color);
    }

    /// Bresenham line, ported statement-for-statement from MicroPython's
    /// `modframebuf.c` — the steep-swap variant that draws `dx` pixels in the
    /// loop and then sets the second endpoint unconditionally, so both
    /// endpoints are always lit. The football perspective lines are golden
    /// tested against it.
    pub fn line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: Rgb888) {
        let (mut x, mut y) = (x1, y1);
        let mut dx = x2 - x1;
        let mut sx = if dx > 0 { 1 } else { -1 };
        dx = dx.abs();
        let mut dy = y2 - y1;
        let mut sy = if dy > 0 { 1 } else { -1 };
        dy = dy.abs();

        let steep = dy > dx;
        if steep {
            core::mem::swap(&mut x, &mut y);
            core::mem::swap(&mut dx, &mut dy);
            core::mem::swap(&mut sx, &mut sy);
        }

        let mut error = 2 * dy - dx;
        for _ in 0..dx {
            if steep {
                self.pixel(y, x, color);
            } else {
                self.pixel(x, y, color);
            }
            while error >= 0 {
                y += sy;
                error -= 2 * dx;
            }
            x += sx;
            error += 2 * dy;
        }
        self.pixel(x2, y2, color);
    }

    /// Copy `source` in at `(x, y)`, clipping every edge.
    ///
    /// Reproduces `framebuf.blit()`'s clipping exactly — per pixel, including
    /// negative offsets. A pixel whose palette entry is `None` is skipped.
    pub fn blit(&mut self, source: &Source<'_>, x: i32, y: i32) {
        let row_bytes = source.format.row_bytes(source.stride as usize);
        for row in 0..source.height {
            let dst_y = y + row;
            if dst_y < 0 || dst_y >= self.height {
                continue;
            }
            // The horizontal span that lands on the canvas. Skipping the rest
            // outright is what keeps a 255-glyph play line cheap when only 76
            // px of it are visible.
            let first = (-x).max(0);
            let last = (self.width - x).min(source.width);
            for col in first..last {
                let raw = source
                    .format
                    .read(source.data, row_bytes, col as usize, row as usize);
                let Some(color) = source.map(raw) else {
                    continue;
                };
                let index = self.index(x + col, dst_y);
                self.pixels[index..index + BYTES_PER_PIXEL].copy_from_slice(&bytes(color));
            }
        }
    }

    /// Multiply every pixel by `terms`' brightness factor, in place.
    ///
    /// The port of `display._dim_frame`, re-derived for eight-bit channels
    /// (BACKLOG 115). The factor is a sum of shifts — `c >> 1`, plus `c >> 2`
    /// and `c >> 3` as the rung asks — applied to every channel byte. Four
    /// bytes go at once as a `u32`, each shift masked to the bits that stay
    /// inside their own byte (`0x7F`, `0x3F`, `0x1F` per byte), so a channel
    /// never bleeds into its neighbour; which bytes share a word does not
    /// matter, because every byte is a channel and every channel is scaled the
    /// same. The sum cannot overflow a byte because the factor is below 1.
    ///
    /// Same four rungs, same truncation per term, as the RGB565 version, which
    /// applied these shifts to 5- and 6-bit fields with masks built for the
    /// R5/G6/B5 boundaries; at eight bits the steps are finer and a dimmed
    /// colour keeps its hue.
    ///
    /// Panics unless this canvas is contiguous (a whole frame, not a sub-view).
    pub fn dim(&mut self, terms: FadeTerms) {
        assert_eq!(self.stride, self.width, "dim() needs a contiguous canvas");
        let len = (self.width * self.height) as usize * BYTES_PER_PIXEL;
        let (words, tail) = self.pixels[..len].as_chunks_mut::<4>();
        for word in words {
            let raw = u32::from_le_bytes(*word);
            let mut value = (raw >> 1) & 0x7F7F_7F7F;
            if terms.half_step {
                value += (raw >> 2) & 0x3F3F_3F3F;
            }
            if terms.quarter_step {
                value += (raw >> 3) & 0x1F1F_1F1F;
            }
            *word = value.to_le_bytes();
        }
        // Only a canvas whose byte count is not a multiple of four has any;
        // a frame (24,576 B) has none.
        for channel in tail {
            let raw = *channel;
            let half = if terms.half_step { raw >> 2 } else { 0 };
            let quarter = if terms.quarter_step { raw >> 3 } else { 0 };
            *channel = (raw >> 1) + half + quarter;
        }
    }

    /// The view's pixels, for bulk reads (tests, and pushing a finished frame
    /// to the driver).
    pub fn pixels(&self) -> &[u8] {
        self.pixels
    }
}

/// One rung of the toast dim ladder: which shifted terms are summed on top of
/// the always-present `w >> 1`.
///
/// `display._FADE_TERMS`, spelled out. Index 0 is 7/8 and index 3 is the held
/// 1/2 level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FadeTerms {
    pub half_step: bool,
    pub quarter_step: bool,
}

/// The ladder: 7/8, 3/4, 5/8, 1/2. Fade-in walks 0 → 3 from the toast's start;
/// fade-out walks 2 → 0 after it expires.
pub const FADE_TERMS: [FadeTerms; 4] = [
    FadeTerms {
        half_step: true,
        quarter_step: true,
    },
    FadeTerms {
        half_step: true,
        quarter_step: false,
    },
    FadeTerms {
        half_step: false,
        quarter_step: true,
    },
    FadeTerms {
        half_step: false,
        quarter_step: false,
    },
];

/// The two-entry palette every text draw runs through: index 0 is the
/// background (`None` for a transparent draw), index 1 the glyph colour.
///
/// In MicroPython this was `FontWriter._palette_buf`, a preallocated
/// `bytearray` on the writer that had to be registered as scratch and poisoned
/// between frames to prove nothing read it stale. Here it is a local array
/// built at every call site, which is the same guarantee with nothing to
/// register.
pub const fn text_palette(background: Option<Rgb888>, color: Rgb888) -> [Option<Rgb888>; 2] {
    [background, Some(color)]
}

/// A colour as the three bytes a pixel occupies.
const fn bytes(color: Rgb888) -> [u8; BYTES_PER_PIXEL] {
    [color.red(), color.green(), color.blue()]
}

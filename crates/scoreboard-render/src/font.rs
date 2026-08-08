//! Glyph tables and the text drawing on top of them — the port of
//! `scoreboard/fonts/__init__.py`.
//!
//! # No writer object
//!
//! MicroPython's `FontWriter` existed to own preallocated scratch: a 2-entry
//! palette buffer and a 5-slot digit list, both allocated once because
//! allocating on the render thread meant GC pauses. Both are plain locals
//! here, so the class has nothing left to hold and every entry point is a free
//! function. That also removes them from the Core-1 scratch registry — see the
//! mutation-contract table in the crate docs.
//!
//! # Fonts are fixed-width
//!
//! All three fonts are fixed-width (`spleen_5x8` 5×8, `unscii_8` 8×8,
//! `unscii_16` 8×16), which is what lets `screen_geometry`'s tables be sized in
//! glyph counts. The table format still carries a per-glyph width, so nothing
//! here assumes it.

use crate::MAGENTA;
use crate::blit::{Canvas, PixelFormat, Source, text_palette};
use crate::time::Motion;

/// Index sentinel for a codepoint the source font has no glyph for.
const ABSENT: u16 = 0xFFFF;
/// First codepoint in the table. Everything below is a control character.
const FIRST_CODEPOINT: u32 = 32;
/// Table entries, covering codepoints 32..=255.
const TABLE_ENTRIES: u32 = 224;

/// One generated glyph table: a record heap plus an offset index.
///
/// Both blobs are `&'static` — they live in flash, cost no RAM, and need no
/// initialisation. The MicroPython modules built a tuple table at import to get
/// the same effect; a lookup here is two loads and a bounds check, so there is
/// nothing to build.
#[derive(Debug)]
pub struct FontFace {
    height: i32,
    /// Per-glyph records: `u16`-LE width, then `ceil(width / 8) * height`
    /// bytes of `MONO_HLSB` rows.
    heap: &'static [u8],
    /// Slot 0 is the default glyph; slots `1..=224` are codepoints 32..=255.
    index: &'static [u16],
}

impl FontFace {
    pub const fn new(height: i32, heap: &'static [u8], index: &'static [u16]) -> Self {
        FontFace {
            height,
            heap,
            index,
        }
    }

    pub const fn height(&self) -> i32 {
        self.height
    }

    /// The glyph for `c`, or the default (`'?'`) for anything outside
    /// ASCII + Latin-1 and for coverage holes inside it.
    ///
    /// Wire strings are folded into this repertoire at ingest
    /// (`scoreboard_model::text`), so the fallback is for text this firmware
    /// did not author — an SSID, mostly.
    pub fn glyph(&self, c: char) -> Glyph {
        let slot = (c as u32).wrapping_sub(FIRST_CODEPOINT);
        let offset = if slot < TABLE_ENTRIES {
            self.index[slot as usize + 1]
        } else {
            ABSENT
        };
        let offset = if offset == ABSENT {
            self.index[0]
        } else {
            offset
        };
        self.record(offset as usize)
    }

    /// The glyph for one decimal digit. `integer` walks digits, not chars.
    pub fn digit(&self, value: u8) -> Glyph {
        self.glyph((b'0' + value % 10) as char)
    }

    fn record(&self, offset: usize) -> Glyph {
        let width = u16::from_le_bytes([self.heap[offset], self.heap[offset + 1]]) as i32;
        let bytes = (width as usize).div_ceil(8) * self.height as usize;
        Glyph {
            bits: &self.heap[offset + 2..offset + 2 + bytes],
            width,
            height: self.height,
        }
    }
}

/// One glyph's `MONO_HLSB` bitmap, ready to blit through a 2-entry palette.
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    pub bits: &'static [u8],
    pub width: i32,
    pub height: i32,
}

impl Glyph {
    /// This glyph as a blit source. Index 0 of `palette` is the background,
    /// index 1 the ink.
    pub fn source<'a>(&self, palette: &'a [u16], key: Option<u16>) -> Source<'a> {
        Source::new(
            self.bits,
            self.width,
            self.height,
            PixelFormat::MonoHlsb,
            Some(palette),
            key,
        )
    }
}

/// Horizontal placement inside a bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    /// The left edge for `text_width` inside `width`.
    ///
    /// Centering floors rather than truncating: text wider than its box gives a
    /// negative difference, and MicroPython's `//` floors it. Truncation would
    /// shift such a line one pixel right of where the panel drew it — visible
    /// on the 22 px soccer score slots, which a three-digit score overflows.
    const fn offset(self, width: i32, text_width: i32) -> i32 {
        match self {
            Align::Left => 0,
            Align::Center => (width - text_width).div_euclid(2),
            Align::Right => width - text_width,
        }
    }
}

/// Width of `text` in pixels.
pub fn measure(text: &str, font: &FontFace) -> i32 {
    text.chars().map(|c| font.glyph(c).width).sum()
}

/// How a run of text is inked.
///
/// The MicroPython entry points carried `color`, `bgcolor` and `font` as three
/// loose arguments through five methods. Grouping them is what keeps the call
/// sites readable now that there is no writer object holding the font.
#[derive(Debug, Clone, Copy)]
pub struct Style<'font> {
    pub font: &'font FontFace,
    pub color: u16,
    /// The color behind the glyphs. `None` leaves everything but the lit pixels
    /// untouched, via the [`MAGENTA`] key.
    ///
    /// The two entry points differ in how far it reaches: [`text_into`] paints
    /// only the glyph cells, while [`draw`] clears the whole region first. That
    /// is `FontWriter.text` and `FontWriter.draw`'s behavior respectively, and
    /// the menu's DONE footer depends on the first of them.
    pub background: Option<u16>,
}

impl<'font> Style<'font> {
    /// Ink with no background: only the lit pixels are written.
    pub const fn new(font: &'font FontFace, color: u16) -> Self {
        Style {
            font,
            color,
            background: None,
        }
    }

    /// The same ink over an opaque background.
    pub const fn on(self, background: u16) -> Self {
        Style {
            background: Some(background),
            ..self
        }
    }

    fn palette(&self) -> ([u16; 2], Option<u16>) {
        match self.background {
            Some(background) => (text_palette(background, self.color), None),
            None => (text_palette(MAGENTA, self.color), Some(MAGENTA)),
        }
    }
}

/// How a line that overflows its box moves.
///
/// **The speed must evenly divide the frame rate.** See
/// [`crate::geometry::SCROLL_SPEEDS`] for why a non-divisor drops pixel
/// columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    /// Dwell at each end of the cycle.
    pub pause_ms: u64,
    pub pixels_per_second: i32,
}

impl Scroll {
    /// `FontWriter.draw`'s defaults — the pitcher/batter feel.
    pub const DEFAULT: Scroll = Scroll {
        pause_ms: 2000,
        pixels_per_second: 20,
    };
}

/// Pixel offset into an overflowing line at `elapsed`, over the cycle
/// `[pause at start] -> [scroll] -> [pause at end] -> repeat`.
///
/// Pure function of its inputs — `fonts.calculate_scroll_offset`.
pub fn scroll_offset(text_width: i32, display_width: i32, elapsed: Motion, scroll: Scroll) -> i32 {
    let max_scroll = text_width - display_width;
    if max_scroll <= 0 {
        return 0;
    }
    let scroll_ms = (max_scroll as u64 * 1000) / scroll.pixels_per_second as u64;
    let cycle_ms = scroll.pause_ms + scroll_ms + scroll.pause_ms;
    let position = elapsed.0 % cycle_ms;

    if position < scroll.pause_ms {
        0
    } else if position < scroll.pause_ms + scroll_ms {
        ((position - scroll.pause_ms) * scroll.pixels_per_second as u64 / 1000) as i32
    } else {
        max_scroll
    }
}

/// Draw `text` at `(x, y)`.
///
/// Returns the x after the last glyph, for chaining.
pub fn text_into(canvas: &mut Canvas<'_>, text: &str, x: i32, y: i32, style: Style<'_>) -> i32 {
    let (palette, key) = style.palette();
    let mut cursor = x;
    for c in text.chars() {
        let glyph = style.font.glyph(c);
        canvas.blit(&glyph.source(&palette, key), cursor, y);
        cursor += glyph.width;
    }
    cursor
}

/// Draw `text` aligned inside a `width`-wide box at `(x, y)`.
pub fn aligned_text(
    canvas: &mut Canvas<'_>,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    align: Align,
    style: Style<'_>,
) -> i32 {
    let start = x + align.offset(width, measure(text, style.font));
    text_into(canvas, text, start, y, style)
}

/// Draw a number without ever building a string.
///
/// `FontWriter.integer` existed because `str(value)` allocates in MicroPython.
/// Formatting into a `heapless::String` would not allocate here, but the digit
/// walk is still both shorter and exact, and it keeps the alignment arithmetic
/// identical to what the panel showed. The argument is `u16` because every
/// score in the snapshot is — five digits always fit, so the MicroPython
/// version's silent five-digit cap has nothing to cap.
pub fn integer(
    canvas: &mut Canvas<'_>,
    value: u16,
    x: i32,
    y: i32,
    width: i32,
    align: Align,
    style: Style<'_>,
) -> i32 {
    let mut digits = [0u8; 5];
    let mut count = 0;
    let mut rest = value;
    loop {
        digits[count] = (rest % 10) as u8;
        rest /= 10;
        count += 1;
        if rest == 0 {
            break;
        }
    }

    let total: i32 = digits[..count]
        .iter()
        .map(|digit| style.font.digit(*digit).width)
        .sum();
    let mut cursor = x + align.offset(width, total);

    let (palette, key) = style.palette();
    for digit in digits[..count].iter().rev() {
        let glyph = style.font.digit(*digit);
        canvas.blit(&glyph.source(&palette, key), cursor, y);
        cursor += glyph.width;
    }
    cursor
}

/// Draw `text` into a region, scrolling it when it overflows.
///
/// The general text entry point (`FontWriter.draw`): alignment while the line
/// fits, the [`scroll_offset`] cycle once it does not, and glyph blits clipped
/// by the region so no caller masks anything. A style with a background clears
/// the whole region first.
///
/// `elapsed` is only consulted when the text overflows. Which rail it comes
/// from is the caller's decision and a meaningful one — see [`crate::time`].
pub fn draw(
    region: &mut Canvas<'_>,
    text: &str,
    align: Align,
    elapsed: Motion,
    style: Style<'_>,
    scroll: Scroll,
) {
    let width = region.width();
    if let Some(background) = style.background {
        region.fill(background);
    }
    let (palette, key) = style.palette();

    let text_width = measure(text, style.font);
    let mut cursor = if text_width <= width {
        align.offset(width, text_width)
    } else {
        -scroll_offset(text_width, width, elapsed, scroll)
    };

    for c in text.chars() {
        let glyph = style.font.glyph(c);
        // Glyphs scrolled off either edge cost one comparison instead of a
        // clipped blit, which is what keeps a 255-glyph play line as cheap as
        // the 76 px window it shows through.
        if cursor + glyph.width > 0 && cursor < width {
            region.blit(&glyph.source(&palette, key), cursor, 0);
        }
        cursor += glyph.width;
        if cursor >= width {
            break;
        }
    }
}

/// Draw `text` into a region with no motion: aligned while it fits, clipped at
/// the region's edge when it does not.
///
/// The MicroPython form of this was `writer.draw(..., elapsed_ms=0, ...)` —
/// same call, with the reader left to notice that a zero elapsed means the
/// scroll never leaves its opening pause.
pub fn draw_unscrolled(region: &mut Canvas<'_>, text: &str, align: Align, style: Style<'_>) {
    draw(region, text, align, Motion(0), style, Scroll::DEFAULT);
}

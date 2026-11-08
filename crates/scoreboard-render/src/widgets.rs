//! The drawn primitives more than one screen needs.
//!
//! A primitive lands here the moment a second sport wants it (NBA.md D11):
//! football's possession arrow became hockey's banner arrow and the NBA's
//! caption arrow, and football's timeout bars became the NBA's timeout blocks.
//! No sport keeps a private copy.

use crate::Rgb888;
use crate::blit::{Canvas, Slice};
use crate::generated::layout;

/// A 3×5 solid triangle, top-left corner at `(x, y)`, pointing the way
/// `points_right` says — the possession arrow, hockey's strength banner's, the
/// court caption's.
pub fn arrow(canvas: &mut Canvas<'_>, x: i32, y: i32, points_right: bool, color: Rgb888) {
    for step in 0..3 {
        let column = if points_right { x + step } else { x + 2 - step };
        canvas.vline(column, y + step, 5 - 2 * step, color);
    }
}

/// A horizontal progress bar: a one-pixel border with the fill inside it.
///
/// `progress` is a percentage. `display.draw_progress_bar`.
pub fn progress_bar(
    canvas: &mut Canvas<'_>,
    rect: Slice,
    progress: u8,
    border: Rgb888,
    fill: Rgb888,
) {
    canvas.rect(rect.x, rect.y, rect.width, rect.height, border);
    let fill_width = (rect.width - 2) * progress.min(100) as i32 / 100;
    if fill_width > 0 {
        canvas.fill_rect(rect.x + 1, rect.y + 1, fill_width, rect.height - 2, fill);
    }
}

/// A row of timeout blocks — football's three bars under each crest, the
/// NBA's seven under each score: `total` equal blocks a pixel apart, the
/// first `remaining` in `color` and the burned ones in `dark`, so a side's
/// timeouts empty left to right.
///
/// A block is as tall as the row and as wide as the row's width leaves per
/// block once the gaps are out, so — as with [`count_dots`] — the geometry
/// table decides the shape: a 20 × 1 row holds three 6 × 1 bars, a 20 × 2
/// row seven 2 × 2 blocks.
pub fn timeout_blocks(
    canvas: &mut Canvas<'_>,
    rect: Slice,
    total: u8,
    remaining: u8,
    color: Rgb888,
    dark: Rgb888,
) {
    let total = total as i32;
    let Some(block) = (rect.width - (total - 1)).checked_div(total) else {
        return;
    };
    for index in 0..total {
        let held = index < remaining as i32;
        canvas.fill_rect(
            rect.x + index * (block + 1),
            rect.y,
            block,
            rect.height,
            if held { color } else { dark },
        );
    }
}

/// A row of count dots — balls/strikes/outs on the MLB screen, and the Wi-Fi
/// attempt dots during startup.
///
/// The dot count comes from the row's pixel width, so the geometry table's
/// widths are what decide whether a row shows three dots or four.
///
/// `tint` colors *every* dot's outline, not just the filled ones, so the whole
/// row reads as one color when the critical-count pulse is running; the filled
/// dots' interiors take the same color. MicroPython wrote those two entries
/// into the sprite's shared palette and restored them in a `finally` so a
/// throwing blit could not leave later frames pulsed. Here the palette is
/// copied into a local first, which is the same guarantee with nothing to
/// restore.
pub fn count_dots(canvas: &mut Canvas<'_>, rect: Slice, filled: u8, tint: Option<Rgb888>) {
    let sprite = layout::dot::SPRITE;
    let mut palette = layout::dot::PALETTE;
    let outline = tint.or(palette[1]);
    let unfilled = palette[2];
    palette[1] = outline;

    let pitch = sprite.width + 1;
    let count = (rect.width + 1) / pitch;
    for index in 0..count {
        palette[2] = if index < filled as i32 {
            outline
        } else {
            unfilled
        };
        canvas.blit(&sprite.tinted(&palette), rect.x + index * pitch, rect.y);
    }
}

//! The two drawn primitives more than one screen needs.

use crate::blit::{Canvas, Slice};
use crate::generated::layout;

/// A horizontal progress bar: a one-pixel border with the fill inside it.
///
/// `progress` is a percentage. `display.draw_progress_bar`.
pub fn progress_bar(canvas: &mut Canvas<'_>, rect: Slice, progress: u8, border: u16, fill: u16) {
    canvas.rect(rect.x, rect.y, rect.width, rect.height, border);
    let fill_width = (rect.width - 2) * progress.min(100) as i32 / 100;
    if fill_width > 0 {
        canvas.fill_rect(rect.x + 1, rect.y + 1, fill_width, rect.height - 2, fill);
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
pub fn count_dots(canvas: &mut Canvas<'_>, rect: Slice, filled: u8, tint: Option<u16>) {
    let sprite = layout::dot::SPRITE;
    let mut palette = layout::dot::PALETTE;
    let outline = tint.unwrap_or(palette[1]);
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

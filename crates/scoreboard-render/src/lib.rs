#![no_std]
#![forbid(unsafe_code)]
//! Everything between a [`ScoreboardSnapshot`] and the pixels on the panel:
//! glyph tables, sprite data, screen geometry, the blitter, and the screen
//! renderers.
//!
//! This is the port of `firmware/src/scoreboard/display.py`,
//! `screen_geometry.py`, `menu.py` and `fonts/__init__.py`. It owns no state
//! that survives a call and touches no hardware — the caller hands it a
//! [`Canvas`] over an RGB565 buffer and a snapshot to read.
//!
//! [`ScoreboardSnapshot`]: scoreboard_model::ScoreboardSnapshot
//!
//! # The prepared view, and why the render path is a pure reader
//!
//! `scoreboard-model`'s snapshot carries semantics, never pixels. Anything
//! derived — measured text widths, scroll windows, the setup QR — is built by
//! [`PreparedView`] once per commit (keyed on
//! [`ScoreboardSnapshot::commit_seq`]) and read unchanged by every frame in
//! between. See [`prepared`] for the rebuild rule.
//!
//! [`ScoreboardSnapshot::commit_seq`]: scoreboard_model::ScoreboardSnapshot::commit_seq
//!
//! # The Core-1 mutation contract, as ownership
//!
//! `display.py:1761-1817` enumerates the four things the MicroPython render
//! thread is allowed to write, and backs the rule with a grep (`ls` must not
//! appear below `render_frame`) plus a preview-side tripwire that poisons
//! registered scratch between frames. Rust does not need either: each bucket
//! becomes something the type system already enforces.
//!
//! | MicroPython bucket | Rust form | What enforces it |
//! |---|---|---|
//! | `LoopState` — all cross-frame state | the app's loop-local struct (Phase 3), holding a [`FrameRail`] | it is a local; renderers take `WallMs`/`FrameElapsed` **values**, never the struct, so they cannot reach it |
//! | Registered scratch (`scratch_buffers()`, `SCRATCH_PALETTE_ENTRIES`) | plain locals: a `[u16; 2]` palette in [`font::text_into`], a copied sprite palette in [`toast::overlay`] | a local cannot outlive the call, so "scratch silently promoted to cross-frame state" is not expressible |
//! | Draw targets | [`Canvas`], holding `&mut [u8]` | the borrow checker: exactly one canvas over a given region at a time |
//! | `ThreadHealth.frame_seq` | an `AtomicU32` the app owns (Phase 3) | not this crate's business |
//!
//! The load-bearing consequence: **this crate mutates no static, ever.** Every
//! generated table is `&'static` immutable data, and the sprite palettes that
//! MicroPython tinted in place — count dots, base markers, endzone tints, the
//! spinner comet — are copied into a caller-owned array first. That deletes
//! the `try`/`finally` restores (`_draw_count_dots`, `_draw_base_markers`,
//! `_draw_football_field`), the scratch registry, and the poisoning tripwire
//! that existed to catch violations of a rule Rust makes unstatable.
//!
//! # What is generated
//!
//! [`generated`] holds committed build products — glyph tables from
//! `tools/compile_fonts.py`, sprites and slices from `tools/compile_layout.py`.
//! They are committed (unlike their MicroPython counterparts, which are
//! gitignored) so that regenerating needs Python, freetype and the Aseprite
//! CLI but *building* needs only cargo. Same pattern as `hub75`'s goldens.

pub mod blit;
pub mod court;
pub mod event;
pub mod font;
pub mod frame;
pub mod game;
pub mod generated;
pub mod geometry;
pub mod menu;
pub mod prepared;
pub mod qr;
pub mod screens;
pub mod time;
pub mod toast;
pub mod widgets;

pub use blit::{Canvas, PixelFormat, Slice, Source, Sprite};
pub use font::{Align, FontFace};
pub use frame::SkipMemo;
pub use game::{Logos, Scene};
pub use geometry::{HEIGHT, RenderSettings, WIDTH};
pub use prepared::PreparedView;
pub use time::{FrameElapsed, FrameRail, WallMs};

/// Pack 8-bit channels into RGB565 — `fonts.rgb565`, and the same expression
/// as [`hub75::rgb565`](https://docs.rs/hub75) so a color computed here and a
/// color packed by the driver are one value.
pub const fn rgb565(red: u8, green: u8, blue: u8) -> u16 {
    ((red as u16 & 0xF8) << 8) | ((green as u16 & 0xFC) << 3) | (blue as u16 >> 3)
}

/// The snapshot's `Rgb888`, packed for the panel.
pub fn pack(color: scoreboard_model::Rgb888) -> u16 {
    rgb565(color.red(), color.green(), color.blue())
}

/// The snapshot's `Rgb888` dimmed to `num`/`den` of itself, then packed.
///
/// A palette color that has to step down — the toast's fade, overtime losses
/// under the losses — scales its 8-bit channels before packing. Scaling the
/// packed value instead would truncate through 5 and 6 bits first, and a white
/// primary would no longer land on exactly the gray ramp the panel drew before
/// these elements took their color from the palette.
pub fn pack_scaled(color: scoreboard_model::Rgb888, num: u32, den: u32) -> u16 {
    let scale = |channel: u8| (channel as u32 * num / den).min(255) as u8;
    rgb565(
        scale(color.red()),
        scale(color.green()),
        scale(color.blue()),
    )
}

/// A packed color's brightness scaled by `num`/`den`, per 565 channel.
///
/// The counterpart of [`pack_scaled`] for a color that is already on the
/// panel's side of the pack — a team's end of the rink, the court's lane
/// lines, the power-play glow — where the 5- and 6-bit truncation has already
/// happened and there is no 8-bit value left to be faithful to.
pub fn scale(color: u16, num: u32, den: u32) -> u16 {
    let red = ((color >> 11) & 0x1F) as u32 * num / den;
    let green = ((color >> 5) & 0x3F) as u32 * num / den;
    let blue = (color & 0x1F) as u32 * num / den;
    ((red.min(0x1F) as u16) << 11) | ((green.min(0x3F) as u16) << 5) | blue.min(0x1F) as u16
}

/// `over` composited onto `under` at `alpha`/256, through 8-bit channels.
///
/// The one blend in the crate: hockey's goal marks and the court's makes read
/// the pixel under them back from the canvas and write this over it, so marks
/// from one spot climb toward the team color instead of fusing (NHL.md D9,
/// NBA.md D5). The channels are widened to 8 bits for the arithmetic and
/// packed again on the way out; the stair that quantisation puts in the
/// climb is BACKLOG 115's.
pub fn mix(under: u16, over: u16, alpha: u32) -> u16 {
    let ([under_r, under_g, under_b], [over_r, over_g, over_b]) = (expand(under), expand(over));
    let blend = |under: u32, over: u32| ((under * (256 - alpha) + over * alpha) / 256) as u8;
    rgb565(
        blend(under_r, over_r),
        blend(under_g, over_g),
        blend(under_b, over_b),
    )
}

/// A packed color's channels widened back to 8 bits, by scaling — the inverse
/// of [`rgb565`]'s truncation as nearly as the lost bits allow.
fn expand(color: u16) -> [u32; 3] {
    let (red, green, blue) = (
        (color >> 11) as u32 & 0x1F,
        (color >> 5) as u32 & 0x3F,
        color as u32 & 0x1F,
    );
    [red * 255 / 31, green * 255 / 63, blue * 255 / 31]
}

/// Transparency sentinel shared by `fonts.MAGENTA_RGB565`, the layout
/// compiler's `_TRANSPARENT_RGB565`, and every sprite's `KEY`.
///
/// It is the *mapped* color, not a palette index, because `framebuf.blit()`
/// looks the source pixel up through the palette **before** comparing against
/// the key — so a paletted sprite and an RGB565 sprite carry the same key.
/// [`blit`] reproduces that ordering.
pub const MAGENTA: u16 = 0xF81F;

pub const BLACK: u16 = 0;
pub const WHITE: u16 = rgb565(255, 255, 255);
pub const DIM_GRAY: u16 = rgb565(96, 96, 96);

/// Triangle wave in `0..=256`, cycling every `period_ms` (`display.pulse`).
///
/// Integer-only. In MicroPython that was to dodge heap-allocated floats; here
/// it stays integer so the pulse phases land on exactly the same values the
/// panel showed before.
pub fn pulse(elapsed_ms: u64, period_ms: u64) -> u32 {
    let phase = ((elapsed_ms % period_ms) * 512 / period_ms) as u32;
    if phase > 256 { 512 - phase } else { phase }
}

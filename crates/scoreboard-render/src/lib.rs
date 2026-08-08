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
//! | Registered scratch (`scratch_buffers()`, `SCRATCH_PALETTE_ENTRIES`) | plain locals: a `[u16; 2]` palette in [`font::text_into`], a copied sprite palette in [`toast::spinner`] | a local cannot outlive the call, so "scratch silently promoted to cross-frame state" is not expressible |
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
pub mod font;
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
pub use geometry::{HEIGHT, WIDTH};
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

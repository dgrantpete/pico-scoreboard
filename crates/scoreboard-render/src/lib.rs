#![no_std]
#![forbid(unsafe_code)]
//! Everything between a [`ScoreboardSnapshot`] and the pixels on the panel:
//! glyph tables, sprite data, screen geometry, the blitter, and the screen
//! renderers.
//!
//! This is the port of `firmware/src/scoreboard/display.py`,
//! `screen_geometry.py`, `menu.py` and `fonts/__init__.py`. It owns no state
//! that survives a call and touches no hardware — the caller hands it a
//! [`Canvas`] over an RGB888 buffer and a snapshot to read.
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
//! | Registered scratch (`scratch_buffers()`, `SCRATCH_PALETTE_ENTRIES`) | plain locals: a two-entry palette in [`font::text_into`], a copied sprite palette in [`toast::overlay`] | a local cannot outlive the call, so "scratch silently promoted to cross-frame state" is not expressible |
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
//!
//! # Colour
//!
//! Every colour on the drawing surface is the snapshot's own [`Rgb888`]: eight
//! bits a channel from the configured palette and the team colours, through
//! the blends, to the three bytes a pixel occupies and the driver packs
//! (BACKLOG 115). Nothing is rounded to fewer bits on the way; the driver's
//! gamma table is the only quantiser. Sprite palettes say where a sprite is
//! see-through with `None` entries rather than a reserved colour, so every
//! colour the palette can hold is drawable.
//!
//! Two content formats stay RGB565: crests, as the backend serves them and the
//! pool holds them, and event clips' palettes. Both are widened by bit
//! replication where they are drawn ([`blit::expand_rgb565`]) — the expansion
//! the panel always applied to them — so they light exactly the LEDs they lit
//! when the surface was RGB565.

pub mod blit;
pub mod count_dot;
pub mod court;
pub mod event;
pub mod font;
pub mod frame;
pub mod game;
pub mod generated;
pub mod geometry;
pub mod inning_arrow;
pub mod menu;
pub mod prepared;
pub mod qr;
pub mod rink;
pub mod screens;
pub mod star;
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

pub use scoreboard_model::Rgb888;

pub const BLACK: Rgb888 = Rgb888::new(0, 0, 0);
pub const WHITE: Rgb888 = Rgb888::WHITE;
pub const DIM_GRAY: Rgb888 = Rgb888::new(96, 96, 96);

/// A colour's brightness scaled by `num`/`den`, per 8-bit channel.
///
/// The step-downs that derive one colour from another — the toast's fade and
/// its spinner trail, overtime losses under the losses, a team's end of the
/// rink, the court's lane lines, the power-play glow — all go through here.
/// Channels saturate at 255, so `num > den` brightens without wrapping.
pub const fn scale(color: Rgb888, num: u32, den: u32) -> Rgb888 {
    const fn channel(value: u8, num: u32, den: u32) -> u8 {
        let scaled = value as u32 * num / den;
        if scaled > 255 { 255 } else { scaled as u8 }
    }
    Rgb888::new(
        channel(color.red(), num, den),
        channel(color.green(), num, den),
        channel(color.blue(), num, den),
    )
}

/// `over` composited onto `under` at `alpha`/256, per 8-bit channel.
///
/// The one blend in the crate: hockey's goal marks and the court's makes read
/// the pixel under them back from the canvas and write this over it, so marks
/// from one spot climb toward the team colour instead of fusing (NHL.md D9,
/// NBA.md D5); the goal lamp and the NBA run glow pulse through it.
pub const fn mix(under: Rgb888, over: Rgb888, alpha: u32) -> Rgb888 {
    const fn channel(under: u8, over: u8, alpha: u32) -> u8 {
        ((under as u32 * (256 - alpha) + over as u32 * alpha) / 256) as u8
    }
    Rgb888::new(
        channel(under.red(), over.red(), alpha),
        channel(under.green(), over.green(), alpha),
        channel(under.blue(), over.blue(), alpha),
    )
}

/// Triangle wave in `0..=256`, cycling every `period_ms` (`display.pulse`).
///
/// Integer-only. In MicroPython that was to dodge heap-allocated floats; here
/// it stays integer so the pulse phases land on exactly the same values the
/// panel showed before.
pub fn pulse(elapsed_ms: u64, period_ms: u64) -> u32 {
    let phase = ((elapsed_ms % period_ms) * 512 / period_ms) as u32;
    if phase > 256 { 512 - phase } else { phase }
}

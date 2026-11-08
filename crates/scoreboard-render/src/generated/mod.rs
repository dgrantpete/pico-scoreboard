//! Committed build products: glyph tables and the hand-drawn sprites.
//!
//! Nothing in here is hand-written, and each file says what wrote it:
//!
//! - the glyph tables, from `firmware/assets/fonts/*.{pcf,bdf}` by
//!   `tools/compile_fonts.py`, which emits the MicroPython module and the
//!   Rust module beside it from one pass over the same bytes;
//! - [`art`], from `crates/scoreboard-render/art/*.png` by
//!   `tests/art_compile.rs`, a cargo test that also fails whenever the
//!   committed module is not what the PNGs compile to (BACKLOG 116).
//!
//! ```text
//! py tools/compile_fonts.py    # needs freetype
//! SCOREBOARD_BLESS_ART=1 cargo test -p scoreboard-render --test art_compile
//! ```
//!
//! Everything else the panel draws is code: the rink, the court, the count
//! dots, the toasts' icons — see their modules.
//!
//! # Why these are committed
//!
//! Regenerating the fonts needs Python and freetype; committing the output
//! keeps both out of the cargo graph, so `cargo build` and CI need nothing but
//! a toolchain. The art module is committed for the same reason a build
//! script would not do: compiling PNGs needs a decoder, which belongs in the
//! test graph, not in every device build. `hub75`'s goldens and
//! `scoreboard-wire`'s fixtures are the same trade.
//!
//! # Flash, not RAM
//!
//! Every table here is an immutable `static`, so the linker places it in
//! `.rodata` — flash on the RP2350, addressed in place via XIP. The RAM budget
//! line for fonts is zero and stays zero as long as nothing in this crate takes
//! a `&mut` to one, which nothing can: they are not `static mut` and this crate
//! forbids `unsafe`.

pub mod art;
pub mod spleen_5x8;
pub mod unscii_16;
pub mod unscii_8;

pub use spleen_5x8::SPLEEN_5X8;
pub use unscii_8::UNSCII_8;
pub use unscii_16::UNSCII_16;

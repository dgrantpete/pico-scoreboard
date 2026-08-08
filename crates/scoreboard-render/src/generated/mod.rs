//! Committed build products: glyph tables and compiled sprite data.
//!
//! Nothing in here is hand-written. The sources of truth are
//! `firmware/assets/fonts/*.{pcf,bdf}` and `firmware/assets/layout/*.aseprite`,
//! and the generators are `tools/compile_fonts.py` and
//! `tools/compile_layout.py` — each emits the MicroPython module *and* the Rust
//! module beside it from one pass over the same bytes, so the two firmwares
//! cannot drift apart.
//!
//! ```text
//! py tools/compile_fonts.py    # needs freetype
//! py tools/compile_layout.py   # needs Pillow + the Aseprite CLI
//! ```
//!
//! # Why these are committed when their MicroPython twins are gitignored
//!
//! Regenerating needs Python, freetype, Pillow and a GUI application's CLI.
//! Committing the output is what keeps all four out of the cargo graph, so
//! `cargo build` and CI need nothing but a toolchain. `hub75`'s goldens and
//! `scoreboard-wire`'s fixtures are the same trade.
//!
//! # Flash, not RAM
//!
//! Every table here is an immutable `static`, so the linker places it in
//! `.rodata` — flash on the RP2350, addressed in place via XIP. The RAM budget
//! line for fonts is zero and stays zero as long as nothing in this crate takes
//! a `&mut` to one, which nothing can: they are not `static mut` and this crate
//! forbids `unsafe`.

pub mod layout;
pub mod spleen_5x8;
pub mod unscii_16;
pub mod unscii_8;

pub use spleen_5x8::SPLEEN_5X8;
pub use unscii_8::UNSCII_8;
pub use unscii_16::UNSCII_16;

//! CPU-free HUB75 LED-matrix driver for the RP2350.
//!
//! Port of `firmware/src/lib/hub75/` (MicroPython): same hardware strategy,
//! new implementation. Two PIO state machines (pixel data + row
//! address/BCM timing) synchronized by a latch-safe/latch-complete IRQ
//! handshake, fed by four DMA channels arranged as two self-perpetuating
//! control/data pairs. After construction the panel refreshes entirely in
//! hardware; the CPU only loads frames and flips buffers.
//!
//! Geometry is fixed at compile time to the production panel (128×64,
//! 1/32-scan, binary row addressing — see [`geometry`]), matching how the
//! MicroPython build passed `COLOR_BIT_DEPTH` in as a C macro.
//!
//! Everything that does not touch a peripheral ([`gamma`], [`packing`],
//! [`timing`], [`programs`], [`display`] with the `simulator` feature) builds
//! and tests on the host.

#![no_std]

pub mod display;
pub mod driver;
pub mod gamma;
pub mod geometry;
pub mod packing;
pub mod programs;
#[cfg(feature = "simulator")]
pub mod sim;
pub mod timing;

/// Pack 8-bit channels into RGB565 (the `framebuf.RGB565` layout the display
/// buffer and `load_rgb565` consume, as a `u16`; store little-endian).
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

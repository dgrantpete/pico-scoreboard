//! Compile-time panel geometry: the production 128×64 1/32-scan panel
//! (display.py `init_display`). The MicroPython driver took these as runtime
//! arguments but only one configuration ever ships; baking them in sizes the
//! static framebuffers and lets the PIO program carry the address bit count
//! in its instruction stream.

/// BCM bitplanes per channel; 256 brightness levels. Also baked into the
/// address-SM timing stream (two words per plane) and the PIO cycle model.
pub const COLOR_BIT_DEPTH: usize = 8;

/// Binary row-address pins (A..E). Baked into the address PIO program as the
/// `in x, 32 - ROW_ADDRESS_BITS` shift — see `src/programs.rs`.
pub const ROW_ADDRESS_BITS: usize = 5;

/// Distinct row addresses the panel cycles through (1/32 scan).
pub const ROW_ADDRESS_COUNT: usize = 1 << ROW_ADDRESS_BITS;

/// Pixels clocked into the panel's shift register per row address.
pub const SHIFT_REGISTER_DEPTH: usize = 128;

/// Display width in pixels (standard indoor panel: width = register depth).
pub const WIDTH: usize = SHIFT_REGISTER_DEPTH;

/// Display height in pixels (two rows lit per address: top + bottom half).
pub const HEIGHT: usize = ROW_ADDRESS_COUNT * 2;

/// Total pixels per frame.
pub const PIXEL_COUNT: usize = WIDTH * HEIGHT;

/// Bytes per bitplane: one byte per pixel *pair* (top-half and bottom-half
/// rows share a byte of packed R1,G1,B1,R2,G2,B2 bits).
pub const BITPLANE_BYTES: usize = PIXEL_COUNT / 2;

/// Bytes per BCM framebuffer (all bitplanes, plane-major, LSB plane first).
pub const BITPLANE_BUFFER_BYTES: usize = BITPLANE_BYTES * COLOR_BIT_DEPTH;

/// Bytes in an RGB565 input frame (little-endian, `framebuf.RGB565` layout).
pub const RGB565_FRAME_BYTES: usize = PIXEL_COUNT * 2;

/// Bytes in an RGB888 input frame (R, G, B per pixel).
pub const RGB888_FRAME_BYTES: usize = PIXEL_COUNT * 3;

/// u32 words in the address-SM timing stream: an (off, on) pair per bitplane.
pub const TIMING_WORDS: usize = COLOR_BIT_DEPTH * 2;

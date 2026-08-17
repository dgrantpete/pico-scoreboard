//! Streaming PNG → 24×24 RGB565 sprite decoder (Phase S).
//!
//! Turns an ESPN team-logo PNG, fed in arbitrary network-sized chunks, into
//! a 24×24 RGB565 sprite blended over a caller-chosen background color —
//! entirely `no_std`, allocation-free, integer-only, in caller-owned
//! buffers. Neither the compressed stream nor the full-resolution image is
//! ever buffered: PNG chunk framing is parsed incrementally, IDAT bytes go
//! straight into miniz_oxide's inflate core over a 32 KiB ring window, and
//! decompressed bytes are consumed row-by-row with only the current and
//! previous rows retained for defiltering.
//!
//! # Scope (a decision, from the measured CDN reality)
//!
//! All six sampled ESPN CDN logos (both the 500×500 originals and the
//! 100×100 combiner variants) are 8-bit RGBA (color type 6), non-interlaced.
//! The supported envelope is exactly that plus the cheap neighbor:
//!
//! - bit depth 8, color type 6 (RGBA) or 2 (RGB), non-interlaced,
//!   width/height 1..=[`MAX_DIM`].
//!
//! Everything else — palette (3), grayscale (0/4), 16-bit, Adam7 interlace —
//! is a clean [`Error::Unsupported`], not a rendering attempt. `tRNS`/`PLTE`
//! and all ancillary chunks are skipped. Bytes after `IEND` are ignored
//! (an HTTP body feed may be sliced generously).
//!
//! # Integrity (CRC decision)
//!
//! Per-chunk PNG CRC32s are **skipped, not validated** — on-device the
//! stream arrives over TCP, whose checksum already covers transport
//! corruption, and a CRC pass would buy a second copy of nothing. The zlib
//! **adler32 over the decompressed pixel data _is_ validated** (miniz_oxide
//! checks it when parsing the zlib wrapper), so the bytes that actually
//! become pixels still carry an end-to-end checksum. A corrupt stream
//! surfaces as [`Error::Deflate`] or [`Error::Malformed`], never a panic.
//!
//! # Downsample (box filter decision)
//!
//! Source pixels map to the 24×24 grid by floor arithmetic
//! (`cell = coord * 24 / dim`) — cells therefore differ by at most one
//! source row/column in weight, which at 500×500→24×24 (≈21× reduction) is
//! visually irrelevant and keeps the hot path to one multiply per axis.
//! Accumulation is premultiplied-alpha in `u32`: per source pixel,
//! `a·r, a·g, a·b, a` are added to the destination cell. Bounds: a cell
//! covers at most ⌈1024/24⌉² = 43² = 1849 source pixels, so a channel
//! accumulator peaks at 255·255·1849 ≈ 1.2×10⁸ < 2³² and the blend
//! numerator at ≈ 2.4×10⁸ < 2³¹. [`SpriteDecoder::finish`] divides,
//! blends over the background (`out = (Σa·c + bg·(255·n − Σa)) / (255·n)`,
//! round-to-nearest), and packs RGB565 (`[u16; 576]`, row-major; on the
//! RP2350 the in-memory byte order is little-endian). Cells with no source
//! pixels (width or height < 24) come out as pure background. This is a new
//! device capability, not a byte-parity port of the backend's CatmullRom
//! resize — the exact-decode stage (inflate + defilter) is byte-checked
//! against the `png` crate instead, via [`RowDecoder`].
//!
//! # Memory
//!
//! Everything lives in the caller's [`Scratch`] (compile-time-asserted
//! ≤ 64 KiB: inflate state ≈ 10.5 KiB + 32 KiB window + 2×4 KiB rows +
//! 9 KiB accumulators + 1 KiB column map). The crate declares no statics.

#![no_std]

mod decode;
mod down;

pub use decode::RowSink;

use decode::Core;
use down::Down;
use miniz_oxide::inflate::core::{DecompressorOxide, TINFL_LZ_DICT_SIZE};

/// Sprite edge length, in pixels.
pub const SPRITE_DIM: usize = 24;
/// Pixels in a finished sprite.
pub const SPRITE_PIXELS: usize = SPRITE_DIM * SPRITE_DIM;
/// Largest accepted source width/height. Sized to the CDN reality (500×500)
/// with 2× headroom; also what bounds the row buffers and the accumulator
/// arithmetic documented on the crate.
pub const MAX_DIM: usize = 1024;

/// Bytes in one full-resolution RGBA row at [`MAX_DIM`].
pub(crate) const MAX_STRIDE: usize = MAX_DIM * 4;
/// Inflate ring window: the zlib format's maximum LZ77 window. Must be a
/// power of two (miniz_oxide's wrapping-output contract).
pub(crate) const WINDOW: usize = TINFL_LZ_DICT_SIZE;

/// A finished 24×24 sprite: RGB565 values, row-major.
pub type Sprite = [u16; SPRITE_PIXELS];

/// Decoder failure. Small and `Copy`; a failed decoder is poisoned and
/// returns the same error from every subsequent call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The first eight bytes were not the PNG signature.
    Signature,
    /// Chunk framing or header contents violate the PNG spec (bad IHDR
    /// field, zero dimension, wrong chunk length, filter byte > 4, more
    /// pixel data than the announced dimensions call for, …).
    Malformed,
    /// A valid PNG outside the supported scope (palette, grayscale,
    /// 16-bit, interlaced — see the crate doc).
    Unsupported,
    /// Width or height exceeds [`MAX_DIM`].
    TooLarge,
    /// The zlib/deflate stream is corrupt (including an adler32 mismatch
    /// over the decompressed pixel data).
    Deflate,
    /// `finish` before the image was complete (truncated feed), or a zlib
    /// stream that ended before supplying every row.
    Truncated,
}

/// Background color for the final blend, 8 bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// From `0xRRGGBB` — the shape `parse_hex_rgb` hands around.
    pub const fn from_rgb888(rgb: u32) -> Self {
        Self {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
        }
    }
}

/// Caller-owned working memory for one decoder. Create once (typically in a
/// `StaticCell` on-device), reuse across images — each `*Decoder::new`
/// fully re-initializes it.
pub struct Scratch {
    /// miniz_oxide inflate state (huffman tables + registers), ≈ 10.5 KiB.
    inflate: DecompressorOxide,
    /// LZ77 ring window; doubles as the decompressor's history.
    window: [u8; WINDOW],
    /// Current + previous defilter rows, one [`MAX_STRIDE`] half each.
    rows: [u8; 2 * MAX_STRIDE],
    /// Premultiplied accumulators: `[Σa·r, Σa·g, Σa·b, Σa]` per cell.
    acc: [u32; 4 * SPRITE_PIXELS],
    /// Source column → sprite column, precomputed once per image.
    col_map: [u8; MAX_DIM],
}

// The RAM-inventory bound the report cites, enforced on every target.
const _: () = assert!(core::mem::size_of::<Scratch>() <= 64 * 1024);

impl Scratch {
    pub fn new() -> Self {
        Scratch {
            inflate: DecompressorOxide::new(),
            window: [0; WINDOW],
            rows: [0; 2 * MAX_STRIDE],
            acc: [0; 4 * SPRITE_PIXELS],
            col_map: [0; MAX_DIM],
        }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}

/// Streaming PNG → sprite decoder: feed the raw network bytes to [`write`]
/// in chunks of any size (1-byte feeds included — the split-invariance
/// tests pin this), then [`finish`] with the background color.
///
/// [`write`]: SpriteDecoder::write
/// [`finish`]: SpriteDecoder::finish
pub struct SpriteDecoder<'a> {
    core: Core<'a>,
    down: Down<'a>,
}

impl<'a> SpriteDecoder<'a> {
    pub fn new(scratch: &'a mut Scratch) -> Self {
        let Scratch {
            inflate,
            window,
            rows,
            acc,
            col_map,
        } = scratch;
        SpriteDecoder {
            core: Core::new(inflate, window, rows),
            down: Down::new(acc, col_map),
        }
    }

    /// Feed the next slice of the PNG byte stream.
    pub fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        self.core.write(data, &mut self.down)
    }

    /// Blend over `bg` and pack. Errors [`Error::Truncated`] unless the
    /// stream was complete: IEND seen, zlib stream terminated (adler32
    /// verified), every row delivered.
    pub fn finish(self, bg: Rgb8) -> Result<Sprite, Error> {
        self.core.complete()?;
        Ok(self.down.finish(bg))
    }
}

/// Full-resolution row streamer over the same core — the exact-decode
/// surface. The oracle tests byte-compare its output (inflate + defilter,
/// no downsampling) against the `png` crate; a bench firmware can hang
/// timing probes off it. Same scope, same streaming contract.
pub struct RowDecoder<'a> {
    core: Core<'a>,
}

impl<'a> RowDecoder<'a> {
    pub fn new(scratch: &'a mut Scratch) -> Self {
        let Scratch {
            inflate,
            window,
            rows,
            ..
        } = scratch;
        RowDecoder {
            core: Core::new(inflate, window, rows),
        }
    }

    /// Feed bytes; `sink` receives `start` once, then each defiltered row.
    pub fn write(&mut self, data: &[u8], sink: &mut impl RowSink) -> Result<(), Error> {
        self.core.write(data, sink)
    }

    /// Verify the stream was complete (same contract as
    /// [`SpriteDecoder::finish`]).
    pub fn finish(self) -> Result<(), Error> {
        self.core.complete()
    }
}

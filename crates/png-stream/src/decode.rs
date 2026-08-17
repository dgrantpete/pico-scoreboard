//! The streaming core: PNG chunk framing → inflate → per-row defilter.
//!
//! Byte-granular by construction — every state consumes as many bytes as
//! the caller's slice offers and no fewer than it must, so a 1-byte feed
//! walks the identical state sequence as a whole-buffer feed (the
//! split-invariance tests pin this). No state ever needs look-behind
//! across a `write` boundary beyond the fixed accumulation buffers.

use crate::{Error, MAX_DIM, MAX_STRIDE, WINDOW};
use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

/// Receives the decoded image: `start` once (after IHDR is accepted), then
/// one `row` call per image row, top to bottom, `width × channels` bytes,
/// fully defiltered.
pub trait RowSink {
    fn start(&mut self, width: u32, height: u32, channels: u8);
    fn row(&mut self, y: u32, px: &[u8]);
}

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const IHDR: [u8; 4] = *b"IHDR";
const IDAT: [u8; 4] = *b"IDAT";
const IEND: [u8; 4] = *b"IEND";

/// Zlib wrapper parsed (adler32 therefore validated at stream end);
/// more input may always follow — chunk feeds never assert finality.
const INFLATE_FLAGS: u32 =
    inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER | inflate_flags::TINFL_FLAG_HAS_MORE_INPUT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    /// Matching the 8 signature bytes.
    Signature,
    /// Accumulating an 8-byte chunk header (length + type).
    ChunkHeader,
    /// Accumulating the 13-byte IHDR payload.
    IhdrData,
    /// Streaming IDAT payload into inflate.
    IdatData,
    /// Skipping an unhandled chunk's payload.
    SkipData,
    /// Skipping a 4-byte chunk CRC (never validated — crate doc).
    Crc,
    /// IEND fully consumed; all further bytes ignored.
    Done,
    /// Poisoned; the error is returned from every subsequent call.
    Failed(Error),
}

pub(crate) struct Core<'a> {
    inflate: &'a mut DecompressorOxide,
    window: &'a mut [u8; WINDOW],
    rows: &'a mut [u8; 2 * MAX_STRIDE],

    st: St,
    /// Header accumulator: chunk headers use 8 bytes, IHDR payload 13.
    hdr: [u8; 13],
    hdr_fill: usize,
    /// Payload (or CRC) bytes left in the current chunk.
    chunk_rem: u32,
    seen_ihdr: bool,
    seen_iend: bool,

    width: u32,
    height: u32,
    channels: u8,
    /// `width × channels` — bytes per row after the filter byte.
    stride: usize,

    /// zlib stream terminated (adler verified). Later IDAT bytes are
    /// ignored — some encoders pad, and they are framing-valid.
    zdone: bool,
    /// Ring-window write position, `< WINDOW`.
    out_pos: usize,

    /// Next raw byte is a row's filter-type byte.
    need_filter: bool,
    filter: u8,
    row_fill: usize,
    /// Rows fully emitted so far.
    rows_done: u32,
    /// Which half of `rows` is the row being assembled (0 or 1).
    cur: usize,
}

impl<'a> Core<'a> {
    pub(crate) fn new(
        inflate: &'a mut DecompressorOxide,
        window: &'a mut [u8; WINDOW],
        rows: &'a mut [u8; 2 * MAX_STRIDE],
    ) -> Self {
        inflate.init();
        // Zeroing keeps malformed streams deterministic (a back-reference
        // past written history reads zeros, not a previous image) and
        // establishes the all-zero "previous row" the first row filters
        // against.
        window.fill(0);
        rows.fill(0);
        Core {
            inflate,
            window,
            rows,
            st: St::Signature,
            hdr: [0; 13],
            hdr_fill: 0,
            chunk_rem: 0,
            seen_ihdr: false,
            seen_iend: false,
            width: 0,
            height: 0,
            channels: 0,
            stride: 0,
            zdone: false,
            out_pos: 0,
            need_filter: true,
            filter: 0,
            row_fill: 0,
            rows_done: 0,
            cur: 0,
        }
    }

    pub(crate) fn write(&mut self, data: &[u8], sink: &mut impl RowSink) -> Result<(), Error> {
        if let St::Failed(e) = self.st {
            return Err(e);
        }
        match self.write_inner(data, sink) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.st = St::Failed(e);
                Err(e)
            }
        }
    }

    /// Complete ⇔ IEND seen, zlib terminated, and every row delivered.
    /// (Requiring IEND makes "truncated anywhere" a clean error — the tail
    /// of a PNG is CRC + IEND, so any cut file is missing it.)
    pub(crate) fn complete(&self) -> Result<(), Error> {
        match self.st {
            St::Failed(e) => Err(e),
            St::Done if self.zdone && self.rows_done == self.height => Ok(()),
            _ => Err(Error::Truncated),
        }
    }

    fn write_inner(&mut self, mut data: &[u8], sink: &mut impl RowSink) -> Result<(), Error> {
        while !data.is_empty() {
            match self.st {
                St::Signature => {
                    let take = (SIGNATURE.len() - self.hdr_fill).min(data.len());
                    if data[..take] != SIGNATURE[self.hdr_fill..self.hdr_fill + take] {
                        return Err(Error::Signature);
                    }
                    self.hdr_fill += take;
                    data = &data[take..];
                    if self.hdr_fill == SIGNATURE.len() {
                        self.hdr_fill = 0;
                        self.st = St::ChunkHeader;
                    }
                }
                St::ChunkHeader => {
                    let take = (8 - self.hdr_fill).min(data.len());
                    self.hdr[self.hdr_fill..self.hdr_fill + take].copy_from_slice(&data[..take]);
                    self.hdr_fill += take;
                    data = &data[take..];
                    if self.hdr_fill == 8 {
                        self.hdr_fill = 0;
                        self.dispatch_chunk()?;
                    }
                }
                St::IhdrData => {
                    let take = (13 - self.hdr_fill).min(data.len());
                    self.hdr[self.hdr_fill..self.hdr_fill + take].copy_from_slice(&data[..take]);
                    self.hdr_fill += take;
                    data = &data[take..];
                    if self.hdr_fill == 13 {
                        self.hdr_fill = 0;
                        self.parse_ihdr(sink)?;
                        self.chunk_rem = 4;
                        self.st = St::Crc;
                    }
                }
                St::IdatData => {
                    let take = (self.chunk_rem as usize).min(data.len());
                    if !self.zdone {
                        self.feed_inflate(&data[..take], sink)?;
                    }
                    self.chunk_rem -= take as u32;
                    data = &data[take..];
                    if self.chunk_rem == 0 {
                        self.chunk_rem = 4;
                        self.st = St::Crc;
                    }
                }
                St::SkipData => {
                    let take = (self.chunk_rem as usize).min(data.len());
                    self.chunk_rem -= take as u32;
                    data = &data[take..];
                    if self.chunk_rem == 0 {
                        self.chunk_rem = 4;
                        self.st = St::Crc;
                    }
                }
                St::Crc => {
                    let take = (self.chunk_rem as usize).min(data.len());
                    self.chunk_rem -= take as u32;
                    data = &data[take..];
                    if self.chunk_rem == 0 {
                        self.st = if self.seen_iend {
                            St::Done
                        } else {
                            St::ChunkHeader
                        };
                    }
                }
                St::Done => return Ok(()),
                St::Failed(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// A chunk header is fully accumulated in `hdr[..8]`.
    fn dispatch_chunk(&mut self) -> Result<(), Error> {
        let len = u32::from_be_bytes([self.hdr[0], self.hdr[1], self.hdr[2], self.hdr[3]]);
        let kind = [self.hdr[4], self.hdr[5], self.hdr[6], self.hdr[7]];
        // PNG lengths are < 2³¹ by spec.
        if len > i32::MAX as u32 {
            return Err(Error::Malformed);
        }
        if !self.seen_ihdr && kind != IHDR {
            return Err(Error::Malformed);
        }
        match kind {
            IHDR => {
                if self.seen_ihdr || len != 13 {
                    return Err(Error::Malformed);
                }
                self.seen_ihdr = true;
                self.st = St::IhdrData;
            }
            IDAT => {
                self.begin_payload(len, St::IdatData);
            }
            IEND => {
                if len != 0 {
                    return Err(Error::Malformed);
                }
                self.seen_iend = true;
                self.chunk_rem = 4;
                self.st = St::Crc;
            }
            // Ancillary and out-of-scope chunks (PLTE, tRNS, gAMA, …) are
            // skipped whole — crate-doc scope decision.
            _ => {
                self.begin_payload(len, St::SkipData);
            }
        }
        Ok(())
    }

    fn begin_payload(&mut self, len: u32, st: St) {
        if len == 0 {
            self.chunk_rem = 4;
            self.st = St::Crc;
        } else {
            self.chunk_rem = len;
            self.st = st;
        }
    }

    /// The IHDR payload is fully accumulated in `hdr[..13]`.
    fn parse_ihdr(&mut self, sink: &mut impl RowSink) -> Result<(), Error> {
        let w = u32::from_be_bytes([self.hdr[0], self.hdr[1], self.hdr[2], self.hdr[3]]);
        let h = u32::from_be_bytes([self.hdr[4], self.hdr[5], self.hdr[6], self.hdr[7]]);
        let depth = self.hdr[8];
        let color = self.hdr[9];
        let compression = self.hdr[10];
        let filter_method = self.hdr[11];
        let interlace = self.hdr[12];

        if w == 0 || h == 0 {
            return Err(Error::Malformed);
        }
        if w > MAX_DIM as u32 || h > MAX_DIM as u32 {
            return Err(Error::TooLarge);
        }
        if depth != 8 {
            return Err(Error::Unsupported);
        }
        let channels = match color {
            2 => 3u8,
            6 => 4u8,
            0 | 3 | 4 => return Err(Error::Unsupported),
            _ => return Err(Error::Malformed),
        };
        // Only method 0 exists for either; anything else is spec-invalid.
        if compression != 0 || filter_method != 0 {
            return Err(Error::Malformed);
        }
        match interlace {
            0 => {}
            1 => return Err(Error::Unsupported), // Adam7 — out of scope
            _ => return Err(Error::Malformed),
        }

        self.width = w;
        self.height = h;
        self.channels = channels;
        self.stride = w as usize * channels as usize; // ≤ 1024·4 = MAX_STRIDE
        sink.start(w, h, channels);
        Ok(())
    }

    /// Push IDAT payload bytes through inflate, draining the ring window
    /// into the row assembler after every call.
    fn feed_inflate(&mut self, mut data: &[u8], sink: &mut impl RowSink) -> Result<(), Error> {
        loop {
            let (status, consumed, produced) =
                decompress(self.inflate, data, self.window, self.out_pos, INFLATE_FLAGS);
            let start = self.out_pos;
            data = &data[consumed..];
            self.out_pos += produced;
            self.consume_raw(start, produced, sink)?;
            if self.out_pos == WINDOW {
                self.out_pos = 0;
            }
            match status {
                TINFLStatus::Done => {
                    // Trailing bytes inside IDAT after the zlib stream are
                    // ignored (crate doc); adler32 already verified.
                    self.zdone = true;
                    return Ok(());
                }
                TINFLStatus::NeedsMoreInput => return Ok(()),
                TINFLStatus::HasMoreOutput => {
                    // Window was full; it is drained now. A full-window
                    // call that moved nothing would spin — bail instead
                    // (cannot happen per miniz semantics; defensive).
                    if produced == 0 && consumed == 0 {
                        return Err(Error::Deflate);
                    }
                }
                // Adler32Mismatch, Failed, BadParam, FailedCannotMakeProgress
                _ => return Err(Error::Deflate),
            }
        }
    }

    /// Hand `window[start .. start+len]` (freshly decompressed raw filter
    /// bytes) to the row assembler.
    fn consume_raw(
        &mut self,
        start: usize,
        len: usize,
        sink: &mut impl RowSink,
    ) -> Result<(), Error> {
        let mut pos = start;
        let end = start + len;
        while pos < end {
            if self.rows_done >= self.height {
                // More raw bytes than height·(1+stride) — not a valid
                // encoding of the announced dimensions.
                return Err(Error::Malformed);
            }
            if self.need_filter {
                self.filter = self.window[pos];
                pos += 1;
                if self.filter > 4 {
                    return Err(Error::Malformed);
                }
                self.need_filter = false;
                continue;
            }
            let take = (self.stride - self.row_fill).min(end - pos);
            let base = self.cur * MAX_STRIDE + self.row_fill;
            self.rows[base..base + take].copy_from_slice(&self.window[pos..pos + take]);
            self.row_fill += take;
            pos += take;
            if self.row_fill == self.stride {
                self.defilter();
                let base = self.cur * MAX_STRIDE;
                sink.row(self.rows_done, &self.rows[base..base + self.stride]);
                self.rows_done += 1;
                self.cur ^= 1;
                self.row_fill = 0;
                self.need_filter = true;
            }
        }
        Ok(())
    }

    /// Reverse the row's filter in place, against the previous defiltered
    /// row (all-zero for the first row — `rows` starts zeroed).
    fn defilter(&mut self) {
        let bpp = self.channels as usize;
        let stride = self.stride;
        let (lo, hi) = self.rows.split_at_mut(MAX_STRIDE);
        let (cur, prev) = if self.cur == 0 {
            (&mut lo[..stride], &hi[..stride])
        } else {
            (&mut hi[..stride], &lo[..stride])
        };
        match self.filter {
            0 => {}
            1 => {
                // Sub: predictor = left.
                for i in bpp..stride {
                    cur[i] = cur[i].wrapping_add(cur[i - bpp]);
                }
            }
            2 => {
                // Up: predictor = above.
                for (c, p) in cur.iter_mut().zip(prev) {
                    *c = c.wrapping_add(*p);
                }
            }
            3 => {
                // Average: predictor = (left + above) / 2; left = 0 for
                // the first pixel.
                for i in 0..bpp {
                    cur[i] = cur[i].wrapping_add(prev[i] >> 1);
                }
                for i in bpp..stride {
                    let sum = (cur[i - bpp] as u16 + prev[i] as u16) >> 1;
                    cur[i] = cur[i].wrapping_add(sum as u8);
                }
            }
            4 => {
                // Paeth; with left = upper-left = 0 the predictor
                // degenerates to `above` for the first pixel.
                for i in 0..bpp {
                    cur[i] = cur[i].wrapping_add(prev[i]);
                }
                for i in bpp..stride {
                    let p = paeth(cur[i - bpp], prev[i], prev[i - bpp]);
                    cur[i] = cur[i].wrapping_add(p);
                }
            }
            // consume_raw rejected anything > 4 before assembling the row.
            _ => unreachable!(),
        }
    }
}

/// The Paeth predictor (PNG spec §9.4). All quantities fit i16:
/// a + b − c ∈ [−255, 510].
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

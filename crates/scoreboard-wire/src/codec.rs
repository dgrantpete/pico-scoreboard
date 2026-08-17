//! The two halves of every payload: a byte sink for encoding and a bounds-
//! checked cursor for decoding. Neither knows anything about games.

use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{MAX_STRING_BYTES, TeamColors};

/// Somewhere encoded bytes go. Implemented for [`SliceSink`] and — with the
/// `alloc` feature — `Vec<u8>`.
pub trait Sink {
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferFull>;
}

/// Encodes into a caller-owned buffer, the no-alloc path.
pub struct SliceSink<'a> {
    buf: &'a mut [u8],
    written: usize,
}

impl<'a> SliceSink<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, written: 0 }
    }

    /// The encoded payload so far.
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.written]
    }

    pub fn len(&self) -> usize {
        self.written
    }

    pub fn is_empty(&self) -> bool {
        self.written == 0
    }
}

impl Sink for SliceSink<'_> {
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferFull> {
        let end = self.written.checked_add(bytes.len()).ok_or(BufferFull)?;
        let room = self.buf.get_mut(self.written..end).ok_or(BufferFull)?;
        room.copy_from_slice(bytes);
        self.written = end;
        Ok(())
    }
}

#[cfg(feature = "alloc")]
impl Sink for alloc::vec::Vec<u8> {
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferFull> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// Field-shaped writes over a [`Sink`]. Blanket-implemented; crate-private
/// because the public surface is `encode`, not a byte API.
pub(crate) trait SinkExt: Sink {
    fn u8(&mut self, value: u8) -> Result<(), BufferFull> {
        self.write_bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), BufferFull> {
        self.write_bytes(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), BufferFull> {
        self.write_bytes(&value.to_le_bytes())
    }

    /// Away pair then home pair — every payload carries them in that order.
    fn color_pairs(&mut self, away: TeamColors, home: TeamColors) -> Result<(), BufferFull> {
        self.u32(away.primary)?;
        self.u32(away.alternate)?;
        self.u32(home.primary)?;
        self.u32(home.alternate)
    }

    /// One `u8` length + UTF-8 bytes, truncated at a char boundary when the
    /// text exceeds what the length prefix can describe.
    fn string(&mut self, text: &str) -> Result<(), BufferFull> {
        let bytes = truncate_utf8(text, MAX_STRING_BYTES).as_bytes();
        self.u8(bytes.len() as u8)?;
        self.write_bytes(bytes)
    }
}

impl<S: Sink + ?Sized> SinkExt for S {}

/// Truncate to at most `max` bytes without splitting a UTF-8 char.
/// (`str::floor_char_boundary` is nightly-only, hence the manual walk.)
///
/// Public because `scoreboard-espn` copies strings into its bounded extract
/// structs with *this exact function* — one implementation is what makes
/// truncate-at-copy byte-identical to truncate-at-encode (its DESIGN.md,
/// ruling 2).
pub fn truncate_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut n = max;
    while !text.is_char_boundary(n) {
        n -= 1;
    }
    &text[..n]
}

/// A bounds-checked cursor over a payload. Every read either advances past
/// validated bytes or returns a [`DecodeError`] carrying the current offset —
/// there is no panicking path, which is what lets the firmware decode straight
/// out of a network buffer.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    /// Skip the already-validated 2-byte header.
    pub(crate) fn skip_header(&mut self) {
        self.offset = crate::HEADER_LEN;
    }

    /// The payload's fixed numeric section, as a fixed-size array so field
    /// reads are plain indexing against the layout tables.
    pub(crate) fn fixed<const N: usize>(&mut self) -> Result<&'a [u8; N], DecodeError> {
        let end = self.offset + N;
        let fixed: &'a [u8; N] = self
            .buf
            .get(self.offset..end)
            .and_then(|slice| slice.try_into().ok())
            .ok_or_else(|| {
                DecodeError::at(
                    self.offset,
                    DecodeErrorKind::TruncatedFixed {
                        need: end,
                        have: self.buf.len(),
                    },
                )
            })?;
        self.offset = end;
        Ok(fixed)
    }

    /// The two per-team line-score runs, whose lengths the fixed section
    /// carries. Both are checked together, as the firmware parsers do.
    pub(crate) fn line_scores(
        &mut self,
        away_len: usize,
        home_len: usize,
    ) -> Result<(&'a [u8], &'a [u8]), DecodeError> {
        let need = away_len + home_len;
        let have = self.buf.len() - self.offset;
        if need > have {
            return Err(DecodeError::at(
                self.offset,
                DecodeErrorKind::TruncatedLineScores { need, have },
            ));
        }
        let away = &self.buf[self.offset..self.offset + away_len];
        self.offset += away_len;
        let home = &self.buf[self.offset..self.offset + home_len];
        self.offset += home_len;
        Ok((away, home))
    }

    /// One `u8`-length-prefixed UTF-8 string, borrowed from the payload.
    ///
    /// The bytes are validated as UTF-8 but otherwise untouched: normalizing to
    /// a display font's repertoire is the renderer's job, and a borrow keeps
    /// decode allocation-free.
    pub(crate) fn string(&mut self, field: &'static str) -> Result<&'a str, DecodeError> {
        let len =
            *self.buf.get(self.offset).ok_or_else(|| {
                DecodeError::at(self.offset, DecodeErrorKind::TruncatedLength(field))
            })? as usize;
        self.offset += 1;
        let end = self.offset + len;
        let bytes = self.buf.get(self.offset..end).ok_or_else(|| {
            DecodeError::at(
                self.offset,
                DecodeErrorKind::TruncatedString {
                    field,
                    need: len,
                    have: self.buf.len() - self.offset,
                },
            )
        })?;
        let text = core::str::from_utf8(bytes)
            .map_err(|_| DecodeError::at(self.offset, DecodeErrorKind::InvalidUtf8(field)))?;
        self.offset = end;
        Ok(text)
    }

    pub(crate) fn u8(&mut self, field: &'static str) -> Result<u8, DecodeError> {
        let value = *self
            .buf
            .get(self.offset)
            .ok_or_else(|| DecodeError::at(self.offset, DecodeErrorKind::Truncated(field)))?;
        self.offset += 1;
        Ok(value)
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.offset
    }

    /// No trailing bytes may follow the last field.
    pub(crate) fn check_end(&self) -> Result<(), DecodeError> {
        let extra = self.remaining();
        if extra == 0 {
            Ok(())
        } else {
            Err(DecodeError::at(
                self.offset,
                DecodeErrorKind::Trailing(extra),
            ))
        }
    }

    /// [`Reader::check_end`] for the common case where the reader is done.
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        self.check_end()
    }
}

/// Little-endian `u16` at `index` of a fixed section.
pub(crate) fn le_u16<const N: usize>(fixed: &[u8; N], index: usize) -> u16 {
    u16::from_le_bytes([fixed[index], fixed[index + 1]])
}

/// Little-endian `u32` at `index` of a fixed section.
pub(crate) fn le_u32<const N: usize>(fixed: &[u8; N], index: usize) -> u32 {
    u32::from_le_bytes([
        fixed[index],
        fixed[index + 1],
        fixed[index + 2],
        fixed[index + 3],
    ])
}

/// The away/home color pairs that close every fixed section.
pub(crate) fn color_pairs<const N: usize>(
    fixed: &[u8; N],
    index: usize,
) -> (TeamColors, TeamColors) {
    (
        TeamColors {
            primary: le_u32(fixed, index),
            alternate: le_u32(fixed, index + 4),
        },
        TeamColors {
            primary: le_u32(fixed, index + 8),
            alternate: le_u32(fixed, index + 12),
        },
    )
}

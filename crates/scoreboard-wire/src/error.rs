//! Wire errors. Decode failures carry the byte offset they were detected at and
//! render as `@29: truncated inside game_id: need 9 bytes, have 3` — the same
//! diagnostic shape the MicroPython parsers have used since v1, which is worth
//! keeping: on a device the offset is often the only clue available.

use core::fmt;

use crate::WIRE_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// Byte offset into the payload where the mismatch was detected.
    pub offset: usize,
    pub kind: DecodeErrorKind,
}

impl DecodeError {
    pub(crate) fn at(offset: usize, kind: DecodeErrorKind) -> Self {
        Self { offset, kind }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeErrorKind {
    /// Zero-length payload — not even a version byte.
    Empty,
    /// A stray JSON body (`{`, `[`) trips this on its first byte.
    UnsupportedVersion(u8),
    /// A tagged byte the payload ended before ("state byte", "game state").
    Truncated(&'static str),
    UnknownState(u8),
    /// The fixed numeric section runs past the end of the payload.
    TruncatedFixed {
        need: usize,
        have: usize,
    },
    /// No room left for a string's length byte.
    TruncatedLength(&'static str),
    TruncatedString {
        field: &'static str,
        need: usize,
        have: usize,
    },
    InvalidUtf8(&'static str),
    TruncatedLineScores {
        need: usize,
        have: usize,
    },
    /// An enum-valued byte outside its known set (inning half, live phase,
    /// soccer period, full-time flavor, game state).
    InvalidCode {
        field: &'static str,
        code: u8,
    },
    /// A well-formed payload followed by bytes nothing claims.
    Trailing(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}: ", self.offset)?;
        match self.kind {
            DecodeErrorKind::Empty => write!(f, "empty payload"),
            DecodeErrorKind::UnsupportedVersion(found) => write!(
                f,
                "unsupported wire version {found} (expected {WIRE_VERSION})"
            ),
            DecodeErrorKind::Truncated(field) => write!(f, "truncated before {field}"),
            DecodeErrorKind::UnknownState(state) => write!(f, "unknown game state {state}"),
            DecodeErrorKind::TruncatedFixed { need, have } => {
                write!(f, "truncated fixed section: {have} < {need}")
            }
            DecodeErrorKind::TruncatedLength(field) => {
                write!(f, "truncated before {field} length")
            }
            DecodeErrorKind::TruncatedString { field, need, have } => {
                write!(
                    f,
                    "truncated inside {field}: need {need} bytes, have {have}"
                )
            }
            DecodeErrorKind::InvalidUtf8(field) => write!(f, "invalid UTF-8 in {field}"),
            DecodeErrorKind::TruncatedLineScores { need, have } => {
                write!(f, "truncated linescores: need {need} bytes, have {have}")
            }
            DecodeErrorKind::InvalidCode { field, code } => {
                write!(f, "invalid {field} code: {code}")
            }
            DecodeErrorKind::Trailing(extra) => write!(f, "{extra} unexpected trailing bytes"),
        }
    }
}

impl core::error::Error for DecodeError {}

/// The only way encoding fails: the caller's buffer ran out. Field-level
/// overflow is not an error — the format truncates (see [`crate::MAX_STRING_BYTES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferFull;

impl fmt::Display for BufferFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("encode buffer full")
    }
}

impl core::error::Error for BufferFull {}

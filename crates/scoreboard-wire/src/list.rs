//! The games list (`GET /{sport}/{league}/games`), identical for every sport:
//! `u8 version = 2`, `u8 count`, then per game `u8 state` + length-prefixed
//! `id`. Entries stay in backend (chronological) order. The ETag /
//! If-None-Match / 304 flow around it is format-independent.

use crate::codec::{Reader, Sink, SinkExt};
use crate::common::read_header_version;
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{GameState, MAX_GAMES, WIRE_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub state: GameState,
    pub id: &'a str,
}

/// Encode a games list. Entries past [`MAX_GAMES`] don't fit the `u8` count and
/// are dropped — callers that can plausibly exceed it should say so upstream.
pub fn encode<S: Sink + ?Sized>(entries: &[Entry<'_>], out: &mut S) -> Result<(), BufferFull> {
    let entries = &entries[..entries.len().min(MAX_GAMES)];
    out.u8(WIRE_VERSION)?;
    out.u8(entries.len() as u8)?;
    for entry in entries {
        out.u8(entry.state.code())?;
        out.string(entry.id)?;
    }
    Ok(())
}

/// Decode a games list into a lazy iterator of borrowed entries.
///
/// Nothing is buffered, so the crate imposes no cap on list length: a consumer
/// that needs to own the entries picks its own bounded storage and can size it
/// from [`Iter::remaining`] before the first `next()`.
pub fn decode(buf: &[u8]) -> Result<Iter<'_>, DecodeError> {
    read_header_version(buf)?;
    let count = *buf
        .get(1)
        .ok_or_else(|| DecodeError::at(1, DecodeErrorKind::Truncated("game count")))?;
    let mut reader = Reader::new(buf);
    reader.skip_header();
    Ok(Iter {
        reader,
        remaining: count as usize,
        stopped: false,
    })
}

/// Yields one `Result` per declared entry, then — once they are all read — a
/// final `Err` if the payload carries bytes nothing claimed. The first error
/// ends iteration.
pub struct Iter<'a> {
    reader: Reader<'a>,
    remaining: usize,
    stopped: bool,
}

impl Iter<'_> {
    /// Entries not yet yielded; equals the payload's declared count before the
    /// first `next()`.
    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = Result<Entry<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.stopped {
            return None;
        }
        if self.remaining == 0 {
            self.stopped = true;
            return self.reader.check_end().err().map(Err);
        }
        match self.read_entry() {
            Ok(entry) => {
                self.remaining -= 1;
                Some(Ok(entry))
            }
            Err(error) => {
                self.stopped = true;
                Some(Err(error))
            }
        }
    }
}

impl<'a> Iter<'a> {
    fn read_entry(&mut self) -> Result<Entry<'a>, DecodeError> {
        let at = self.reader.offset();
        let code = self.reader.u8("game state")?;
        let state = GameState::from_code(code).ok_or_else(|| {
            DecodeError::at(
                at,
                DecodeErrorKind::InvalidCode {
                    field: "game state",
                    code,
                },
            )
        })?;
        Ok(Entry {
            state,
            id: self.reader.string("game id")?,
        })
    }
}

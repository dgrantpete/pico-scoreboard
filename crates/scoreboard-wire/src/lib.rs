#![no_std]
//! The packed binary wire format the backend serves and the scoreboard firmware
//! decodes — **this crate is the normative spec**, and the only implementation
//! of it. Layout tables live in the per-sport modules; this page covers what
//! they share.
//!
//! Content negotiation: the client sends `Accept: application/x-scoreboard-struct`
//! ([`STRUCT_CONTENT_TYPE`]) and the server responds with that content type;
//! otherwise JSON is served. Error responses (4xx/5xx) are always JSON. All
//! integers are little-endian. Strings are `u8 length` + UTF-8 bytes, truncated
//! at a char boundary if they exceed [`MAX_STRING_BYTES`]. No trailing bytes
//! follow the last field.
//!
//! `WIRE_VERSION = 2` and is **frozen**: devices in the field decode this
//! layout, so a byte-level change is a breaking change, not a refactor.
//!
//! # Header
//!
//! Every game detail opens with the same 2 bytes: `u8 version = 2`, `u8 state`
//! ([`GameState`]: `0 = pregame`, `1 = live`, `2 = final`). The variant payload
//! follows at offset 2. The firmware picks the *sport* parser from the endpoint
//! it polled, not by sniffing bytes; only the state is discriminated on the
//! wire. The games list ([`list`]) uses the same version byte and state codes.
//!
//! # Both directions
//!
//! [`Sink`]-based `encode` (the backend's side) and borrowing `decode` (the
//! firmware's side) are defined together per sport so they cannot drift.
//! Decoded strings and line scores borrow the caller's receive buffer: nothing
//! here allocates, and no field is owned. Copying what outlives the buffer is
//! the caller's decision — the firmware's snapshot has bounded owned fields for
//! exactly that, so this crate never has to guess a capacity.

#[cfg(feature = "alloc")]
extern crate alloc;

/// Host tests link `std` for their scratch buffers and hex formatting; the
/// library itself never does.
#[cfg(test)]
extern crate std;

mod codec;
mod common;
#[cfg(test)]
mod tests;

pub mod error;
pub mod football;
pub mod list;
pub mod mlb;
pub mod nba;
pub mod soccer;

pub use codec::{Sink, SliceSink, truncate_utf8};
pub use common::{
    FinalTeam, GameState, LastPlay, LivePhase, Record, Side, TeamColors, TeamState,
    clamp_temperature, saturate_score,
};
pub use error::{BufferFull, DecodeError, DecodeErrorKind};

/// The `Accept` / `Content-Type` value that selects this format over JSON.
pub const STRUCT_CONTENT_TYPE: &str = "application/x-scoreboard-struct";

pub const WIRE_VERSION: u8 = 2;

/// `u8 version` + `u8 state`.
pub const HEADER_LEN: usize = 2;

/// The `u8` length prefix caps every string; longer text is truncated at a
/// char boundary when encoded.
pub const MAX_STRING_BYTES: usize = 255;

/// The same `u8` prefix caps a line score. A real game never approaches it —
/// the cap is a safety net, and encoding truncates rather than corrupt the
/// length/payload agreement.
pub const MAX_LINE_SCORE: usize = 255;

/// A games list carries a `u8` count, so at most 255 entries encode.
pub const MAX_GAMES: usize = 255;

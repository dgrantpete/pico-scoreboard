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
//! # Extensions
//!
//! New data reaches new devices without touching the frozen layout. The
//! `Accept` media type carries an extension **level**, `ext=N`
//! (`Accept: application/x-scoreboard-struct; ext=2`): the server appends,
//! after a payload's last v2 field, the blocks that payload's module defines
//! for each level up to the **highest it knows, capped by the request** — a
//! device asking for a level the backend predates gets the highest the backend
//! has — and answers with the plain [`STRUCT_CONTENT_TYPE`] either way. Without
//! the parameter the bytes are exactly v2: the gift fleet and every firmware
//! before 2026-10-12 never send it. Each level's block follows the one before,
//! so a payload is v2, then the `ext=1` block, then the `ext=2` block, and a
//! decoder **accepts a payload that ends after any complete block**, with the
//! groups a shorter payload does not carry absent. That is what lets any
//! backend answer any device: an older backend's plain v2 or `ext=1` decodes
//! on a newer device, and a newer backend never has to know which device asked.
//!
//! Only [`basketball`] defines levels — `ext=1` its pregame team lines, `ext=2` its
//! live, pregame and final blocks — so [`EXT_LEVEL_MAX`] is its ceiling. Its
//! `encode_at(game, level, out)` writes a payload at a level, `encode` is level
//! 0 by name, and `decode` accepts every level.
//! The other modules' `encode` writes plain v2 whatever the request, and a new
//! sport needs no extension: no deployed decoder has ever requested its
//! endpoint, so its first v2 layout carries everything ([`hockey`]).
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
//! Decoded strings, line scores and hockey's goal records borrow the caller's
//! receive buffer: nothing here allocates, and no field is owned. Copying what
//! outlives the buffer is the caller's decision — the firmware's snapshot has
//! bounded owned fields for exactly that, so this crate never has to guess a
//! capacity.

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

pub mod basketball;
pub mod error;
pub mod football;
pub mod hockey;
pub mod list;
pub mod mlb;
pub mod soccer;

pub use codec::{Sink, SliceSink, truncate_utf8};
pub use common::{
    FinalTeam, GameState, LastPlay, LivePhase, Record, Side, TeamColors, TeamState,
    clamp_temperature, saturate_score,
};
pub use error::{BufferFull, DecodeError, DecodeErrorKind};

/// The `Accept` / `Content-Type` value that selects this format over JSON.
pub const STRUCT_CONTENT_TYPE: &str = "application/x-scoreboard-struct";

/// The `Accept` value that asks for the highest extension level this crate
/// defines — what the shipping Rust firmware sends (level 2 since the NBA
/// court screens, NBA.md phase 6.6; level 1 from 2026-10-12 until then). It
/// contains [`STRUCT_CONTENT_TYPE`], so a server that predates extensions
/// still serves plain v2 to it, and one that knows a lower level serves that
/// level: either decodes, with the groups it lacks absent.
pub const STRUCT_CONTENT_TYPE_EXT: &str = "application/x-scoreboard-struct; ext=2";

/// The name of the media-type parameter that carries the extension level:
/// `ext=N` on a [`STRUCT_CONTENT_TYPE`] media range (crate docs). The backend
/// reads the number after it and caps it at [`EXT_LEVEL_MAX`]; a range without
/// the parameter, or with one that is not a number, is level 0.
pub const EXT_PARAM_NAME: &str = "ext";

/// The highest extension level any module defines (see the crate docs): a
/// server caps a request's level here, and [`basketball::encode_at`] writes this
/// level for anything above it.
pub const EXT_LEVEL_MAX: u8 = 2;

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

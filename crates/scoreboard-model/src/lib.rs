#![no_std]
//! The scoreboard's display state: what the four sports mean, what the panel
//! shows, and the rules for getting from one to the other.
//!
//! This is the port of `firmware/src/scoreboard/state.py` (plus the semantic
//! halves of the sport parsers, `textfold.py`, and `poller.py`'s slate
//! handling). It owns:
//!
//! - [`ScoreboardSnapshot`] — the core-0 → core-1 handoff, bounded and owned,
//!   with no borrow into any receive buffer.
//! - [`Store`] — the authoritative state and every rule for changing it,
//!   including the view-identity rule that decides when an animation restarts.
//! - [`Slate`] — the merged games list and the live-first rotation over it.
//! - [`GameFeed`] — the one seam upstream data arrives through, so Phase S's
//!   direct-to-ESPN mode plugs in without touching this crate.
//!
//! No hardware, no embassy, no allocator: everything here is `core` plus
//! `heapless`, and every rule is exercised by host tests.

#[cfg(test)]
extern crate std;

pub mod channel;
pub mod color;
pub mod feed;
pub mod slate;
pub mod snapshot;
pub mod sports;
pub mod store;
pub mod text;

#[cfg(test)]
mod tests;

pub use channel::{Publisher, Reader, SnapshotChannel};
pub use color::{Rgb888, UiColors};
pub use feed::{GameDetail, GameFeed, LeagueId, ListSink, Sport, WireFeed};
pub use slate::Slate;
pub use snapshot::{Millis, Mode, ScoreboardSnapshot, SetupReason, ToastKind};
pub use sports::{LocalClock, PregameInput, PregameSideInput};
pub use store::{Logos, MenuRowInput, StartupExit, Store};
pub use text::Text;

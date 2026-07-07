//! MLB: ESPN inbound types, transform to the Pico-facing domain model, and
//! the game handlers. The JSON/binary wire shape here is consumed by the
//! firmware (`firmware/src/scoreboard/mlb.py`) — see `wire.rs` for the
//! binary layout contract.

pub mod handler;
pub mod transform;
mod types;

pub use handler::{get_live_game, list_active_games};
pub use types::{
    AtBat, Bases, Count, Inning, InningHalf, LastPlay, LiveGame, PregameGame, TeamState,
};

pub use crate::shared::team::TeamColors;

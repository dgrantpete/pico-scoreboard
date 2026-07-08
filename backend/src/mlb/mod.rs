//! MLB: ESPN inbound types, transform to the Pico-facing domain model, and
//! the game handlers. The JSON/binary wire shape here is consumed by the
//! firmware (`firmware/src/scoreboard/mlb.py`) — see `wire.rs` for the
//! binary layout contract.

pub mod handler;
pub mod transform;
mod types;

pub use handler::{get_game, list_games};
pub use types::{
    AtBat, Bases, Count, FinalGame, FinalTeam, GameListEntry, GameState, Inning, InningHalf,
    LastPlay, LiveGame, MlbGame, PregameGame, PregameTeam, Record, TeamState, Weather,
};

pub use crate::shared::team::TeamColors;

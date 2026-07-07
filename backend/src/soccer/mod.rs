//! Soccer: ESPN inbound types, transform to the display-facing domain model,
//! and the game handlers. JSON responses only for now — the binary wire
//! format is display-driven and lands with the firmware work.

pub mod handler;
pub mod transform;
mod types;

pub use handler::{get_live_game, list_active_games};
pub use types::{LastEvent, Side, SoccerGame, SoccerTeam, SoccerTeamState};

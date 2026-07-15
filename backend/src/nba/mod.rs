//! NBA: ESPN inbound types, transform to the display-facing domain model,
//! and the game handlers. Responses content-negotiate JSON (debugging) vs
//! the packed binary wire format (firmware) — see `wire.rs` for the layout
//! contract, mirrored by `firmware/src/scoreboard/nba.py`.

pub mod handler;
pub mod transform;
mod types;

pub use handler::{get_game, list_games};
pub use types::{
    LivePhase, NbaFinalGame, NbaFinalTeam, NbaGame, NbaLastPlay, NbaLiveGame, NbaPregameGame,
    NbaTeam,
};

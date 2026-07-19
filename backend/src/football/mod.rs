//! Football (NFL + NCAAF): ESPN inbound types, transform to the display-facing
//! domain model, and the game handlers. Multi-league like soccer — both
//! leagues share the ESPN sport slug "football" and this one module. Responses
//! content-negotiate JSON (debugging) vs the packed binary wire format
//! (firmware) — see `wire.rs` for the layout contract, mirrored by
//! `firmware/src/scoreboard/football.py`.

pub mod handler;
pub mod transform;
mod types;

pub use handler::{get_game, list_games};
pub use types::{
    FootballFinalGame, FootballFinalTeam, FootballGame, FootballLiveGame, FootballPregameGame,
    FootballPregameTeam, FootballSituation, Timeouts,
};

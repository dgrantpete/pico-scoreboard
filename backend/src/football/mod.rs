//! Football (NFL + NCAAF): game handlers over the shared `scoreboard-espn`
//! extraction, the display-facing domain model, and its wire encoding.
//! Multi-league like soccer — both leagues share the ESPN sport slug
//! "football" and this one module. Responses content-negotiate JSON
//! (debugging) vs the packed binary wire format (firmware) — see `wire.rs`
//! for the layout contract, mirrored by `firmware/src/scoreboard/football.py`.

pub(crate) mod adapter;
pub mod handler;
pub(crate) mod types;
pub(crate) mod wire;

pub use handler::{get_game, list_games};
pub use types::{
    FootballFinalGame, FootballFinalTeam, FootballGame, FootballLiveGame, FootballPregameGame,
    FootballPregameTeam, FootballSituation, Timeouts,
};

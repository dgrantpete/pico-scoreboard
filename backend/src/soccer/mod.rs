//! Soccer: game handlers over the shared `scoreboard-espn` extraction
//! (scoreboard + per-event summary), the display-facing domain model, and
//! its wire encoding. Responses content-negotiate JSON (debugging) vs the
//! packed binary wire format (firmware) — see `wire.rs` for the layout
//! contract, mirrored by `firmware/src/scoreboard/soccer.py`.

pub(crate) mod adapter;
pub mod handler;
pub(crate) mod types;
pub(crate) mod wire;

pub use handler::{get_game, list_games};
pub use types::{
    Commentary, EventKind, LastEvent, SoccerFinalFlavor, SoccerFinalGame, SoccerFinalTeam,
    SoccerGame, SoccerLiveGame, SoccerPregameGame, SoccerPregameTeam,
};

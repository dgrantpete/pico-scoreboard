//! MLB: game handlers over the shared `scoreboard-espn` extraction, the
//! Pico-facing domain model, and its wire encoding. The JSON/binary wire
//! shape here is consumed by the firmware (`firmware/src/scoreboard/mlb.py`)
//! — see `wire.rs` for the binary layout contract.

pub(crate) mod adapter;
pub mod handler;
pub(crate) mod types;
pub(crate) mod wire;

pub use handler::{get_game, list_games};
pub use types::{
    InningHalf, MlbAtBat, MlbBases, MlbCount, MlbFinalGame, MlbFinalTeam, MlbGame, MlbInning,
    MlbLiveGame, MlbPregameGame, MlbPregameTeam, MlbWeather,
};

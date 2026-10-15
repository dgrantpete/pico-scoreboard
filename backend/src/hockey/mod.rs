//! NHL: game handlers over the shared `scoreboard-espn` extraction
//! (scoreboard + per-event summary), the display-facing domain model, and its
//! wire encoding. Responses content-negotiate JSON (debugging) vs the packed
//! binary wire format (firmware) — the layout contract is
//! `crates/scoreboard-wire/src/hockey.rs`, and its semantics
//! `firmware-rs/NHL.md` §3. Only the Rust firmware decodes it: the MicroPython
//! tree is frozen and never gets hockey (NHL.md D7).

pub(crate) mod adapter;
pub mod handler;
pub(crate) mod types;
pub(crate) mod wire;

pub use handler::{get_game, list_games};
pub use types::{
    GoalTag, HockeyCoordinate, HockeyFinalGame, HockeyFinalTeam, HockeyGame, HockeyGoal,
    HockeyLiveGame, HockeyPregameGame, HockeyPregameTeam, HockeyRecord, HockeyShootout,
    HockeyShots, HockeyStar, HockeyStars, HockeyStrength, ShootoutAttempt, StrengthSituation,
};

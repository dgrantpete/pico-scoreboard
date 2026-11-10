//! Basketball, every league (the NBA and NCAA men's, `BasketballLeague`):
//! game handlers over the shared `scoreboard-espn` extraction (scoreboard +
//! the per-event summary for live games), the display-facing domain model,
//! and its wire encoding. Responses content-negotiate JSON (debugging) vs the
//! packed binary wire format (firmware) — the layout contract is
//! `crates/scoreboard-wire/src/basketball.rs`, its semantics
//! `firmware-rs/NBA.md` §3 and `firmware-rs/NCAAB.md` §3. The NBA's frozen v2
//! layout is what the gift fleet's `firmware/src/scoreboard/nba.py` decodes;
//! everything since rides the extension levels the client's `Accept` names.

pub(crate) mod adapter;
pub mod handler;
pub(crate) mod types;
pub(crate) mod wire;

pub use handler::{get_game, list_games};
pub use types::{
    BasketballFinalGame, BasketballFinalTeam, BasketballGame, BasketballLeader, BasketballLeaders,
    BasketballLedger, BasketballLiveGame, BasketballMake, BasketballNewestScore,
    BasketballPregameGame, BasketballPregameTeam, BasketballRecords, BasketballRun,
    BasketballTeamLedger,
};

//! NBA outbound domain model — the JSON DTOs the handlers serialize and
//! `wire.rs` encodes. The inbound ESPN parse lives in
//! `crates/scoreboard-espn::nba`; `adapter.rs` bridges its extracts here.

use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::game::{LastPlay, LivePhase, Record};
use crate::shared::team::{TeamColors, TeamState};

/// One NBA game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (see `wire.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum NbaGame {
    Pregame(NbaPregameGame),
    Live(NbaLiveGame),
    Final(NbaFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, and (when available)
/// season records.
#[derive(Serialize, ToSchema)]
pub struct NbaPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies
    /// the device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    pub home: NbaPregameTeam,
    pub away: NbaPregameTeam,
}

/// Pre-game team: identity and pre-game context, but no score (there is no
/// game yet — a fake 0 would invite the firmware to render one).
#[derive(Serialize, ToSchema)]
pub struct NbaPregameTeam {
    /// Team abbreviation, e.g. "LAL" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
}

/// Live state snapshot for one NBA game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct NbaLiveGame {
    pub game_id: String,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    /// Raw ESPN clock, display-shaped: "10:08", "53.0" under a minute; "0.0"
    /// or a reset "12:00" during breaks (see `phase`). NBA's clock stops
    /// unpredictably and ESPN sends no clock-running signal, so unlike
    /// soccer there is no `clock_seconds` — the string is exact at poll time
    /// and must not be extrapolated.
    pub clock: String,
    pub phase: LivePhase,
    pub home: TeamState,
    pub away: TeamState,
    /// Absent before the opening tip.
    pub last_play: Option<LastPlay>,
}

/// Final snapshot: score, per-quarter line score, and quarters played (4, or
/// more for overtime). No explicit winner — it is derivable from the scores,
/// and a second copy could disagree with them on a glitch.
#[derive(Serialize, ToSchema)]
pub struct NbaFinalGame {
    pub game_id: String,
    pub periods_played: u8,
    pub home: NbaFinalTeam,
    pub away: NbaFinalTeam,
}

#[derive(Serialize, ToSchema)]
pub struct NbaFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Points per quarter, quarter 1 first; overtime periods extend past 4.
    pub line_score: Vec<u8>,
}

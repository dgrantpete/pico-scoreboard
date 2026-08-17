//! MLB outbound domain model — the JSON DTOs the handlers serialize and
//! `wire.rs` encodes. The inbound ESPN parse lives in
//! `crates/scoreboard-espn::mlb`; `adapter.rs` bridges its extracts here.

use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::game::{LastPlay, Record};
use crate::shared::team::{TeamColors, TeamState};

/// One MLB game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (see `wire.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum MlbGame {
    Pregame(MlbPregameGame),
    Live(MlbLiveGame),
    Final(MlbFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, and (when available)
/// weather, records, and probable pitchers.
#[derive(Serialize, ToSchema)]
pub struct MlbPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies
    /// the device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    /// Absent when ESPN's weather block is missing or unusable.
    pub weather: Option<MlbWeather>,
    pub home: MlbPregameTeam,
    pub away: MlbPregameTeam,
}

#[derive(Serialize, ToSchema)]
pub struct MlbWeather {
    pub condition: String,
    pub temperature: i16,
}

/// Pre-game team: identity and pre-game context, but no score (there is no
/// game yet — a fake 0 would invite the firmware to render one).
#[derive(Serialize, ToSchema)]
pub struct MlbPregameTeam {
    /// Team abbreviation, e.g. "BOS" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
    /// Probable starting pitcher's short name, e.g. "G. Marquez".
    pub probable_pitcher: Option<String>,
}

/// Final snapshot: score, per-inning line score, and innings played (9, or
/// more for extras). No explicit winner — it is derivable from the scores, and
/// a second copy could disagree with them on a glitch.
#[derive(Serialize, ToSchema)]
pub struct MlbFinalGame {
    pub game_id: String,
    pub innings_played: u8,
    pub home: MlbFinalTeam,
    pub away: MlbFinalTeam,
}

#[derive(Serialize, ToSchema)]
pub struct MlbFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Runs per inning, inning 1 first. Per-team lengths are independent: a
    /// walk-off leaves the home line short, extras run past 9.
    pub line_score: Vec<u8>,
}

/// Live state snapshot for one MLB game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct MlbLiveGame {
    pub game_id: String,
    pub inning: MlbInning,
    pub home: TeamState,
    pub away: TeamState,
    pub count: MlbCount,
    pub bases: MlbBases,
    /// Absent between innings or before an at-bat starts.
    pub at_bat: Option<MlbAtBat>,
    pub last_play: LastPlay,
}

#[derive(Serialize, ToSchema)]
pub struct MlbCount {
    pub balls: u8,
    pub strikes: u8,
    pub outs: u8,
}

#[derive(Serialize, ToSchema)]
pub struct MlbBases {
    pub first: bool,
    pub second: bool,
    pub third: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MlbAtBat {
    pub pitcher: String,
    pub batter: String,
}

#[derive(Serialize, ToSchema)]
pub struct MlbInning {
    pub number: u8,
    pub half: InningHalf,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InningHalf {
    Top,
    Middle,
    Bottom,
    End,
}

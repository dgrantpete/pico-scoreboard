//! Football outbound domain model — the JSON DTOs the handlers serialize
//! and `wire.rs` encodes. The inbound ESPN parse lives in
//! `crates/scoreboard-espn::football`; `adapter.rs` bridges its extracts here.

use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::game::{LastPlay, LivePhase, Record, Side};
use crate::shared::team::{TeamColors, TeamState};

/// One football game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (see `wire.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum FootballGame {
    Pregame(FootballPregameGame),
    Live(FootballLiveGame),
    Final(FootballFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, season records, and
/// (college only) the AP/Coaches rank line.
#[derive(Serialize, ToSchema)]
pub struct FootballPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies the
    /// device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    pub home: FootballPregameTeam,
    pub away: FootballPregameTeam,
}

/// Pre-game team: identity, colors, season record, and the college rank line.
#[derive(Serialize, ToSchema)]
pub struct FootballPregameTeam {
    /// Team abbreviation, e.g. "KC" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
    /// Display-shaped poll line ("#3 OHIO STATE"); college only and only when
    /// ranked. Rides the wire's pitcher slot (the record travels numerically).
    pub rank_line: Option<String>,
}

/// Live state snapshot for one football game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct FootballLiveGame {
    pub game_id: String,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    /// Raw ESPN clock, display-shaped ("12:00", "0:37"); meaningless during
    /// breaks (render by `phase`). Like NBA — and unlike soccer — football's
    /// clock stops with no running signal from ESPN, so the string is exact at
    /// poll time and must not be extrapolated.
    pub clock: String,
    pub phase: LivePhase,
    pub home: TeamState,
    pub away: TeamState,
    /// The current down/distance/ball spot; absent between plays (and whenever
    /// ESPN's situation fails validation in `scoreboard-espn::football`).
    pub situation: Option<FootballSituation>,
    /// Remaining timeouts per side; absent when ESPN hasn't populated them.
    pub timeouts: Option<Timeouts>,
    /// Absent before the opening snap.
    pub last_play: Option<LastPlay>,
}

/// The current offensive situation. Present only for a well-formed snap; the
/// extraction drops a half-formed one rather than misdraw the field markers.
#[derive(Serialize, ToSchema)]
pub struct FootballSituation {
    /// 1st–4th down (validated into 1..=4).
    pub down: u8,
    /// Yards to the first-down line.
    pub distance: u8,
    /// Ball spot as an absolute 0–100 yard line.
    pub yard_line: u8,
    pub possession: Side,
    pub red_zone: bool,
}

/// Remaining timeouts for both sides. All-or-nothing on the wire (one "timeouts
/// present" flag), because ESPN populates both counts together or neither.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct Timeouts {
    pub away: u8,
    pub home: u8,
}

/// Final snapshot: score, per-quarter line score, and quarters played (4, or
/// more for overtime). Byte-identical to the NBA final on the wire.
#[derive(Serialize, ToSchema)]
pub struct FootballFinalGame {
    pub game_id: String,
    pub periods_played: u8,
    pub home: FootballFinalTeam,
    pub away: FootballFinalTeam,
}

#[derive(Serialize, ToSchema)]
pub struct FootballFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Points per quarter, quarter 1 first; overtime periods extend past 4.
    pub line_score: Vec<u8>,
}

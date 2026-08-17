//! Soccer outbound domain model — the JSON DTOs the handlers serialize and
//! `wire.rs` encodes. The inbound ESPN parse (scoreboard AND the per-event
//! summary) lives in `crates/scoreboard-espn::soccer`; `adapter.rs` bridges
//! its extracts here.

use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::game::Side;
use crate::shared::team::{TeamColors, TeamState};

/// One soccer game, discriminated on the cross-sport `pre/in/post` state.
/// All three states are served (like MLB); the firmware renders pregame via
/// its shared pregame pipeline and final via the soccer full-time screen.
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum SoccerGame {
    Pregame(SoccerPregameGame),
    Live(SoccerLiveGame),
    Final(SoccerFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, and venue.
#[derive(Serialize, ToSchema)]
pub struct SoccerPregameGame {
    pub game_id: String,
    /// Scheduled start, unix epoch seconds UTC (what the wire carries).
    pub start_time: u32,
    /// Stadium name (ESPN `venue.fullName`); 100%-present in the corpus.
    pub venue: String,
    pub home: SoccerPregameTeam,
    pub away: SoccerPregameTeam,
}

/// Live state snapshot for one soccer game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct SoccerLiveGame {
    pub game_id: String,
    /// Raw ESPN clock, display-shaped (e.g. "45'+6'", "90'+3'").
    pub clock: String,
    /// Elapsed match seconds parsed from `clock` (floor minutes × 60);
    /// what the wire carries — the firmware extrapolates from it.
    pub clock_seconds: u16,
    /// ESPN's raw competition period: regulation halves 1/2, extra-time halves
    /// 3/4, shootout 5.
    pub half: u8,
    /// True during a non-playing break (halftime, extra-time halftime, end of
    /// regulation, end of extra time) — the clock alone cannot distinguish a
    /// break from active stoppage time.
    pub on_break: bool,
    pub home: TeamState,
    pub away: TeamState,
    pub last_event: Option<LastEvent>,
    /// Latest play-by-play commentary line (from the summary endpoint),
    /// e.g. "Goal! Argentina 3, Egypt 2. Lionel Messi converts...".
    /// Absent when the summary has no commentary or its fetch failed —
    /// commentary is best-effort and never blocks the live payload.
    pub commentary: Option<Commentary>,
}

/// Final snapshot: per-side scores, pre-formatted scorer lists, and how the
/// match was decided.
#[derive(Serialize, ToSchema)]
pub struct SoccerFinalGame {
    pub game_id: String,
    /// Full time, after extra time, or on penalties — the wire `flavor` byte.
    pub flavor: SoccerFinalFlavor,
    pub home: SoccerFinalTeam,
    pub away: SoccerFinalTeam,
}

/// How a finished match was decided. Carried as the final wire `flavor` byte
/// (0/1/2); the firmware renders the "AET"/"pens" annotation from it.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SoccerFinalFlavor {
    FullTime,
    AfterExtraTime,
    AfterPenalties,
}

/// One commentary line; `id` is the ESPN sequence number as a string — the
/// firmware compares it to detect new lines (same contract as MLB's play id).
#[derive(Serialize, ToSchema)]
pub struct Commentary {
    pub id: String,
    pub text: String,
}

#[derive(Serialize, ToSchema)]
pub struct SoccerPregameTeam {
    /// Team abbreviation, e.g. "POR" — key for `/{sport}/{league}/teams/{abbrev}/logo`.
    pub abbreviation: String,
    pub colors: TeamColors,
}

#[derive(Serialize, ToSchema)]
pub struct SoccerFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Pre-formatted goal-scorer list ("M. Merino 90'+1', F. Torres 12'"),
    /// empty when the team didn't score. Built once here so the firmware
    /// never formats strings.
    pub scorers: String,
}

/// The most recent goal or red card.
#[derive(Serialize, ToSchema)]
pub struct LastEvent {
    /// e.g. "Goal - R. Lukaku" or "Red Card - J. Doe".
    pub text: String,
    /// Structured kind — what the wire carries (with `athlete`).
    pub kind: EventKind,
    /// Athlete short name ("R. Lukaku"), empty if ESPN lists no athlete.
    pub athlete: String,
    /// Match clock of the event, display-shaped (e.g. "90'+3'").
    pub clock: String,
    /// Which side the event belongs to; absent if ESPN omits the team.
    pub team: Option<Side>,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Goal,
    RedCard,
}

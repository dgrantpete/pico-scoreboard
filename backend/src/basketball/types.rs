use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::types::{Color, FinalStatus, Team, Winner};

// ── List endpoint response (from scoreboard -- no fouls available) ──

/// Basketball game response for list endpoints (scoreboard data).
/// No fouls -- scoreboard doesn't include them.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum BasketballGameResponse {
    Pregame(BasketballPregame),
    Live(BasketballLive),
    Final(BasketballFinal),
}

/// Basketball pregame data. Shared by both list and detail responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballPregame {
    pub event_id: String,
    pub home: Team,
    pub away: Team,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<String>,
    // no weather -- indoor sport
}

/// Team score for list endpoints (no fouls).
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballTeamScore {
    pub abbreviation: String,
    pub color: Color,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u8>,
    pub score: u16,
}

/// Live basketball game from scoreboard (no fouls).
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballLive {
    pub event_id: String,
    pub home: BasketballTeamScore,
    pub away: BasketballTeamScore,
    pub period: BasketballPeriod,
    pub clock: String,
}

/// Final basketball game from scoreboard (no fouls).
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballFinal {
    pub event_id: String,
    pub home: BasketballTeamScore,
    pub away: BasketballTeamScore,
    pub status: FinalStatus,
    pub winner: Winner,
}

// ── Single-game detail response (from summary -- has fouls) ──

/// Basketball game detail for single-game endpoints (summary data with fouls).
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum BasketballGameDetail {
    Pregame(BasketballPregame),
    Live(BasketballLiveDetail),
    Final(BasketballFinalDetail),
}

/// Team score for detail endpoints (includes fouls).
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballTeamScoreDetail {
    pub abbreviation: String,
    pub color: Color,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u8>,
    pub score: u16,
    pub fouls: u8,
}

/// Live basketball game detail (with fouls).
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballLiveDetail {
    pub event_id: String,
    pub home: BasketballTeamScoreDetail,
    pub away: BasketballTeamScoreDetail,
    pub period: BasketballPeriod,
    pub clock: String,
}

/// Final basketball game detail (with fouls).
#[derive(Debug, Serialize, ToSchema)]
pub struct BasketballFinalDetail {
    pub event_id: String,
    pub home: BasketballTeamScoreDetail,
    pub away: BasketballTeamScoreDetail,
    pub status: FinalStatus,
    pub winner: Winner,
}

// ── Shared basketball period enum ──

/// Basketball period. NBA uses quarters (Q1-Q4), NCAAB uses halves (H1-H2).
/// Both share overtime and halftime.
#[derive(Debug, Serialize, ToSchema)]
pub enum BasketballPeriod {
    Q1,
    Q2,
    Q3,
    Q4,
    H1,
    H2,
    OT,
    OT2,
    OT3,
    OT4,
    Halftime,
}

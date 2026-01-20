use serde::Serialize;
use utoipa::ToSchema;

/// The API response - a tagged enum that serializes with "state" discriminator
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum GameResponse {
    Pregame(PregameGame),
    Live(LiveGame),
    Final(FinalGame),
}

/// Team data shared across all game states
#[derive(Debug, Serialize, ToSchema)]
pub struct Team {
    pub abbreviation: String,
    pub color: Color,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
}

/// RGB color as a strongly-typed struct
#[derive(Debug, Serialize, ToSchema)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Pregame-specific data
#[derive(Debug, Serialize, ToSchema)]
pub struct PregameGame {
    pub event_id: String,
    pub home: Team,
    pub away: Team,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weather: Option<Weather>,
}

/// Weather information
#[derive(Debug, Serialize, ToSchema)]
pub struct Weather {
    pub temp: i16,
    pub description: String,
}

/// Live game-specific data
#[derive(Debug, Serialize, ToSchema)]
pub struct LiveGame {
    pub event_id: String,
    pub home: TeamWithScore,
    pub away: TeamWithScore,
    pub quarter: Quarter,
    pub clock: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub situation: Option<Situation>,
}

/// Team with score and timeouts (for live/final games)
#[derive(Debug, Serialize, ToSchema)]
pub struct TeamWithScore {
    pub abbreviation: String,
    pub color: Color,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    pub score: u8,
    pub timeouts: u8,
}

/// Quarter as a strongly-typed enum
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Quarter {
    First,
    Second,
    Third,
    Fourth,
    #[serde(rename = "OT")]
    Overtime,
    #[serde(rename = "OT2")]
    DoubleOvertime,
}

/// Current play situation (only during active play)
#[derive(Debug, Serialize, ToSchema)]
pub struct Situation {
    pub down: Down,
    pub distance: u8,
    pub yard_line: u8,
    pub possession: Possession,
    pub red_zone: bool,
}

/// Down as a strongly-typed enum
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Down {
    First,
    Second,
    Third,
    Fourth,
}

/// Possession indicator
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Possession {
    Home,
    Away,
}

/// Final game-specific data
#[derive(Debug, Serialize, ToSchema)]
pub struct FinalGame {
    pub event_id: String,
    pub home: TeamWithScore,
    pub away: TeamWithScore,
    pub status: FinalStatus,
    pub winner: Winner,
}

/// Final status variants
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FinalStatus {
    Final,
    #[serde(rename = "final/OT")]
    FinalOvertime,
}

/// Winner indicator
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Winner {
    Home,
    Away,
    Tie,
}

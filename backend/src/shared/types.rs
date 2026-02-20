use serde::Serialize;
use utoipa::ToSchema;

/// RGB color as a strongly-typed struct
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Team data shared across all game states.
/// Used by both football and basketball pregame responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct Team {
    pub abbreviation: String,
    pub color: Color,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// AP/Coaches ranking (college sports only; absent for pro leagues)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u8>,
}

/// Weather information (football only — basketball is indoor)
#[derive(Debug, Serialize, ToSchema)]
pub struct Weather {
    pub temp: i16,
    pub description: String,
}

/// Final status variants — universal across all sports
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FinalStatus {
    Final,
    #[serde(rename = "final/OT")]
    FinalOvertime,
}

/// Winner indicator — universal across all sports
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Winner {
    Home,
    Away,
    Tie,
}

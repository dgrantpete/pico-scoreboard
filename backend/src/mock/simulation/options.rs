//! Request types for creating game simulations.
//!
//! Uses a discriminated union (tagged enum) to allow creating games
//! in any of the three states: pregame, live, or final.

use serde::Deserialize;
use utoipa::ToSchema;

use crate::game::types::{Down, Possession, Quarter};

/// Request body for creating a new game simulation.
///
/// This is a discriminated union - the `state` field determines which
/// variant is used and what options are available.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum CreateGameRequest {
    /// Create a game in pregame state (not yet started)
    Pregame(CreatePregameOptions),
    /// Create a game already in progress
    Live(CreateLiveOptions),
    /// Create a completed game
    Final(CreateFinalOptions),
}

/// Options for creating a pregame.
///
/// Pregame stores minimal config. The `seed` drives all randomness
/// when the game transitions to live state.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CreatePregameOptions {
    /// Home team abbreviation (e.g., "KC"). Random if not specified.
    pub home_team: Option<String>,
    /// Away team abbreviation (e.g., "PHI"). Random if not specified.
    pub away_team: Option<String>,

    /// When the game transitions to live state (ISO 8601 datetime).
    /// Default: ~30 seconds in the future.
    pub start_time: Option<String>,

    /// Stadium name. Random if not specified.
    pub venue: Option<String>,
    /// Broadcast network. Random if not specified.
    pub broadcast: Option<String>,
    /// Weather conditions. Random if not specified.
    pub weather: Option<CreateWeatherOptions>,

    /// Random seed for simulation. Used when game transitions to live.
    /// Random if not specified.
    pub seed: Option<u64>,
    /// Time acceleration factor for live simulation.
    /// 1.0 = real-time, 60.0 = 60x speed (full game in ~3 min).
    /// Default: 60.0
    pub time_scale: Option<f64>,
}

/// Weather options for pregame creation.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWeatherOptions {
    /// Temperature in Fahrenheit. Random 20-85 if not specified.
    pub temp: Option<i16>,
    /// Weather description (e.g., "Partly Cloudy"). Random if not specified.
    pub description: Option<String>,
}

/// Options for creating a live (in-progress) game.
///
/// All fields are optional - unspecified values are randomized.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CreateLiveOptions {
    /// Home team abbreviation. Random if not specified.
    pub home_team: Option<String>,
    /// Away team abbreviation. Random if not specified.
    pub away_team: Option<String>,

    /// Home team score. Default: 0.
    pub home_score: Option<u8>,
    /// Away team score. Default: 0.
    pub away_score: Option<u8>,

    /// Current quarter. Default: First.
    pub quarter: Option<Quarter>,
    /// Game clock in "MM:SS" format (e.g., "8:42"). Default: "15:00".
    pub clock: Option<String>,

    /// Team with possession. Random if not specified.
    pub possession: Option<Possession>,
    /// Current down. Default: First if possession is set.
    pub down: Option<Down>,
    /// Yards to go for first down. Default: 10.
    pub distance: Option<u8>,
    /// Yard line (0-100 from possessing team's perspective). Default: 25 (after touchback).
    pub yard_line: Option<u8>,

    /// Home team remaining timeouts. Default: 3.
    pub home_timeouts: Option<u8>,
    /// Away team remaining timeouts. Default: 3.
    pub away_timeouts: Option<u8>,

    /// Random seed for simulation progression.
    pub seed: Option<u64>,
    /// Time acceleration factor.
    /// 1.0 = real-time, 60.0 = 60x speed.
    /// Default: 60.0
    pub time_scale: Option<f64>,
}

/// Options for creating a final (completed) game.
///
/// No seed is needed - final games are fully deterministic.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CreateFinalOptions {
    /// Home team abbreviation. Random if not specified.
    pub home_team: Option<String>,
    /// Away team abbreviation. Random if not specified.
    pub away_team: Option<String>,

    /// Home team final score. Random realistic score if not specified.
    pub home_score: Option<u8>,
    /// Away team final score. Random realistic score if not specified.
    pub away_score: Option<u8>,

    /// Whether the game went to overtime. Default: false.
    pub overtime: Option<bool>,
}

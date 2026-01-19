use serde::Deserialize;

/// Root response from ESPN scoreboard API
#[derive(Debug, Deserialize)]
pub struct EspnScoreboard {
    pub events: Vec<EspnEvent>,
}

/// Single game/event from ESPN
#[derive(Debug, Deserialize)]
pub struct EspnEvent {
    pub id: String,
    #[allow(dead_code)]
    pub date: String,
    pub status: EspnStatus,
    pub competitions: Vec<EspnCompetition>,
    pub weather: Option<EspnWeather>,
    #[serde(rename = "geoBroadcasts", default)]
    pub geo_broadcasts: Vec<EspnBroadcast>,
}

/// Game status information
#[derive(Debug, Deserialize)]
pub struct EspnStatus {
    pub period: u8,
    #[serde(rename = "displayClock")]
    pub display_clock: String,
    #[serde(rename = "type")]
    pub status_type: EspnStatusType,
}

/// Status type with state and display info
#[derive(Debug, Deserialize)]
pub struct EspnStatusType {
    pub state: String,
    #[serde(rename = "shortDetail")]
    pub short_detail: String,
}

/// Competition (the actual matchup)
#[derive(Debug, Deserialize)]
pub struct EspnCompetition {
    pub competitors: Vec<EspnCompetitor>,
    pub situation: Option<EspnSituation>,
    pub venue: Option<EspnVenue>,
}

/// Team competitor in a game
#[derive(Debug, Deserialize)]
pub struct EspnCompetitor {
    pub team: EspnTeam,
    pub score: Option<String>,
    #[serde(rename = "homeAway")]
    pub home_away: String,
    #[serde(default)]
    pub records: Vec<EspnRecord>,
}

/// Team information
#[derive(Debug, Deserialize)]
pub struct EspnTeam {
    pub id: String,
    pub abbreviation: String,
    pub color: Option<String>,
}

/// Team record
#[derive(Debug, Deserialize)]
pub struct EspnRecord {
    pub summary: String,
}

/// Live game situation (only present during active play)
#[derive(Debug, Deserialize)]
pub struct EspnSituation {
    pub down: Option<u8>,
    #[serde(rename = "distance")]
    pub distance: Option<u8>,
    #[serde(rename = "yardLine")]
    pub yard_line: Option<u8>,
    pub possession: Option<String>,
    #[serde(rename = "isRedZone")]
    pub is_red_zone: Option<bool>,
    #[serde(rename = "homeTimeouts")]
    pub home_timeouts: Option<u8>,
    #[serde(rename = "awayTimeouts")]
    pub away_timeouts: Option<u8>,
}

/// Venue information
#[derive(Debug, Deserialize)]
pub struct EspnVenue {
    #[serde(rename = "fullName")]
    pub full_name: String,
    pub indoor: Option<bool>,
}

/// Weather information
#[derive(Debug, Deserialize)]
pub struct EspnWeather {
    pub temperature: Option<i16>,
    #[serde(rename = "displayValue")]
    pub display_value: Option<String>,
}

/// Broadcast information
#[derive(Debug, Deserialize)]
pub struct EspnBroadcast {
    pub media: Option<EspnMedia>,
}

/// Media/network information
#[derive(Debug, Deserialize)]
pub struct EspnMedia {
    #[serde(rename = "shortName")]
    pub short_name: String,
}

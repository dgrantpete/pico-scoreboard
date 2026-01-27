use serde::Deserialize;

/// Root response from ESPN scoreboard API
#[derive(Debug, Deserialize)]
pub struct EspnScoreboard {
    pub events: Vec<EspnEvent>,
}

/// Single game/event from ESPN
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EspnEvent {
    pub id: String,
    #[allow(dead_code)]
    pub date: String,
    pub status: EspnStatus,
    pub competitions: Vec<EspnCompetition>,
    pub weather: Option<EspnWeather>,
    #[serde(default)]
    pub geo_broadcasts: Vec<EspnBroadcast>,
}

/// Game status information
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EspnStatus {
    pub period: u8,
    pub display_clock: String,
    #[serde(rename = "type")]
    pub status_type: EspnStatusType,
}

/// Status type with state and display info
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EspnStatusType {
    pub id: String,
    pub state: String,
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
#[serde(rename_all = "camelCase")]
pub struct EspnCompetitor {
    pub team: EspnTeam,
    pub score: Option<String>,
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
#[serde(rename_all = "camelCase")]
pub struct EspnSituation {
    pub down: Option<i8>,
    pub distance: Option<i8>,
    pub yard_line: Option<i8>,
    pub possession: Option<String>,
    pub is_red_zone: Option<bool>,
    pub home_timeouts: Option<u8>,
    pub away_timeouts: Option<u8>,
    pub last_play: Option<EspnLastPlay>,
}

/// Last play information
#[derive(Debug, Deserialize)]
pub struct EspnLastPlay {
    pub id: String,
    #[serde(rename = "type")]
    pub play_type: EspnPlayType,
    pub text: Option<String>,
}

/// Play type information
#[derive(Debug, Deserialize)]
pub struct EspnPlayType {
    pub id: String,
    pub text: Option<String>,
}

/// Venue information
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EspnVenue {
    pub full_name: String,
    pub indoor: Option<bool>,
}

/// Weather information
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EspnWeather {
    pub temperature: Option<i16>,
    pub display_value: Option<String>,
}

/// Broadcast information
#[derive(Debug, Deserialize)]
pub struct EspnBroadcast {
    pub media: Option<EspnMedia>,
}

/// Media/network information
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EspnMedia {
    pub short_name: String,
}

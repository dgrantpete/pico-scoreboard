use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
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
    /// Whether the game clock is believed to be running.
    /// Computed from game status and last play type using NFL rules.
    pub clock_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub situation: Option<Situation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_play: Option<LastPlay>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Down {
    First,
    Second,
    Third,
    Fourth,
}

/// Possession indicator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

/// Last play information (simplified)
#[derive(Debug, Serialize, ToSchema)]
pub struct LastPlay {
    pub play_type: PlayType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Play type from ESPN API.
///
/// These IDs are reverse-engineered from ESPN's undocumented API.
/// Sources:
/// - Live API observation from multiple NFL games
/// - <https://gist.github.com/nntrn/ee26cb2a0716de0947a0a4e9a157bc1c>
/// - <https://gist.github.com/akeaswaran/b48b02f1c94f873c6655e7129910fc3b>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlayType {
    // === Administrative / Game Flow ===
    /// End of period (ID: 2)
    EndPeriod,
    /// End of half (ID: 65)
    EndHalf,
    /// End of game (ID: 66)
    EndGame,
    /// Coin toss (ID: 70)
    CoinToss,
    /// Team timeout (ID: 21)
    Timeout,
    /// Official/TV timeout (ID: 74)
    OfficialTimeout,
    /// Two-minute warning (ID: 75)
    TwoMinuteWarning,

    // === Passing Plays ===
    /// Completed pass / reception (ID: 24)
    PassReception,
    /// Pass ruled incomplete (ID: 3)
    PassIncompletion,
    /// Pass intercepted (ID: 26)
    Interception,
    /// Interception returned for touchdown (ID: 36)
    InterceptionReturnTouchdown,
    /// Passing touchdown (ID: 67)
    PassingTouchdown,
    /// Quarterback sacked (ID: 7)
    Sack,

    // === Rushing Plays ===
    /// Running play (ID: 5)
    Rush,
    /// Rushing touchdown (ID: 68)
    RushingTouchdown,
    /// Two-point conversion rush (ID: 16)
    TwoPointRush,

    // === Fumbles ===
    /// Fumble recovered by own team (ID: 9)
    FumbleRecoveryOwn,
    /// Fumble recovered by opponent (ID: 29)
    FumbleRecoveryOpponent,

    // === Kicking - Field Goals ===
    /// Successful field goal (ID: 59)
    FieldGoalGood,
    /// Missed field goal (ID: 60)
    FieldGoalMissed,
    /// Field goal blocked (ID: 18)
    BlockedFieldGoal,
    /// Missed field goal returned (ID: 40)
    MissedFieldGoalReturn,

    // === Kicking - Punts ===
    /// Punt (ID: 52)
    Punt,
    /// Punt blocked (ID: 17)
    BlockedPunt,

    // === Kicking - Kickoffs ===
    /// Kickoff (ID: 53)
    Kickoff,
    /// Kickoff return by offense (ID: 12)
    KickoffReturn,
    /// Kickoff return touchdown (ID: 32)
    KickoffReturnTouchdown,

    // === Extra Points ===
    /// Extra point good (ID: 61)
    ExtraPointGood,
    /// Extra point missed (ID: 62)
    ExtraPointMissed,
    /// Two-point conversion pass (ID: 15)
    TwoPointPass,

    // === Scoring / Safety ===
    /// Safety (ID: 20)
    Safety,

    // === Penalties ===
    /// Penalty called (ID: 8)
    Penalty,

    /// Unknown or unmapped play type
    Unknown,
}

impl PlayType {
    /// Parse ESPN play type ID to our enum.
    ///
    /// ID mappings are reverse-engineered from ESPN's undocumented API.
    /// Logs a warning when an unknown play type ID is encountered.
    pub fn from_espn_id(id: &str) -> Self {
        let play_type = Self::from_espn_id_inner(id);
        if play_type == PlayType::Unknown {
            tracing::warn!(
                play_type_id = %id,
                "Unknown ESPN play type ID encountered - please report this!"
            );
        }
        play_type
    }

    /// Parse ESPN play type ID with additional context for logging.
    ///
    /// Use this when you have the play text available for better logging.
    pub fn from_espn_id_with_context(id: &str, text: Option<&str>) -> Self {
        let play_type = Self::from_espn_id_inner(id);
        if play_type == PlayType::Unknown {
            tracing::warn!(
                play_type_id = %id,
                play_text = %text.unwrap_or("<no text>"),
                "Unknown ESPN play type ID encountered - please report this!"
            );
        }
        play_type
    }

    fn from_espn_id_inner(id: &str) -> Self {
        match id {
            // Administrative / Game Flow
            "2" => PlayType::EndPeriod,
            "21" => PlayType::Timeout,
            "65" => PlayType::EndHalf,
            "66" => PlayType::EndGame,
            "70" => PlayType::CoinToss,
            "74" => PlayType::OfficialTimeout,
            "75" => PlayType::TwoMinuteWarning,

            // Passing
            "3" => PlayType::PassIncompletion,
            "24" => PlayType::PassReception,
            "26" => PlayType::Interception,
            "36" => PlayType::InterceptionReturnTouchdown,
            "67" => PlayType::PassingTouchdown,
            "7" => PlayType::Sack,

            // Rushing
            "5" => PlayType::Rush,
            "16" => PlayType::TwoPointRush,
            "68" => PlayType::RushingTouchdown,

            // Fumbles
            "9" => PlayType::FumbleRecoveryOwn,
            "29" => PlayType::FumbleRecoveryOpponent,

            // Field Goals
            "18" => PlayType::BlockedFieldGoal,
            "40" => PlayType::MissedFieldGoalReturn,
            "59" => PlayType::FieldGoalGood,
            "60" => PlayType::FieldGoalMissed,

            // Punts
            "17" => PlayType::BlockedPunt,
            "52" => PlayType::Punt,

            // Kickoffs
            "12" => PlayType::KickoffReturn,
            "32" => PlayType::KickoffReturnTouchdown,
            "53" => PlayType::Kickoff,

            // Extra Points
            "15" => PlayType::TwoPointPass,
            "61" => PlayType::ExtraPointGood,
            "62" => PlayType::ExtraPointMissed,

            // Scoring
            "20" => PlayType::Safety,

            // Penalties
            "8" => PlayType::Penalty,

            _ => PlayType::Unknown,
        }
    }

    /// Returns the ESPN API ID for this play type, if known.
    pub fn espn_id(&self) -> Option<&'static str> {
        match self {
            PlayType::EndPeriod => Some("2"),
            PlayType::Timeout => Some("21"),
            PlayType::EndHalf => Some("65"),
            PlayType::EndGame => Some("66"),
            PlayType::CoinToss => Some("70"),
            PlayType::OfficialTimeout => Some("74"),
            PlayType::TwoMinuteWarning => Some("75"),
            PlayType::PassIncompletion => Some("3"),
            PlayType::PassReception => Some("24"),
            PlayType::Interception => Some("26"),
            PlayType::InterceptionReturnTouchdown => Some("36"),
            PlayType::PassingTouchdown => Some("67"),
            PlayType::Sack => Some("7"),
            PlayType::Rush => Some("5"),
            PlayType::TwoPointRush => Some("16"),
            PlayType::RushingTouchdown => Some("68"),
            PlayType::FumbleRecoveryOwn => Some("9"),
            PlayType::FumbleRecoveryOpponent => Some("29"),
            PlayType::BlockedFieldGoal => Some("18"),
            PlayType::MissedFieldGoalReturn => Some("40"),
            PlayType::FieldGoalGood => Some("59"),
            PlayType::FieldGoalMissed => Some("60"),
            PlayType::BlockedPunt => Some("17"),
            PlayType::Punt => Some("52"),
            PlayType::KickoffReturn => Some("12"),
            PlayType::KickoffReturnTouchdown => Some("32"),
            PlayType::Kickoff => Some("53"),
            PlayType::TwoPointPass => Some("15"),
            PlayType::ExtraPointGood => Some("61"),
            PlayType::ExtraPointMissed => Some("62"),
            PlayType::Safety => Some("20"),
            PlayType::Penalty => Some("8"),
            PlayType::Unknown => None,
        }
    }

    /// Returns true if this play type always stops the clock.
    ///
    /// Based on NFL rulebook clock rules.
    pub fn stops_clock(&self) -> bool {
        matches!(
            self,
            // Incomplete/intercepted passes
            PlayType::PassIncompletion
                | PlayType::Interception
                | PlayType::InterceptionReturnTouchdown
            // Timeouts and stoppages
                | PlayType::Timeout
                | PlayType::OfficialTimeout
                | PlayType::TwoMinuteWarning
                | PlayType::EndPeriod
                | PlayType::EndHalf
                | PlayType::EndGame
            // Scoring plays (clock stops after score)
                | PlayType::PassingTouchdown
                | PlayType::RushingTouchdown
                | PlayType::FieldGoalGood
                | PlayType::Safety
                | PlayType::KickoffReturnTouchdown
            // Change of possession / kicks
                | PlayType::Punt
                | PlayType::Kickoff
                | PlayType::FieldGoalMissed
                | PlayType::BlockedFieldGoal
                | PlayType::BlockedPunt
                | PlayType::MissedFieldGoalReturn
                | PlayType::FumbleRecoveryOpponent
            // Penalties
                | PlayType::Penalty
            // Extra points (between TD and kickoff)
                | PlayType::ExtraPointGood
                | PlayType::ExtraPointMissed
                | PlayType::TwoPointRush
                | PlayType::TwoPointPass
        )
    }

    /// Returns true if clock behavior depends on play details (e.g., out of bounds).
    pub fn clock_depends_on_details(&self) -> bool {
        matches!(
            self,
            PlayType::Rush
                | PlayType::PassReception
                | PlayType::Sack
                | PlayType::KickoffReturn
                | PlayType::FumbleRecoveryOwn
        )
    }
}
